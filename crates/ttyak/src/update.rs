//! The update check: `latest.json` from the site, read with a plain GET (nothing about you is
//! sent), at most every `check_every_secs` (`[updates]` in settings.toml). A newer version is
//! notified once, ever: the last one notified is kept in `<data>/updates.json`.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use semver::Version;
use serde::{Deserialize, Serialize};

/// Where the newest release is described.
pub const URL: &str = "https://ttyak.sh/latest.json";
/// Overrides [`URL`] (a local server in tests).
pub const URL_ENV: &str = "TTYAK_UPDATE_URL";
/// How often whether a check is due is looked at again, in seconds (the settings may have
/// changed meanwhile).
const LOOK_AGAIN_SECS: u64 = 3600;

/// The newest release for this platform, from `latest.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct Latest {
    pub version: Version,
    /// `YYYY-MM-DD`.
    pub date: String,
    /// The release notes' link.
    pub notes: String,
    pub asset: Asset,
}

/// This platform's archive.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Asset {
    pub url: String,
    pub sha256: String,
}

/// The newest release described in `json`, with `platform`'s archive (an error when it has none).
pub fn parse(json: &str, platform: &str) -> Result<Latest> {
    #[derive(Deserialize)]
    struct Feed {
        version: String,
        date: String,
        notes: String,
        assets: std::collections::HashMap<String, Asset>,
    }
    let mut feed: Feed = serde_json::from_str(json).context("latest.json")?;
    let version = Version::parse(&feed.version).with_context(|| format!("latest.json: version {:?}", feed.version))?;
    let asset = feed.assets.remove(platform).with_context(|| format!("latest.json: no archive for {platform}"))?;
    Ok(Latest { version, date: feed.date, notes: feed.notes, asset })
}

/// Whether `latest` is newer than `current`; a pre-release is never offered.
pub fn newer(current: &Version, latest: &Latest) -> bool {
    latest.version.pre.is_empty() && latest.version > *current
}

/// What `updates.json` keeps between runs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// The last check (whether or not it got an answer), in Unix seconds.
    pub last_check: u64,
    /// The last version notified.
    pub notified: Option<String>,
}

/// `<data>/updates.json`.
pub fn state_path() -> Option<PathBuf> {
    slack_core::session::project_dirs().map(|d| d.data_local_dir().join("updates.json"))
}

/// The state in `path`; none yet (or a broken file): a fresh one.
pub fn load_state(path: &Path) -> State {
    std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

/// Write `state` to `path` as a whole (through a file beside it).
pub fn save_state(path: &Path, state: &State) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    slack_core::session::write_private(path, serde_json::to_string(state)?.as_bytes())
}

/// Whether a check is due at `now`, `every` seconds after the last one (never checked, or the
/// clock set back: at once).
pub fn due(state: &State, now: u64, every: u64) -> bool {
    state.last_check == 0 || now < state.last_check || now - state.last_check >= every
}

/// Whether `latest` is still to be notified: newer than the version notified last.
pub fn should_notify(state: &State, latest: &Latest) -> bool {
    state.notified.as_deref().and_then(|v| Version::parse(v).ok()).is_none_or(|notified| latest.version > notified)
}

/// What a check at `now` that ended with `result` leaves: the state to write (before anything is
/// shown, so a crash can never notify twice) and the version to notify, if any.
pub fn after_check(state: &State, now: u64, current: &Version, result: Result<Latest>) -> (State, Option<Latest>) {
    // Tried, answered or not: the next try is a whole interval later.
    let mut next = State { last_check: now, ..state.clone() };
    let latest = match result {
        Ok(latest) => latest,
        Err(e) => {
            tracing::debug!("update check: {e:#}");
            return (next, None);
        }
    };
    if !newer(current, &latest) || !should_notify(state, &latest) {
        return (next, None);
    }
    next.notified = Some(latest.version.to_string());
    (next, Some(latest))
}

/// A check of `url` when one is due under `settings`, for `platform`: what [`after_check`] gives;
/// None when the check is off, not due or ttyak is not released for this platform (nothing is
/// fetched then).
pub async fn check<F, Fut>(settings: &crate::tui::settings::Updates, state: &State, now: u64, current: &Version, platform: Option<&str>, url: &str, fetch: F) -> Option<(State, Option<Latest>)>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Result<String>>,
{
    let platform = platform?;
    if !settings.check || !due(state, now, settings.check_every_secs) {
        return None;
    }
    let result = fetch(url.to_string()).await.and_then(|json| parse(&json, platform));
    Some(after_check(state, now, current, result))
}

/// `latest.json` at `url`: a GET with ttyak's name and version as the User-Agent, nothing else.
pub async fn fetch(url: &str) -> Result<String> {
    let http = slack_core::api::http_client(|| slack_core::reqwest::Client::builder().user_agent(concat!("ttyak/", env!("CARGO_PKG_VERSION"))).timeout(Duration::from_secs(10)))?;
    Ok(http.get(url).send().await?.error_for_status()?.text().await?)
}

/// How long until whether a check is due is looked at again.
fn wait(settings: &crate::tui::settings::Updates, state: &State, now: u64) -> Duration {
    let until_due = if settings.check { (state.last_check + settings.check_every_secs).saturating_sub(now) } else { LOOK_AGAIN_SECS };
    Duration::from_secs(until_due.clamp(60, LOOK_AGAIN_SECS))
}

/// Check in the background, the first time at once when due, then as it comes due (the settings
/// read again each time); a new version to notify is sent on `found`.
pub fn spawn(found: tokio::sync::mpsc::UnboundedSender<Latest>) {
    let (Some(path), Some(platform)) = (state_path(), crate::install::platform()) else { return };
    let current = Version::parse(env!("CARGO_PKG_VERSION")).expect("the package version is semver");
    let url = std::env::var(URL_ENV).ok().filter(|u| !u.is_empty()).unwrap_or_else(|| URL.to_string());
    tokio::spawn(async move {
        loop {
            let settings = crate::tui::settings::load().0.updates;
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            // Read each time: another ttyak (another profile) may have checked meanwhile.
            let mut state = load_state(&path);
            if let Some((next, latest)) = check(&settings, &state, now, &current, Some(platform), &url, |u| async move { fetch(&u).await }).await {
                match save_state(&path, &next) {
                    // Only once it is written: never notified twice.
                    Ok(()) => {
                        if let Some(latest) = latest {
                            if found.send(latest).is_err() {
                                return;
                            }
                        }
                    }
                    Err(e) => tracing::debug!("update check: {}: {e:#}", path.display()),
                }
                state = next;
            }
            tokio::time::sleep(wait(&settings, &state, now)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::settings::Updates;

    const FEED: &str = r#"{"name":"ttyak","version":"0.10.2","date":"2026-11-01","notes":"https://github.com/dmytrobsp/ttyak/releases/tag/v0.10.2",
        "assets":{"x86_64-linux":{"url":"https://github.com/dmytrobsp/ttyak/releases/download/v0.10.2/ttyak-0.10.2-x86_64-linux.zip","sha256":"ab12"},
                  "aarch64-macos":{"url":"https://example.invalid/mac.zip","sha256":"cd34"}}}"#;

    fn latest(v: &str) -> Latest {
        Latest { version: Version::parse(v).unwrap(), date: "2026-11-01".into(), notes: String::new(), asset: Asset { url: "u".into(), sha256: "s".into() } }
    }

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn parses_latest() {
        let l = parse(FEED, "x86_64-linux").unwrap();
        assert_eq!(l.version, v("0.10.2"));
        assert_eq!((l.date.as_str(), l.notes.as_str()), ("2026-11-01", "https://github.com/dmytrobsp/ttyak/releases/tag/v0.10.2"));
        assert_eq!(l.asset, Asset { url: "https://github.com/dmytrobsp/ttyak/releases/download/v0.10.2/ttyak-0.10.2-x86_64-linux.zip".into(), sha256: "ab12".into() });
        assert_eq!(parse(FEED, "aarch64-macos").unwrap().asset.sha256, "cd34");
    }

    #[test]
    fn missing_platform_is_an_error() {
        let e = parse(FEED, "x86_64-windows").unwrap_err();
        assert!(format!("{e:#}").contains("x86_64-windows"), "{e:#}");
    }

    #[test]
    fn malformed_is_an_error() {
        for bad in ["", "<html>404</html>", "{}", r#"{"version":"ten","date":"","notes":"","assets":{"x86_64-linux":{"url":"u","sha256":"s"}}}"#, r#"{"version":"0.10.2","date":"","notes":"","assets":{"x86_64-linux":{"url":"u"}}}"#] {
            assert!(parse(bad, "x86_64-linux").is_err(), "{bad:?}");
        }
    }

    #[test]
    fn older_equal_and_prerelease_are_not_newer() {
        let now = v("0.10.0");
        assert!(newer(&now, &latest("0.10.1")) && newer(&now, &latest("1.0.0")));
        assert!(!newer(&now, &latest("0.10.0")), "equal");
        assert!(!newer(&now, &latest("0.9.9")), "older");
        assert!(!newer(&now, &latest("0.11.0-rc.1")), "a pre-release is never offered");
        assert!(newer(&v("0.10.0-rc.2"), &latest("0.10.0")), "the release after its candidate");
    }

    #[test]
    fn due_after_the_interval_only() {
        let s = State { last_check: 1_000, notified: None };
        assert!(!due(&s, 1_000, 86_400) && !due(&s, 87_399, 86_400));
        assert!(due(&s, 87_400, 86_400) && due(&s, 200_000, 86_400));
        assert!(due(&State::default(), 1_000, 86_400), "never checked");
        assert!(due(&s, 999, 86_400), "the clock was set back");
    }

    #[test]
    fn notified_once() {
        let s = State { last_check: 0, notified: Some("0.10.1".into()) };
        assert!(!should_notify(&s, &latest("0.10.1")));
        assert!(!should_notify(&s, &latest("0.10.0")), "older than the one notified");
        assert!(should_notify(&s, &latest("0.10.2")));
        assert!(should_notify(&State::default(), &latest("0.10.1")));
        // Across runs: the state as written and read back.
        let dir = std::env::temp_dir().join(format!("ttyak-test-updates-{}", std::process::id()));
        let path = dir.join("updates.json");
        let (next, shown) = after_check(&State::default(), 50, &v("0.10.0"), Ok(latest("0.10.1")));
        assert_eq!(shown, Some(latest("0.10.1")));
        save_state(&path, &next).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&text).unwrap(), serde_json::json!({"last_check": 50, "notified": "0.10.1"}));
        let (again, shown) = after_check(&load_state(&path), 90_000, &v("0.10.0"), Ok(latest("0.10.1")));
        assert_eq!((again.last_check, shown), (90_000, None), "the next run checks and says nothing");
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(load_state(&path), State::default(), "a broken file starts afresh");
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(load_state(&path), State::default(), "no file yet");
    }

    #[test]
    fn state_written_before_notifying() {
        let old = State { last_check: 10, notified: Some("0.10.1".into()) };
        let (next, shown) = after_check(&old, 500, &v("0.10.0"), Ok(latest("0.10.2")));
        assert_eq!(shown, Some(latest("0.10.2")));
        assert_eq!(next, State { last_check: 500, notified: Some("0.10.2".into()) }, "the state to write names what is shown");
        let (next, shown) = after_check(&old, 600, &v("0.10.0"), Err(anyhow::anyhow!("offline")));
        assert_eq!((next, shown), (State { last_check: 600, notified: Some("0.10.1".into()) }, None), "a failed check counts as the day's");
        let (next, shown) = after_check(&old, 700, &v("0.10.2"), Ok(latest("0.10.2")));
        assert_eq!((next.notified.as_deref(), shown), (Some("0.10.1"), None), "not newer than this ttyak");
    }

    #[tokio::test]
    async fn check_off_never_fetches() {
        let asked = std::sync::atomic::AtomicUsize::new(0);
        let fetch = |url: String| {
            asked.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            async move {
                assert_eq!(url, "http://127.0.0.1:9/latest.json");
                Ok(FEED.to_string())
            }
        };
        let off = Updates { check: false, ..Default::default() };
        let old = State::default();
        let now = v("0.10.0");
        assert_eq!(check(&off, &old, 1_000_000, &now, Some("x86_64-linux"), "http://127.0.0.1:9/latest.json", fetch).await, None);
        let on = Updates::default();
        let recent = State { last_check: 999_000, notified: None };
        assert_eq!(check(&on, &recent, 1_000_000, &now, Some("x86_64-linux"), "http://127.0.0.1:9/latest.json", fetch).await, None, "not due");
        assert_eq!(check(&on, &old, 1_000_000, &now, None, "http://127.0.0.1:9/latest.json", fetch).await, None, "no release for this platform");
        assert_eq!(asked.load(std::sync::atomic::Ordering::Relaxed), 0);
        let (next, shown) = check(&on, &old, 1_000_000, &now, Some("x86_64-linux"), "http://127.0.0.1:9/latest.json", fetch).await.unwrap();
        assert_eq!((next.last_check, shown.map(|l| l.version)), (1_000_000, Some(v("0.10.2"))));
        assert_eq!(asked.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn fetch_sends_only_the_user_agent() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let _ = rustls::crypto::ring::default_provider().install_default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/latest.json", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0; 4096];
            let n = sock.read(&mut buf).await.unwrap();
            let body = "{\"ok\":1}";
            sock.write_all(format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            String::from_utf8_lossy(&buf[..n]).into_owned()
        });
        assert_eq!(fetch(&url).await.unwrap(), "{\"ok\":1}");
        let request = server.await.unwrap().to_lowercase();
        assert!(request.starts_with("get /latest.json http/1.1\r\n"), "{request}");
        assert!(request.contains(&format!("user-agent: ttyak/{}\r\n", env!("CARGO_PKG_VERSION"))), "{request}");
        let mut headers: Vec<&str> = request.lines().skip(1).filter(|l| !l.is_empty()).map(|l| l.split(':').next().unwrap()).collect();
        headers.sort();
        assert_eq!(headers, ["accept", "host", "user-agent"], "no cookies, nothing about you: {request}");
    }

    #[test]
    fn waits_until_due_at_most_an_hour() {
        let s = State { last_check: 1_000, notified: None };
        let every = |secs| Updates { check: true, check_every_secs: secs };
        assert_eq!(wait(&every(86_400), &s, 1_000), Duration::from_secs(3_600));
        assert_eq!(wait(&every(600), &s, 1_000), Duration::from_secs(600));
        assert_eq!(wait(&every(600), &s, 1_300), Duration::from_secs(300));
        assert_eq!(wait(&every(0), &s, 1_000), Duration::from_secs(60), "never in a tight loop");
        assert_eq!(wait(&Updates { check: false, ..every(0) }, &s, 1_000), Duration::from_secs(3_600), "off: looked at hourly");
    }
}

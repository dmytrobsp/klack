//! The update check: `latest.json` from the site, read with a plain GET (nothing about you is
//! sent), at most every `check_every_secs` (`[updates]` in settings.toml). A newer version is
//! notified once, ever: the last one notified is kept in `<data>/updates.json`. And the
//! self-update of a copy the install script put in a folder ttyak can write.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::install::{Install, Method};

/// Where the newest release is described.
pub const URL: &str = "https://ttyak.sh/latest.json";
/// Overrides [`URL`] (a local server in tests).
pub const URL_ENV: &str = "TTYAK_UPDATE_URL";
/// How often whether a check is due is looked at again, in seconds (the settings may have
/// changed meanwhile).
const LOOK_AGAIN_SECS: u64 = 3600;
/// The least `check_every_secs` used (the shortest F6 offers); a hand-edited one below it counts as it.
pub const MIN_CHECK_EVERY_SECS: u64 = 3600;
/// The least and the most `check_timeout_secs` used.
pub const MIN_CHECK_TIMEOUT_SECS: u64 = 5;
pub const MAX_CHECK_TIMEOUT_SECS: u64 = 600;
/// The least and the most `download_timeout_secs` used.
pub const MIN_DOWNLOAD_TIMEOUT_SECS: u64 = 10;
pub const MAX_DOWNLOAD_TIMEOUT_SECS: u64 = 86_400;
/// More than `latest.json` ever is.
const MAX_FEED_BYTES: u64 = 64 << 10;

/// `[updates]` as used: hand-edited seconds brought within [`MIN_CHECK_EVERY_SECS`],
/// [`MIN_CHECK_TIMEOUT_SECS`]–[`MAX_CHECK_TIMEOUT_SECS`] and
/// [`MIN_DOWNLOAD_TIMEOUT_SECS`]–[`MAX_DOWNLOAD_TIMEOUT_SECS`].
pub fn sane(u: crate::tui::settings::Updates) -> crate::tui::settings::Updates {
    crate::tui::settings::Updates {
        check_every_secs: u.check_every_secs.max(MIN_CHECK_EVERY_SECS),
        check_timeout_secs: u.check_timeout_secs.clamp(MIN_CHECK_TIMEOUT_SECS, MAX_CHECK_TIMEOUT_SECS),
        download_timeout_secs: u.download_timeout_secs.clamp(MIN_DOWNLOAD_TIMEOUT_SECS, MAX_DOWNLOAD_TIMEOUT_SECS),
        ..u
    }
}

/// `[updates]` from settings.toml as read now, [`sane`].
fn settings() -> crate::tui::settings::Updates {
    sane(crate::tui::settings::load().0.updates)
}

/// The newest release for this platform, from `latest.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct Latest {
    pub version: Version,
    /// `YYYY-MM-DD`.
    pub date: String,
    pub asset: Asset,
}

impl Latest {
    /// The release's page (its notes and downloads): built from the version, never the feed's link.
    pub fn page(&self) -> String {
        let base = std::env::var(PAGES_URL_ENV).ok().filter(|u| !u.is_empty()).unwrap_or_else(|| PAGES_URL.to_string());
        page_at(&base, &self.version)
    }
}

/// The release pages' parent on GitHub: `<base>/v<version>` (what packaging/render.py writes as the feed's `notes`).
pub const PAGES_URL: &str = "https://github.com/dmytrobsp/ttyak/releases/tag";
/// Overrides [`PAGES_URL`] (a local server in tests), as [`DOWNLOAD_URL_ENV`] does the downloads'.
pub const PAGES_URL_ENV: &str = "TTYAK_PAGES_URL";

/// `version`'s page under the release pages' parent `base`.
fn page_at(base: &str, version: &Version) -> String {
    format!("{}/v{version}", base.trim_end_matches('/'))
}

/// This platform's archive.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Asset {
    pub url: String,
    pub sha256: String,
}

/// The newest release described in `json`, with `platform`'s archive (an error when it has none).
pub fn parse(json: &str, platform: &str) -> Result<Latest> {
    // (Its `notes` link is not read: the release's page is built from the version.)
    #[derive(Deserialize)]
    struct Feed {
        version: String,
        date: String,
        assets: std::collections::HashMap<String, Asset>,
    }
    let mut feed: Feed = serde_json::from_str(json).context("latest.json")?;
    let version = Version::parse(&feed.version).with_context(|| format!("latest.json: version {:?}", feed.version))?;
    let asset = feed.assets.remove(platform).with_context(|| format!("latest.json: no archive for {platform}"))?;
    // What is downloaded and checked is refused, never cleaned up.
    ensure!(!asset.url.chars().chain(asset.sha256.chars()).any(hidden), "latest.json: a control character or an invisible format character in the archive's address or SHA256");
    Ok(Latest { version, date: printable(&feed.date), asset })
}

/// A character that is not shown as itself: a control (C0 with ESC, DEL, C1), a format character
/// (bidi overrides and isolates, zero-width ones, the soft hyphen, …) or a line or paragraph separator.
fn hidden(c: char) -> bool {
    c.is_control() || matches!(c, '\u{ad}' | '\u{600}'..='\u{605}' | '\u{61c}' | '\u{6dd}' | '\u{70f}' | '\u{8e2}' | '\u{180e}' | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206f}' | '\u{feff}' | '\u{fff9}'..='\u{fffb}' | '\u{110bd}' | '\u{110cd}' | '\u{13430}'..='\u{1343f}' | '\u{1bca0}'..='\u{1bca3}' | '\u{1d173}'..='\u{1d17a}' | '\u{e0001}' | '\u{e0020}'..='\u{e007f}')
}

/// `text` without the characters [`hidden`] names, so text from the feed can never steer the
/// terminal it is shown in or hide what it says.
pub fn printable(text: &str) -> String {
    text.chars().filter(|c| !hidden(*c)).collect()
}

/// Whether `latest` is newer than `current` (build metadata does not count); a pre-release is
/// never offered.
pub fn newer(current: &Version, latest: &Latest) -> bool {
    latest.version.pre.is_empty() && latest.version.cmp_precedence(current).is_gt()
}

/// A version newer than this ttyak, as a check found it.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub latest: Latest,
    /// Not notified before: the status line's pill for this run.
    pub notify: bool,
    /// And the desktop notification: only once `updates.json` says so, so no later run notifies it again.
    pub desktop: bool,
}

/// What a check asked for (F6 → Updates → Check now) found.
#[derive(Debug, Clone, PartialEq)]
pub enum Checked {
    UpToDate,
    /// This version, newer than this ttyak, is out.
    Out(Version),
    /// No answer, or none that could be read: why.
    Failed(String),
}

/// What the background check tells the app.
#[derive(Debug)]
pub enum Report {
    Found(Found),
    /// A check asked for ended (after its [`Report::Found`], if any).
    Checked(Checked),
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

/// The state in `path`; none yet, or a broken file: a fresh one (so a broken file forgets the
/// version notified, and that version may be notified once more).
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
    state.notified.as_deref().and_then(|v| Version::parse(v).ok()).is_none_or(|notified| latest.version.cmp_precedence(&notified).is_gt())
}

/// `disk` with `kept` (what this run last wrote, or tried to) as its floor: the later check and
/// the later version notified, so a state that could not be written still counts.
pub fn with_floor(disk: State, kept: &State) -> State {
    let version = |s: &State| s.notified.as_deref().and_then(|v| Version::parse(v).ok());
    let notified = match (version(&disk), version(kept)) {
        (Some(d), Some(k)) if k.cmp_precedence(&d).is_gt() => kept.notified.clone(),
        (None, Some(_)) => kept.notified.clone(),
        _ => disk.notified,
    };
    State { last_check: disk.last_check.max(kept.last_check), notified }
}

/// What a check at `now` that ended with `result` leaves: the state to write (before anything is
/// shown, so a crash can never notify twice) and the newer version, if any (to notify when it
/// was not before).
pub fn after_check(state: &State, now: u64, current: &Version, result: Result<Latest>) -> (State, Option<Found>) {
    // Tried, answered or not: the next try is a whole interval later.
    let mut next = State { last_check: now, ..state.clone() };
    let latest = match result {
        Ok(latest) => latest,
        Err(e) => {
            tracing::debug!("update check: {e:#}");
            return (next, None);
        }
    };
    if !newer(current, &latest) {
        return (next, None);
    }
    let notify = should_notify(state, &latest);
    if notify {
        next.notified = Some(latest.version.to_string());
    }
    (next, Some(Found { latest, notify, desktop: notify }))
}

/// A check of `url` when one is due under `settings`, for `platform`: what [`after_check`] gives;
/// None when the check is off, not due or ttyak is not released for this platform (nothing is
/// fetched then).
pub async fn check<F, Fut>(settings: &crate::tui::settings::Updates, state: &State, now: u64, current: &Version, platform: Option<&str>, url: &str, fetch: F) -> Option<(State, Option<Found>)>
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

/// A check asked for (F6 → Updates → Check now) of `url` for `platform`: due or not, with the check
/// off too. What [`after_check`] gives, the newer version never to notify (the page shows it, so it
/// counts as notified), and what to say.
pub async fn check_now<F, Fut>(state: &State, now: u64, current: &Version, platform: Option<&str>, url: &str, fetch: F) -> (State, Option<Found>, Checked)
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Result<String>>,
{
    let Some(platform) = platform else { return (state.clone(), None, Checked::Failed("ttyak is not released for this platform".into())) };
    let result = fetch(url.to_string()).await.and_then(|json| parse(&json, platform));
    let failed = result.as_ref().err().map(|e| format!("{e:#}"));
    let (next, found) = after_check(state, now, current, result);
    let checked = match (&found, failed) {
        (_, Some(e)) => Checked::Failed(e),
        (Some(f), None) => Checked::Out(f.latest.version.clone()),
        (None, None) => Checked::UpToDate,
    };
    (next, found.map(|f| Found { notify: false, desktop: false, ..f }), checked)
}

/// A client that sends ttyak's name and version as the User-Agent and nothing else (no cookies),
/// with `limits`' timeouts.
fn client(limits: impl Fn(slack_core::reqwest::ClientBuilder) -> slack_core::reqwest::ClientBuilder) -> Result<slack_core::reqwest::Client> {
    slack_core::api::http_client(|| limits(slack_core::reqwest::Client::builder().user_agent(concat!("ttyak/", env!("CARGO_PKG_VERSION")))))
}

/// `latest.json` at `url` (at most [`MAX_FEED_BYTES`], within `check_timeout_secs`): a GET with
/// ttyak's name and version as the User-Agent, nothing else.
pub async fn fetch(url: &str) -> Result<String> {
    let http = client(|b| b.timeout(Duration::from_secs(settings().check_timeout_secs)))?;
    bounded_text(http.get(url), MAX_FEED_BYTES).await.context("latest.json")
}

/// How long until whether a check is due is looked at again.
fn wait(settings: &crate::tui::settings::Updates, state: &State, now: u64) -> Duration {
    let until_due = if settings.check { state.last_check.saturating_add(settings.check_every_secs).saturating_sub(now) } else { LOOK_AGAIN_SECS };
    Duration::from_secs(until_due.clamp(60, LOOK_AGAIN_SECS))
}

/// Check in the background, the first time at once when due, then as it comes due (the settings
/// read again each time; a broken settings.toml means the defaults, as everywhere in ttyak), and at
/// once on a request on `asks` (Check now). Each newer version a check finds is sent on `reports`,
/// to be notified only once written down; a check asked for also says how it ended.
pub fn spawn(reports: tokio::sync::mpsc::UnboundedSender<Report>, asks: tokio::sync::mpsc::UnboundedReceiver<()>) {
    let Some(path) = state_path() else { return };
    let current = Version::parse(env!("CARGO_PKG_VERSION")).expect("the package version is semver");
    let url = std::env::var(URL_ENV).ok().filter(|u| !u.is_empty()).unwrap_or_else(|| URL.to_string());
    tokio::spawn(run(path, crate::install::platform(), url, current, reports, asks, |u: String| async move { fetch(&u).await }, settings));
}

/// [`spawn`]'s task, with `path` for `updates.json`, `fetch` for the GET and `settings` for
/// `[updates]`; it ends when the app is gone. All the requests that came before a check ends are
/// answered by that one check.
async fn run<F, Fut>(path: PathBuf, platform: Option<&str>, url: String, current: Version, reports: tokio::sync::mpsc::UnboundedSender<Report>, mut asks: tokio::sync::mpsc::UnboundedReceiver<()>, fetch: F, settings: impl Fn() -> crate::tui::settings::Updates)
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<String>>,
{
    // What this run last wrote (or tried to): a floor under what the file says.
    let mut kept = State::default();
    let mut asked = false;
    loop {
        let settings = settings();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        // Read each time: another ttyak (another profile) may have checked meanwhile.
        let mut state = with_floor(load_state(&path), &kept);
        let ran = if asked {
            let (next, newer, checked) = check_now(&state, now, &current, platform, &url, &fetch).await;
            // Pressed while it ran: answered by it.
            while asks.try_recv().is_ok() {}
            Some((next, newer, Some(checked)))
        } else {
            check(&settings, &state, now, &current, platform, &url, &fetch).await.map(|(next, newer)| (next, newer, None))
        };
        if let Some((next, newer, checked)) = ran {
            kept = next.clone();
            // Only once it is written: never notified twice.
            let saved = save_state(&path, &next);
            if let Err(e) = &saved {
                tracing::debug!("update check: {}: {e:#}", path.display());
            }
            if let Some(found) = newer {
                // Not written: no desktop notice (a later run would notify it again); the pill for
                // this run all the same (`kept` keeps a second one from this run).
                if reports.send(Report::Found(Found { desktop: found.desktop && saved.is_ok(), ..found })).is_err() {
                    return;
                }
            }
            if let Some(checked) = checked {
                if reports.send(Report::Checked(checked)).is_err() {
                    return;
                }
            }
            state = next;
        }
        asked = tokio::select! {
            () = tokio::time::sleep(wait(&settings, &state, now)) => false,
            ask = asks.recv() => match ask {
                Some(()) => {
                    while asks.try_recv().is_ok() {}
                    true
                }
                // The app is gone.
                None => return,
            },
        };
    }
}

/// The executable in the release archive (its top level).
const EXE: &str = if cfg!(windows) { "ttyak.exe" } else { "ttyak" };
/// The new executable in `dir`, beside the old one, while it is checked and put in place (this
/// process's own: another ttyak updating at the same time has another).
fn staged(dir: &Path) -> PathBuf {
    dir.join(format!(".ttyak-new-{}", std::process::id()))
}

/// `install.json`'s new text in `dir` while the new executable is put in place.
fn staged_record(dir: &Path) -> PathBuf {
    dir.join(format!(".install.json.new-{}", std::process::id()))
}
/// Where Windows keeps the running executable once the new one is in place (deleted at the next start).
#[cfg(windows)]
const ASIDE: &str = "ttyak.old.exe";
/// More than an executable ever unpacks to: an archive that says otherwise is not ttyak's.
const MAX_EXE_BYTES: u64 = 1 << 30;

/// How far a self-update has come, shared with the update screen, which can cancel it while it
/// downloads.
#[derive(Debug, Default)]
pub struct Progress {
    got: AtomicU64,
    /// The archive's size; 0 while not known.
    total: AtomicU64,
    stage: AtomicU8,
}

const DOWNLOADING: u8 = 0;
const INSTALLING: u8 = 1;
const CANCELLED: u8 = 2;

impl Progress {
    /// The bytes downloaded and the archive's size (None while not known).
    pub fn bytes(&self) -> (u64, Option<u64>) {
        let total = self.total.load(Ordering::Relaxed);
        (self.got.load(Ordering::Relaxed), (total > 0).then_some(total))
    }

    /// Whether the archive is in and the new ttyak is being checked and put in place (too late to
    /// cancel).
    pub fn installing(&self) -> bool {
        self.stage.load(Ordering::Acquire) == INSTALLING
    }

    /// Cancel the download; false when it is too late (see [`Progress::installing`]).
    pub fn cancel(&self) -> bool {
        match self.stage.compare_exchange(DOWNLOADING, CANCELLED, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => true,
            Err(now) => now == CANCELLED,
        }
    }

    /// From the download on to putting it in place, unless cancelled first.
    fn begin_install(&self) -> Result<()> {
        ensure!(self.stage.compare_exchange(DOWNLOADING, INSTALLING, Ordering::AcqRel, Ordering::Acquire).is_ok(), "cancelled");
        Ok(())
    }

    /// Whether the download was cancelled.
    pub fn cancelled(&self) -> bool {
        self.stage.load(Ordering::Acquire) == CANCELLED
    }
}

/// Files removed when dropped: every way out of a self-update, a cancel (the task dropped) too.
struct Leftovers(Vec<PathBuf>);

impl Drop for Leftovers {
    fn drop(&mut self) {
        for path in &self.0 {
            if std::fs::symlink_metadata(path).is_ok() {
                if let Err(e) = std::fs::remove_file(path) {
                    tracing::warn!("could not remove {}: {e}", path.display());
                }
            }
        }
    }
}

/// A new file at `path`, made by this run: a file (or a link) left there before is removed first,
/// never followed.
fn create_fresh(path: &Path) -> std::io::Result<std::fs::File> {
    if std::fs::symlink_metadata(path).is_ok() {
        std::fs::remove_file(path)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o700);
    }
    options.open(path)
}

/// `path` as people write it (without Windows' `\\?\`), for what a self-update says.
fn shown(path: &Path) -> String {
    crate::uninstall::plain(path).display().to_string()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The text `request` is answered with, at most `max` bytes.
async fn bounded_text(request: slack_core::reqwest::RequestBuilder, max: u64) -> Result<String> {
    let mut response = request.send().await?.error_for_status()?;
    let too_big = || format!("it is too big (more than {max} bytes)");
    ensure!(response.content_length().is_none_or(|n| n <= max), too_big());
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        body.extend_from_slice(&chunk);
        ensure!(body.len() as u64 <= max, too_big());
    }
    Ok(String::from_utf8(body)?)
}

/// The SHA256 that `sums` (a `SHA256SUMS` file) gives for the file named exactly `name`.
fn listed_sha256<'a>(sums: &'a str, name: &str) -> Option<&'a str> {
    sums.lines().find_map(|line| {
        let mut fields = line.trim_end_matches('\r').split_whitespace();
        let (sha, file) = (fields.next()?, fields.next()?);
        (file == name || file.strip_prefix('*') == Some(name)).then_some(sha)
    })
}

/// Download `latest`'s archive (only the release's own for that version, newer than this ttyak,
/// from [`DOWNLOAD_URL`] or [`DOWNLOAD_URL_ENV`]'s) into the folder of `install`'s executable, check it (its SHA256
/// against `latest.json`'s and against its line in the release's `SHA256SUMS`), unpack the
/// executable beside the old one and put it in place (Windows: the running one renamed aside),
/// then write the new version into `install.json`; the new executable's path. It never starts
/// it. Any failure, a cancel too, leaves the old executable as it was and the folder without the
/// files it made, and says what failed.
pub async fn self_update(latest: &Latest, install: &Install, progress: &Progress) -> Result<PathBuf> {
    self_update_from(latest, install, progress, &Release::this()?).await
}

/// The release folders' parent on GitHub.
pub const DOWNLOAD_URL: &str = "https://github.com/dmytrobsp/ttyak/releases/download";
/// Overrides [`DOWNLOAD_URL`] (a local server in tests), as for the install scripts.
pub const DOWNLOAD_URL_ENV: &str = "TTYAK_DOWNLOAD_URL";
/// More than a release archive ever is (13 MB now).
const MAX_ZIP_BYTES: u64 = 200 << 20;
/// More than a `SHA256SUMS` ever is.
const MAX_SUMS_BYTES: u64 = 64 << 10;

/// Where a self-update may download from, and how much it takes.
struct Release {
    /// The release folders' parent: `<base>/v<version>/…`.
    base: String,
    platform: &'static str,
    max_zip: u64,
    max_sums: u64,
    /// What the platform says of the executables' signatures.
    verifier: &'static dyn Verifier,
}

impl Release {
    /// GitHub's releases ([`DOWNLOAD_URL_ENV`] overrides them) for this build's platform.
    fn this() -> Result<Release> {
        let base = std::env::var(DOWNLOAD_URL_ENV).ok().filter(|u| !u.is_empty()).unwrap_or_else(|| DOWNLOAD_URL.to_string());
        let platform = crate::install::platform().context("ttyak is not released for this platform")?;
        Ok(Release { base: base.trim_end_matches('/').to_string(), platform, max_zip: MAX_ZIP_BYTES, max_sums: MAX_SUMS_BYTES, verifier: VERIFIER })
    }

    /// `version`'s archive for this platform and its `SHA256SUMS`.
    fn urls(&self, version: &Version) -> (String, String) {
        let folder = format!("{}/v{version}", self.base);
        (format!("{folder}/ttyak-{version}-{}.zip", self.platform), format!("{folder}/SHA256SUMS"))
    }
}

/// [`self_update`] from `release`.
async fn self_update_from(latest: &Latest, install: &Install, progress: &Progress, release: &Release) -> Result<PathBuf> {
    ensure!(install.method == Method::Script && install.writable, "ttyak can update itself only where the install script put it, in a folder it can write");
    let current = Version::parse(env!("CARGO_PKG_VERSION")).expect("the package version is semver");
    ensure!(newer(&current, latest), "ttyak {} is not newer than this one ({current}): nothing was downloaded", latest.version);
    // Only the release's own archive for this version: a feed naming anything else (another
    // host, version or platform) gets nothing downloaded.
    let (asset_url, sums_url) = release.urls(&latest.version);
    ensure!(latest.asset.url == asset_url, "latest.json names {} for ttyak {}, not the release's {asset_url}: nothing was downloaded", latest.asset.url, latest.version);
    let exe = install.exe.clone();
    let dir = exe.parent().filter(|d| !d.as_os_str().is_empty()).context("the executable's folder")?.to_path_buf();
    let download = dir.join(format!(".ttyak-update-{}.zip", std::process::id()));
    let leftovers = Leftovers(vec![download.clone(), staged(&dir), staged_record(&dir)]);
    let name = format!("ttyak-{}-{}.zip", latest.version, release.platform);
    // Connecting and every wait for data alike.
    let timeout = Duration::from_secs(settings().download_timeout_secs);
    let http = client(|b| b.connect_timeout(timeout).read_timeout(timeout))?;

    // The archive, its SHA256 taken as it comes.
    let sha256 = {
        use sha2::Digest;
        use std::io::Write;
        let mut file = create_fresh(&download).with_context(|| format!("could not write in {}", shown(&dir)))?;
        let mut response = http.get(&asset_url).send().await.and_then(|r| r.error_for_status()).with_context(|| format!("could not download {name}"))?;
        let too_big = || format!("{name} is too big to be ttyak's archive (more than {} MB)", release.max_zip >> 20);
        ensure!(response.content_length().is_none_or(|n| n <= release.max_zip), too_big());
        progress.total.store(response.content_length().unwrap_or(0), Ordering::Relaxed);
        let mut hasher = sha2::Sha256::new();
        let mut got = 0;
        while let Some(chunk) = response.chunk().await.with_context(|| format!("could not download {name}"))? {
            if progress.cancelled() {
                bail!("cancelled");
            }
            got += chunk.len() as u64;
            ensure!(got <= release.max_zip, too_big());
            hasher.update(&chunk);
            file.write_all(&chunk).with_context(|| format!("could not write {}", shown(&download)))?;
            progress.got.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        }
        file.sync_all().with_context(|| format!("could not write {}", shown(&download)))?;
        hex(&hasher.finalize())
    };
    ensure!(sha256.eq_ignore_ascii_case(latest.asset.sha256.trim()), "the download does not match the SHA256 in latest.json");
    let sums = bounded_text(http.get(&sums_url), release.max_sums).await.context("could not download the release's SHA256SUMS")?;
    match listed_sha256(&sums, &name) {
        Some(listed) if listed.eq_ignore_ascii_case(&sha256) => {}
        Some(_) => bail!("the download does not match the SHA256 in the release's SHA256SUMS"),
        None => bail!("the release's SHA256SUMS has no line for {name}"),
    }
    progress.begin_install()?;
    // Unpacking and moving files: off the async threads, and no longer cancelled.
    let version = latest.version.to_string();
    let verifier = release.verifier;
    tokio::task::spawn_blocking(move || {
        let leftovers = leftovers;
        install_from(&download, &exe, &version, verifier)?;
        drop(leftovers);
        Ok(exe)
    })
    .await
    .context("the update stopped")?
}

/// The executable from the archive `zip` put in place of `exe` (once `verifier` finds it signed as
/// `exe` is) and `version` written into the `install.json` beside it.
fn install_from(zip: &Path, exe: &Path, version: &str, verifier: &dyn Verifier) -> Result<()> {
    let dir = exe.parent().context("the executable's folder")?;
    let staged = staged(dir);
    unpack(zip, &staged)?;
    check_signer(verifier, exe, &staged)?;
    let record = dir.join(crate::install::RECORD_FILE);
    let new_record = crate::install::Record::read(dir, &crate::install::System).map(|r| crate::install::Record { version: version.to_string(), ..r });
    let staged_record = staged_record(dir);
    if let Some(r) = &new_record {
        use std::io::Write;
        let mut file = create_fresh(&staged_record).with_context(|| format!("could not write {}", shown(&staged_record)))?;
        file.write_all(format!("{}\n", serde_json::to_string(r)?).as_bytes()).and_then(|()| file.sync_all()).with_context(|| format!("could not write {}", shown(&staged_record)))?;
        // (The old file's mode kept.)
        if let Ok(old) = std::fs::metadata(&record) {
            std::fs::set_permissions(&staged_record, old.permissions()).with_context(|| format!("could not write {}", shown(&staged_record)))?;
        }
    }
    swap(&staged, exe)?;
    if new_record.is_some() {
        if let Err(e) = std::fs::rename(&staged_record, &record) {
            tracing::warn!("the new ttyak is in place, but {} still names the old version: {e}", record.display());
        }
    }
    Ok(())
}

/// The archive's top-level executable (exactly [`EXE`]; nothing else in it is looked at, so no
/// name can lead out of the folder) written to `to`, executable.
fn unpack(zip: &Path, to: &Path) -> Result<()> {
    use std::io::Read;
    let file = std::fs::File::open(zip).with_context(|| format!("could not read {}", shown(zip)))?;
    let mut archive = zip::ZipArchive::new(file).context("the download is not a zip archive")?;
    let index = (0..archive.len()).find(|&i| archive.by_index(i).is_ok_and(|e| e.is_file() && e.name().is_ok_and(|n| n == EXE))).with_context(|| format!("the archive has no {EXE}"))?;
    let mut entry = archive.by_index(index).with_context(|| format!("could not unpack {EXE}"))?;
    let mut out = create_fresh(to).with_context(|| format!("could not write {}", shown(to)))?;
    let copied = std::io::copy(&mut (&mut entry).take(MAX_EXE_BYTES + 1), &mut out).with_context(|| format!("could not unpack {EXE}"))?;
    ensure!(copied <= MAX_EXE_BYTES, "the archive's {EXE} is too big to be ttyak");
    out.sync_all().with_context(|| format!("could not write {}", shown(to)))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o755)).with_context(|| format!("could not make {} executable", shown(to)))?;
    }
    Ok(())
}

/// What the platform says of an executable's signature.
trait Verifier: Sync {
    /// The Team ID `exe` is signed with; None when it is not signed by a team (unsigned, ad hoc).
    fn team(&self, exe: &Path) -> Result<Option<String>>;
    /// Whether `exe`'s signature holds, strictly checked, and is a Developer ID of `team`.
    fn holds(&self, exe: &Path, team: &str) -> Result<bool>;
}

/// No signatures checked: a platform ttyak is not signed for here.
#[cfg(any(not(target_os = "macos"), test))]
struct NoSignatures;

#[cfg(any(not(target_os = "macos"), test))]
impl Verifier for NoSignatures {
    fn team(&self, _: &Path) -> Result<Option<String>> {
        Ok(None)
    }

    fn holds(&self, _: &Path, _: &str) -> Result<bool> {
        Ok(true)
    }
}

/// macOS's `codesign`.
#[cfg(target_os = "macos")]
struct Codesign;

#[cfg(target_os = "macos")]
const CODESIGN: &str = "/usr/bin/codesign";

#[cfg(target_os = "macos")]
impl Verifier for Codesign {
    fn team(&self, exe: &Path) -> Result<Option<String>> {
        let out = std::process::Command::new(CODESIGN).args(["-d", "--verbose=2"]).arg(exe).stdin(std::process::Stdio::null()).output().context("could not run codesign")?;
        // (What it finds goes to stderr.)
        let said = String::from_utf8_lossy(&out.stderr);
        if out.status.success() {
            team_identifier(&said).with_context(|| format!("codesign on {}", exe.display()))
        } else if said.contains("not signed at all") {
            Ok(None)
        } else {
            bail!("codesign could not read {}: {}", exe.display(), said.trim())
        }
    }

    fn holds(&self, exe: &Path, team: &str) -> Result<bool> {
        // Apple's requirement for Developer ID programs: Apple's anchor, the Developer ID
        // intermediate and leaf, and the team. (The same as release.yml's Sign (macOS) step.)
        let requirement = format!("=anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = \"{team}\"");
        let status = std::process::Command::new(CODESIGN)
            .args(["--verify", "--strict", "--test-requirement"])
            .arg(requirement)
            .arg(exe)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .context("could not run codesign")?;
        Ok(status.success())
    }
}

/// This platform's signatures.
#[cfg(target_os = "macos")]
const VERIFIER: &dyn Verifier = &Codesign;
#[cfg(not(target_os = "macos"))]
const VERIFIER: &dyn Verifier = &NoSignatures;

/// The `TeamIdentifier=` that `codesign -d --verbose=2` says; None when not set (unsigned, ad
/// hoc). A Developer ID without one, or one that is no Team ID, is an error, never "unsigned".
#[cfg(any(target_os = "macos", test))]
fn team_identifier(said: &str) -> Result<Option<String>> {
    let developer_id = said.lines().any(|l| l.starts_with("Authority=Developer ID"));
    match said.lines().find_map(|l| l.strip_prefix("TeamIdentifier=")).map(str::trim) {
        Some(team) if !team.is_empty() && team.len() <= 20 && team.bytes().all(|b| b.is_ascii_alphanumeric()) => Ok(Some(team.to_string())),
        None | Some("not set") if !developer_id => Ok(None),
        None | Some("not set") => bail!("a Developer ID signature without a Team ID"),
        Some(team) => bail!("a Team ID that is none: {team:?}"),
    }
}

/// `new` signed as `running` is: when `running` has a team's signature, `new` must carry the same
/// team's, and it must hold; an unsigned `running` (a build of one's own) checks nothing.
fn check_signer(verifier: &dyn Verifier, running: &Path, new: &Path) -> Result<()> {
    let Some(team) = verifier.team(running).context("could not read this ttyak's signature; the old one stays")? else { return Ok(()) };
    match verifier.team(new).context("could not read the new ttyak's signature; the old one stays")? {
        None => bail!("the new ttyak is not signed, but this one is (Team ID {team}); the old one stays"),
        Some(theirs) if theirs != team => bail!("the new ttyak is signed by Team ID {theirs}, not by this one's {team}; the old one stays"),
        Some(_) => ensure!(verifier.holds(new, &team).context("could not check the new ttyak's signature; the old one stays")?, "the new ttyak's signature does not hold as a Developer ID of Team ID {team}; the old one stays"),
    }
    Ok(())
}

/// Put `staged` in place of `exe`: renamed over it (a running ttyak keeps the old file).
#[cfg(not(windows))]
fn swap(staged: &Path, exe: &Path) -> Result<()> {
    std::fs::rename(staged, exe).with_context(|| format!("could not put the new ttyak in place of {}; the old one stays", exe.display()))
}

/// Put `staged` in place of `exe`: Windows keeps a running executable's file, but lets it be
/// renamed, so the old one goes aside first ([`ASIDE`], or a name of its own while an older one
/// there is still in use) and comes back when the new one cannot be moved in.
#[cfg(windows)]
fn swap(staged: &Path, exe: &Path) -> Result<()> {
    let dir = exe.parent().context("the executable's folder")?;
    let mut aside = dir.join(ASIDE);
    if std::fs::symlink_metadata(&aside).is_ok() && std::fs::remove_file(&aside).is_err() {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
        aside = dir.join(format!("ttyak.old-{:08x}.exe", nanos ^ std::process::id()));
    }
    std::fs::rename(exe, &aside).with_context(|| format!("could not move {} aside; the old one stays", shown(exe)))?;
    if let Err(e) = std::fs::rename(staged, exe) {
        return Err(match std::fs::rename(&aside, exe) {
            Ok(()) => anyhow::Error::new(e).context("could not put the new ttyak.exe in place; the old one stays"),
            Err(_) => anyhow::Error::new(e).context(format!("could not put the new ttyak.exe in place; the old one is now {}", shown(&aside))),
        });
    }
    // Gone at once unless it is running (this ttyak): then at the next start.
    let _ = std::fs::remove_file(&aside);
    Ok(())
}

/// Remove `ttyak.old.exe` and the other copies a self-update or install.ps1 renamed aside beside
/// the running executable, where they can go (Windows; nothing elsewhere).
pub fn remove_old_copies() {
    #[cfg(windows)]
    {
        let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) else { return };
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let name = entry.file_name();
            if name.to_str().is_some_and(crate::uninstall::is_old_copy) {
                // Still in use (another ttyak runs it): left for the next start.
                if let Err(e) = std::fs::remove_file(entry.path()) {
                    tracing::debug!("{}: {e}", entry.path().display());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::settings::Updates;

    const FEED: &str = r#"{"name":"ttyak","version":"0.10.2","date":"2026-11-01","notes":"https://github.com/dmytrobsp/ttyak/releases/tag/v0.10.2",
        "assets":{"x86_64-linux":{"url":"https://github.com/dmytrobsp/ttyak/releases/download/v0.10.2/ttyak-0.10.2-x86_64-linux.zip","sha256":"ab12"},
                  "aarch64-macos":{"url":"https://example.invalid/mac.zip","sha256":"cd34"}}}"#;

    fn latest(v: &str) -> Latest {
        Latest { version: Version::parse(v).unwrap(), date: "2026-11-01".into(), asset: Asset { url: "u".into(), sha256: "s".into() } }
    }

    /// The version a check's result notifies, if any.
    fn notified(found: Option<Found>) -> Option<Latest> {
        found.filter(|f| f.notify).map(|f| f.latest)
    }

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn parses_latest() {
        let l = parse(FEED, "x86_64-linux").unwrap();
        assert_eq!(l.version, v("0.10.2"));
        assert_eq!(l.date, "2026-11-01");
        assert_eq!(page_at(PAGES_URL, &l.version), "https://github.com/dmytrobsp/ttyak/releases/tag/v0.10.2");
        assert_eq!(l.asset, Asset { url: "https://github.com/dmytrobsp/ttyak/releases/download/v0.10.2/ttyak-0.10.2-x86_64-linux.zip".into(), sha256: "ab12".into() });
        assert_eq!(parse(FEED, "aarch64-macos").unwrap().asset.sha256, "cd34");
    }

    /// The feed packaging/render.py writes (packaging/test_render.py keeps this copy equal to its output): every
    /// platform's archive is the very one a self-update would take from the release.
    #[test]
    fn parses_the_rendered_feed() {
        let rendered = include_str!("../testdata/latest.json");
        for platform in ["x86_64-linux", "aarch64-linux", "x86_64-macos", "aarch64-macos", "x86_64-windows"] {
            let l = parse(rendered, platform).unwrap();
            assert_eq!((&l.version, l.date.as_str()), (&v("0.11.0"), "2026-11-01"));
            // The page ttyak builds from the version is the one render.py names.
            let notes = serde_json::from_str::<serde_json::Value>(rendered).unwrap()["notes"].as_str().unwrap().to_string();
            assert_eq!(page_at(PAGES_URL, &l.version), notes);
            let release = Release { base: DOWNLOAD_URL.into(), platform, max_zip: MAX_ZIP_BYTES, max_sums: MAX_SUMS_BYTES, verifier: &NoSignatures };
            assert_eq!(l.asset.url, release.urls(&l.version).0, "{platform}");
            assert!(l.asset.sha256.len() == 64 && l.asset.sha256.bytes().all(|b| b.is_ascii_hexdigit()), "{platform}");
        }
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
    fn build_metadata_is_not_newer() {
        let ci = latest("0.10.0+ci.1");
        assert!(!newer(&v("0.10.0"), &ci));
        assert!(!newer(&v("0.10.0+ci.1"), &latest("0.10.0")));
        assert!(!should_notify(&State { last_check: 0, notified: Some("0.10.0".into()) }, &ci), "the same version notified");
        assert!(newer(&v("0.10.0+ci.1"), &latest("0.10.1")));
    }

    #[test]
    fn a_failed_save_still_means_once() {
        // updates.json could not be written: nothing on disk, the last check kept in memory.
        let kept = State { last_check: 500, notified: Some("9.9.9".into()) };
        let state = with_floor(State::default(), &kept);
        assert_eq!(state, kept);
        assert!(!due(&state, 600, 86_400), "not again within the day");
        assert!(!should_notify(&state, &latest("9.9.9")), "never a second notice");
        // Another ttyak wrote a later check meanwhile: the later of each wins.
        let disk = State { last_check: 900, notified: Some("9.9.10".into()) };
        assert_eq!(with_floor(disk.clone(), &kept), disk);
        assert_eq!(with_floor(State { last_check: 900, notified: None }, &kept), State { last_check: 900, notified: Some("9.9.9".into()) });
        assert_eq!(with_floor(State { last_check: 100, notified: Some("0.1.0".into()) }, &kept), kept);
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
        /// A temporary folder, removed with what it holds even when an assertion fails.
        struct Temp(PathBuf);
        impl Drop for Temp {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let dir = Temp(std::env::temp_dir().join(format!("ttyak-test-updates-{}", std::process::id())));
        let path = dir.0.join("updates.json");
        let (next, shown) = after_check(&State::default(), 50, &v("0.10.0"), Ok(latest("0.10.1")));
        assert_eq!(notified(shown), Some(latest("0.10.1")));
        save_state(&path, &next).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&text).unwrap(), serde_json::json!({"last_check": 50, "notified": "0.10.1"}));
        let (again, shown) = after_check(&load_state(&path), 90_000, &v("0.10.0"), Ok(latest("0.10.1")));
        assert_eq!((again.last_check, again.notified.as_deref()), (90_000, Some("0.10.1")));
        assert_eq!(shown, Some(Found { latest: latest("0.10.1"), notify: false, desktop: false }), "the next run knows of it and says nothing");
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(load_state(&path), State::default(), "a broken file starts afresh");
        std::fs::remove_dir_all(&dir.0).unwrap();
        assert_eq!(load_state(&path), State::default(), "no file yet");
    }

    #[test]
    fn state_written_before_notifying() {
        let old = State { last_check: 10, notified: Some("0.10.1".into()) };
        let (next, shown) = after_check(&old, 500, &v("0.10.0"), Ok(latest("0.10.2")));
        assert_eq!(notified(shown), Some(latest("0.10.2")));
        assert_eq!(next, State { last_check: 500, notified: Some("0.10.2".into()) }, "the state to write names what is shown");
        let (next, shown) = after_check(&old, 600, &v("0.10.0"), Err(anyhow::anyhow!("offline")));
        assert_eq!((next, shown), (State { last_check: 600, notified: Some("0.10.1".into()) }, None), "a failed check counts as the day's");
        let (next, shown) = after_check(&old, 700, &v("0.10.2"), Ok(latest("0.10.2")));
        assert_eq!((next.notified.as_deref(), shown), (Some("0.10.1"), None), "not newer than this ttyak");
        let (next, shown) = after_check(&old, 800, &v("0.10.0"), Ok(latest("0.10.1")));
        assert_eq!((next.notified.as_deref(), shown), (Some("0.10.1"), Some(Found { latest: latest("0.10.1"), notify: false, desktop: false })), "notified before: known, not notified again");
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
        assert_eq!((next.last_check, notified(shown).map(|l| l.version)), (1_000_000, Some(v("0.10.2"))));
        assert_eq!(asked.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn check_now_never_pops_a_notice() {
        let fetch = |_url: String| async { Ok(FEED.to_string()) };
        let now = v("0.10.0");
        // Not due and never notified: checked all the same, known, never to notify; seen on the
        // page, it counts as notified (no notice for it later).
        let recent = State { last_check: 999_000, notified: None };
        let (next, found, checked) = check_now(&recent, 1_000_000, &now, Some("x86_64-linux"), URL, fetch).await;
        assert_eq!(checked, Checked::Out(v("0.10.2")));
        assert_eq!(found.map(|f| (f.latest.version, f.notify)), Some((v("0.10.2"), false)));
        assert_eq!(next, State { last_check: 1_000_000, notified: Some("0.10.2".into()) });
        let (_, later) = after_check(&next, 2_000_000, &now, parse(FEED, "x86_64-linux"));
        assert!(later.is_some_and(|f| !f.notify), "the next check says nothing");
        // Notified before: still never to notify.
        let (_, found, _) = check_now(&State { last_check: 0, notified: Some("0.10.1".into()) }, 5, &now, Some("x86_64-linux"), URL, fetch).await;
        assert!(found.is_some_and(|f| !f.notify));
        // Up to date; no answer; no release for this platform (nothing fetched).
        let (_, found, checked) = check_now(&recent, 1_000_000, &v("0.10.2"), Some("x86_64-linux"), URL, fetch).await;
        assert_eq!((found, checked), (None, Checked::UpToDate));
        let (next, found, checked) = check_now(&recent, 1_000_000, &now, Some("x86_64-linux"), URL, |_| async { Err(anyhow::anyhow!("offline")) }).await;
        assert_eq!((next.last_check, found, checked), (1_000_000, None, Checked::Failed("offline".into())));
        let (next, found, checked) = check_now(&recent, 1_000_000, &now, None, URL, |_| async { panic!("fetched") }).await;
        assert_eq!((next, found, checked), (recent.clone(), None, Checked::Failed("ttyak is not released for this platform".into())));
    }

    #[test]
    fn feed_text_is_sanitised() {
        // ESC sequences (clear the screen, a hidden link), BEL, a C1 CSI, bidi overrides and isolates,
        // zero-width characters and a line separator in the text shown.
        let feed = r#"{"version":"9.9.9","date":"2026-11-01\u001b[2J\u202e","notes":"https://x.invalid/\u001b]8;;https://evil.invalid\u0007notes\u009b31m\u2066\u2069\u200b\u200f\u2028\u2029\ufeff",
            "assets":{"x86_64-linux":{"url":"https://x.invalid/a.zip","sha256":"ab12"}}}"#;
        let l = parse(feed, "x86_64-linux").unwrap();
        assert_eq!(l.date, "2026-11-01[2J");
        assert!(!format!("{l:?}").contains("evil"), "the feed's link is not kept: {l:?}");
        assert_eq!(printable("a\u{1b}b\u{7f}c\u{85}d\u{9f}e\tf\u{202a}\u{202d}\u{2067}\u{200e}\u{ad} ü 🎉"), "abcdef ü 🎉", "C0, DEL, C1 and format characters go; the rest stays");
        // In what is downloaded and checked: refused, never cleaned up.
        for (url, sha) in [("https://x.invalid/a.zip\\u0000", "ab12"), ("https://x.invalid/a.zip", "ab12\\r\\n"), ("https://x.invalid/\\u001b[2Ja.zip", "ab12"), ("https://x.invalid/a.zip", "ab\\u202e12")] {
            let feed = format!(r#"{{"version":"9.9.9","date":"","notes":"","assets":{{"x86_64-linux":{{"url":"{url}","sha256":"{sha}"}}}}}}"#);
            let e = parse(&feed, "x86_64-linux").unwrap_err();
            assert!(format!("{e:#}").contains("control character"), "{url} {sha}: {e:#}");
        }
    }

    #[test]
    fn hand_edited_seconds_are_floored() {
        let read = |every, check, download| sane(Updates { check: true, check_every_secs: every, check_timeout_secs: check, download_timeout_secs: download });
        let least = read(0, 0, 0);
        assert_eq!((least.check_every_secs, least.check_timeout_secs, least.download_timeout_secs), (MIN_CHECK_EVERY_SECS, MIN_CHECK_TIMEOUT_SECS, MIN_DOWNLOAD_TIMEOUT_SECS));
        assert_eq!((MIN_CHECK_EVERY_SECS, MIN_CHECK_TIMEOUT_SECS, MIN_DOWNLOAD_TIMEOUT_SECS), (3600, 5, 10), "the shortest F6 offers");
        let max = read(u64::MAX, u64::MAX, u64::MAX);
        assert_eq!((max.check_every_secs, max.check_timeout_secs, max.download_timeout_secs), (u64::MAX, MAX_CHECK_TIMEOUT_SECS, MAX_DOWNLOAD_TIMEOUT_SECS));
        assert_eq!(read(86_400, 10, 30), Updates::default(), "the defaults as they are");
        let s = State { last_check: 1_000, notified: None };
        assert_eq!(wait(&max, &s, 1_000), Duration::from_secs(3_600), "no overflow: at most an hour");
        assert_eq!(wait(&least, &s, 1_000), Duration::from_secs(3_600), "0: an hour, never a tight loop");
        assert!(!due(&s, 2_000, u64::MAX) && due(&s, 999, u64::MAX));
    }

    #[tokio::test]
    async fn queued_check_nows_are_answered_by_one_check() {
        // (Removed even when an assertion fails.)
        let dir = Temp::new("asks");
        let (reports_tx, mut reports) = tokio::sync::mpsc::unbounded_channel();
        let (asks_tx, asks) = tokio::sync::mpsc::unbounded_channel();
        let fetched = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        // Two presses before it runs, two more while it fetches.
        asks_tx.send(()).unwrap();
        asks_tx.send(()).unwrap();
        let fetch = {
            // (Weak: the channel still closes when the test lets go of it.)
            let (fetched, asks_tx) = (fetched.clone(), asks_tx.downgrade());
            move |_url: String| {
                fetched.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if let Some(asks_tx) = asks_tx.upgrade() {
                    let _ = asks_tx.send(());
                    let _ = asks_tx.send(());
                }
                async { Ok(FEED.to_string()) }
            }
        };
        let off = || Updates { check: false, ..Default::default() };
        let task = tokio::spawn(run(dir.0.join("updates.json"), Some("x86_64-linux"), "http://127.0.0.1:9/latest.json".into(), v("0.10.0"), reports_tx, asks, fetch, off));
        async fn next(r: &mut tokio::sync::mpsc::UnboundedReceiver<Report>) -> Result<Option<Report>, tokio::time::error::Elapsed> {
            tokio::time::timeout(Duration::from_secs(5), r.recv()).await
        }
        assert!(matches!(next(&mut reports).await, Ok(Some(Report::Found(Found { notify: false, .. })))));
        assert!(matches!(next(&mut reports).await, Ok(Some(Report::Checked(Checked::Out(_))))));
        assert!(tokio::time::timeout(Duration::from_millis(300), reports.recv()).await.is_err(), "no second check");
        assert_eq!(fetched.load(std::sync::atomic::Ordering::SeqCst), 1);
        // A press after it: checked again.
        asks_tx.send(()).unwrap();
        assert!(matches!(next(&mut reports).await, Ok(Some(Report::Found(_)))));
        drop(asks_tx);
        tokio::time::timeout(Duration::from_secs(5), task).await.unwrap().unwrap();
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

    #[tokio::test]
    async fn the_feed_is_read_up_to_64_kb() {
        let at_most = vec![b' '; 64 << 10];
        let more = vec![b' '; (64 << 10) + 1];
        let (base, _) = serve(vec![("/fits".into(), at_most.clone()), ("/big".into(), more.clone()), ("/unsized".into(), more)], Some(("/unsized".into(), Quirk::NoLength))).await;
        assert_eq!(fetch(&format!("{base}/fits")).await.unwrap().len(), at_most.len());
        for path in ["/big", "/unsized"] {
            let e = fetch(&format!("{base}{path}")).await.unwrap_err();
            assert!(format!("{e:#}").contains("too big"), "{path}: {e:#}");
        }
    }

    /// Windows' `\\?\` never shows in what a self-update says.
    #[test]
    fn paths_in_errors_are_plain() {
        let temp = Temp::new("plain");
        let e = unpack(Path::new(r"\\?\C:\ttyak-nowhere\update.zip"), &temp.0.join("bin/staged")).unwrap_err();
        let said = format!("{e:#}");
        assert!(said.contains(r"C:\ttyak-nowhere\update.zip") && !said.contains(r"\\?\"), "{said}");
    }

    #[tokio::test]
    async fn a_failed_save_drops_only_the_desktop_notice() {
        let temp = Temp::new("unsaved");
        // updates.json cannot be written: its folder would be a file.
        std::fs::write(temp.0.join("file"), b"").unwrap();
        let (reports_tx, mut reports) = tokio::sync::mpsc::unbounded_channel();
        let (asks_tx, asks) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(run(temp.0.join("file/updates.json"), Some("x86_64-linux"), "http://127.0.0.1:9/latest.json".into(), v("0.10.0"), reports_tx, asks, |_url: String| async { Ok(FEED.to_string()) }, Updates::default));
        let found = tokio::time::timeout(Duration::from_secs(5), reports.recv()).await.unwrap();
        let Some(Report::Found(found)) = found else { panic!("{found:?}") };
        assert_eq!(found.latest.version, v("0.10.2"));
        assert!(found.notify, "the status line's pill for this run");
        assert!(!found.desktop, "no desktop notice: a later run would not know of it");
        drop(asks_tx);
        tokio::time::timeout(Duration::from_secs(5), task).await.unwrap().unwrap();
        // Written: both.
        let (reports_tx, mut reports) = tokio::sync::mpsc::unbounded_channel();
        let (asks_tx, asks) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(run(temp.0.join("updates.json"), Some("x86_64-linux"), "http://127.0.0.1:9/latest.json".into(), v("0.10.0"), reports_tx, asks, |_url: String| async { Ok(FEED.to_string()) }, Updates::default));
        let found = tokio::time::timeout(Duration::from_secs(5), reports.recv()).await.unwrap();
        assert!(matches!(found, Some(Report::Found(Found { notify: true, desktop: true, .. }))), "{found:?}");
        drop(asks_tx);
        tokio::time::timeout(Duration::from_secs(5), task).await.unwrap().unwrap();
    }

    #[test]
    fn waits_until_due_at_most_an_hour() {
        let s = State { last_check: 1_000, notified: None };
        let every = |secs| Updates { check: true, check_every_secs: secs, ..Default::default() };
        assert_eq!(wait(&every(86_400), &s, 1_000), Duration::from_secs(3_600));
        assert_eq!(wait(&every(600), &s, 1_000), Duration::from_secs(600));
        assert_eq!(wait(&every(600), &s, 1_300), Duration::from_secs(300));
        assert_eq!(wait(&every(0), &s, 1_000), Duration::from_secs(60), "never in a tight loop");
        assert_eq!(wait(&Updates { check: false, ..every(0) }, &s, 1_000), Duration::from_secs(3_600), "off: looked at hourly");
    }

    /// A folder of its own under the temp folder, removed with what it holds even when an
    /// assertion fails.
    struct Temp(PathBuf);

    impl Temp {
        fn new(test: &str) -> Temp {
            let dir = std::env::temp_dir().join(format!("ttyak-test-{test}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("bin")).unwrap();
            Temp(dir)
        }

        /// What the install folder holds, sorted.
        fn listing(&self) -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(self.0.join("bin")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
            names.sort();
            names
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::Digest;
        sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    /// A zip of `entries` (deflated, as the release archives are).
    fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, data) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    /// How the fixture server answers for one path.
    #[derive(Clone, Copy, PartialEq)]
    enum Quirk {
        /// The first 64 KB of a promised 50 MB, then nothing more.
        Stall,
        /// The body without a length (read until the connection closes).
        NoLength,
        /// A length of 300 MB, and then only the body.
        ClaimsHuge,
    }

    /// Serves `files` (path, body) on a local port, 404 for any other path, `quirk`'s path as it
    /// says. The address, and how many requests came.
    async fn serve(files: Vec<(String, Vec<u8>)>, quirk: Option<(String, Quirk)>) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let _ = rustls::crypto::ring::default_provider().install_default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let files = std::sync::Arc::new(files);
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = asked.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let (files, quirk, counter) = (files.clone(), quirk.clone(), counter.clone());
                tokio::spawn(async move {
                    let mut req = Vec::new();
                    let mut buf = [0u8; 4096];
                    while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                        match sock.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => req.extend_from_slice(&buf[..n]),
                        }
                    }
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let path = String::from_utf8_lossy(&req).split(' ').nth(1).unwrap_or("").to_string();
                    let quirk = quirk.filter(|(p, _)| *p == path).map(|(_, q)| q);
                    if quirk == Some(Quirk::Stall) {
                        let _ = sock.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 50000000\r\n\r\n").await;
                        let _ = sock.write_all(&[7u8; 65536]).await;
                        tokio::time::sleep(Duration::from_secs(600)).await;
                        return;
                    }
                    let reply = match files.iter().find(|(p, _)| *p == path) {
                        Some((_, body)) => {
                            let length = match quirk {
                                Some(Quirk::NoLength) => String::new(),
                                Some(Quirk::ClaimsHuge) => "content-length: 300000000\r\n".into(),
                                _ => format!("content-length: {}\r\n", body.len()),
                            };
                            [format!("HTTP/1.1 200 OK\r\n{length}connection: close\r\n\r\n").into_bytes(), body.clone()].concat()
                        }
                        None => b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec(),
                    };
                    let _ = sock.write_all(&reply).await;
                });
            }
        });
        (base, asked)
    }

    /// A fixture release: its address (the release folders' parent) and how many requests it had.
    struct Fixture {
        base: String,
        asked: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Fixture {
        /// [`self_update`] from this release, for x86_64-linux, with the real size limits.
        async fn update(&self, latest: &Latest, install: &Install, progress: &Progress) -> Result<PathBuf> {
            self.update_with(latest, install, progress, MAX_ZIP_BYTES, MAX_SUMS_BYTES).await
        }

        async fn update_with(&self, latest: &Latest, install: &Install, progress: &Progress, max_zip: u64, max_sums: u64) -> Result<PathBuf> {
            self_update_from(latest, install, progress, &Release { base: self.base.clone(), platform: "x86_64-linux", max_zip, max_sums, verifier: &NoSignatures }).await
        }

        /// [`Fixture::update`] with signatures as `verifier` says.
        async fn update_verified(&self, latest: &Latest, install: &Install, progress: &Progress, verifier: &'static dyn Verifier) -> Result<PathBuf> {
            self_update_from(latest, install, progress, &Release { base: self.base.clone(), platform: "x86_64-linux", max_zip: MAX_ZIP_BYTES, max_sums: MAX_SUMS_BYTES, verifier }).await
        }

        fn requests(&self) -> usize {
            self.asked.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    const ASSET: &str = "ttyak-9.9.9-x86_64-linux.zip";

    /// A script install of an old ttyak in `<temp>/bin`, and the release of 9.9.9 whose archive is
    /// `zip`, with `sums` as its SHA256SUMS (None: none at all), `sha256` in latest.json and the
    /// archive's `quirk`.
    async fn release(temp: &Temp, zip: Vec<u8>, sums: Option<String>, sha256: Option<String>, quirk: Option<Quirk>) -> (Install, Latest, Fixture) {
        let dir = temp.0.join("bin");
        let exe = dir.join(EXE);
        std::fs::write(&exe, b"old ttyak").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let record = serde_json::json!({"method": "script", "version": "0.9.9", "dir": dir});
        std::fs::write(dir.join("install.json"), format!("{record}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.join("install.json"), std::fs::Permissions::from_mode(0o640)).unwrap();
        }
        let sha = sha256.unwrap_or_else(|| sha256_hex(&zip));
        let mut files = vec![(format!("/v9.9.9/{ASSET}"), zip)];
        files.extend(sums.map(|s| ("/v9.9.9/SHA256SUMS".to_string(), s.into_bytes())));
        let (base, asked) = serve(files, quirk.map(|q| (format!("/v9.9.9/{ASSET}"), q))).await;
        let latest = Latest { version: v("9.9.9"), date: "2026-11-01".into(), asset: Asset { url: format!("{base}/v9.9.9/{ASSET}"), sha256: sha } };
        (Install { method: Method::Script, exe, writable: true }, latest, Fixture { base, asked })
    }

    /// `SHA256SUMS` as the release workflow writes it, `zip`'s line among the others.
    fn sums_for(zip: &[u8]) -> String {
        format!("{}  ttyak-9.9.9-x86_64-windows.zip\n{}  {ASSET}\n", "ab".repeat(32), sha256_hex(zip))
    }

    fn version_recorded(temp: &Temp) -> String {
        let text = std::fs::read_to_string(temp.0.join("bin/install.json")).unwrap();
        serde_json::from_str::<serde_json::Value>(&text).unwrap()["version"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn self_update_swaps_and_keeps_mode() {
        let temp = Temp::new("self-update");
        // Other entries, one trying to climb out and one in a folder: only the top-level executable is taken.
        let zip = archive(&[("README.md", b"read me"), ("../ttyak", b"climbed out"), ("sub/ttyak", b"nested"), (EXE, b"new ttyak"), ("LICENSE", b"MIT")]);
        // Windows line ends and a binary-mode `*` are read too.
        let sums = format!("{}  ttyak-9.9.9-aarch64-linux.zip\r\n{} *{ASSET}\r\n", "cd".repeat(32), sha256_hex(&zip));
        let (install, latest, fixture) = release(&temp, zip.clone(), Some(sums), None, None).await;
        let dir = temp.0.join("bin");
        // A stale staged file that is a link elsewhere: replaced, never followed.
        #[cfg(unix)]
        {
            std::fs::write(temp.0.join("elsewhere"), b"keep me").unwrap();
            std::os::unix::fs::symlink(temp.0.join("elsewhere"), staged(&dir)).unwrap();
        }
        let progress = Progress::default();
        let exe = fixture.update(&latest, &install, &progress).await.unwrap();
        assert_eq!(exe, install.exe);
        assert_eq!(std::fs::read(&exe).unwrap(), b"new ttyak");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // The new executable is 755, whatever the old one's mode was (700 here).
            assert_eq!(std::fs::metadata(&exe).unwrap().permissions().mode() & 0o777, 0o755);
            assert_eq!(std::fs::read(temp.0.join("elsewhere")).unwrap(), b"keep me", "the link was not followed");
        }
        assert_eq!(version_recorded(&temp), "9.9.9");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(dir.join("install.json")).unwrap().permissions().mode() & 0o777, 0o640, "install.json keeps its mode");
        }
        let record: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("install.json")).unwrap()).unwrap();
        assert_eq!((record["method"].as_str(), record["dir"].as_str()), (Some("script"), dir.to_str()), "the method and folder kept");
        assert_eq!(temp.listing(), ["install.json", EXE], "no download, staged file or other entry left");
        assert!(!temp.0.join("ttyak").exists() && !dir.join("sub").exists());
        assert_eq!(progress.bytes(), (zip.len() as u64, Some(zip.len() as u64)));
        assert!(progress.installing() && !progress.cancel(), "too late to cancel once done");
    }

    #[tokio::test]
    async fn sha_mismatch_leaves_the_old_binary() {
        let temp = Temp::new("sha-mismatch");
        let zip = archive(&[(EXE, b"new ttyak")]);
        let (install, latest, fixture) = release(&temp, zip.clone(), Some(sums_for(&zip)), Some("00".repeat(32)), None).await;
        let e = fixture.update(&latest, &install, &Progress::default()).await.unwrap_err();
        assert!(format!("{e:#}").contains("SHA256"), "{e:#}");
        assert_eq!(std::fs::read(&install.exe).unwrap(), b"old ttyak");
        assert_eq!((temp.listing(), version_recorded(&temp)), (vec!["install.json".to_string(), EXE.into()], "0.9.9".into()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&install.exe).unwrap().permissions().mode() & 0o777, 0o700);
        }
    }

    #[tokio::test]
    async fn sums_mismatch_leaves_the_old_binary() {
        let zip = archive(&[(EXE, b"new ttyak")]);
        let other = format!("{}  {ASSET}\n", "ef".repeat(32));
        let unnamed = format!("{}  ./{ASSET}\n{}  {ASSET}.old\n", sha256_hex(&zip), sha256_hex(&zip));
        for (case, sums, says) in [("another SHA256", Some(other), "SHA256SUMS"), ("no line of its exact name", Some(unnamed), "SHA256SUMS"), ("no SHA256SUMS", None, "SHA256SUMS")] {
            let temp = Temp::new("sums-mismatch");
            let (install, latest, fixture) = release(&temp, zip.clone(), sums, None, None).await;
            let e = fixture.update(&latest, &install, &Progress::default()).await.unwrap_err();
            assert!(format!("{e:#}").contains(says), "{case}: {e:#}");
            assert_eq!(std::fs::read(&install.exe).unwrap(), b"old ttyak", "{case}");
            assert_eq!((temp.listing(), version_recorded(&temp)), (vec!["install.json".to_string(), EXE.into()], "0.9.9".into()), "{case}");
        }
    }

    #[tokio::test]
    async fn an_archive_without_ttyak_leaves_the_old_binary() {
        let temp = Temp::new("no-exe");
        let zip = archive(&[("README.md", b"read me"), ("bin/ttyak", b"nested")]);
        let (install, latest, fixture) = release(&temp, zip.clone(), Some(sums_for(&zip)), None, None).await;
        let e = fixture.update(&latest, &install, &Progress::default()).await.unwrap_err();
        assert!(format!("{e:#}").contains(EXE), "{e:#}");
        assert_eq!((std::fs::read(&install.exe).unwrap(), temp.listing()), (b"old ttyak".to_vec(), vec!["install.json".to_string(), EXE.into()]));
    }

    #[tokio::test]
    async fn only_a_writable_script_install_updates_itself() {
        let temp = Temp::new("not-ours");
        let zip = archive(&[(EXE, b"new ttyak")]);
        let (install, latest, fixture) = release(&temp, zip.clone(), Some(sums_for(&zip)), None, None).await;
        for other in [Install { writable: false, ..install.clone() }, Install { method: Method::Homebrew, ..install.clone() }] {
            assert!(fixture.update(&latest, &other, &Progress::default()).await.is_err(), "{other:?}");
        }
        assert_eq!((std::fs::read(&install.exe).unwrap(), temp.listing()), (b"old ttyak".to_vec(), vec!["install.json".to_string(), EXE.into()]));
    }

    #[tokio::test]
    async fn a_cancel_removes_the_download() {
        let temp = Temp::new("cancel");
        let zip = archive(&[(EXE, b"new ttyak")]);
        let (install, latest, fixture) = release(&temp, zip.clone(), Some(sums_for(&zip)), None, Some(Quirk::Stall)).await;
        let progress = std::sync::Arc::new(Progress::default());
        let task = {
            let progress = progress.clone();
            tokio::spawn(async move { fixture.update(&latest, &install, &progress).await })
        };
        let started = std::time::Instant::now();
        while progress.bytes().0 < 65536 {
            assert!(started.elapsed() < Duration::from_secs(10), "the download never started");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(progress.bytes().1, Some(50_000_000));
        assert!(temp.listing().iter().any(|n| n.starts_with(".ttyak-update-")), "{:?}", temp.listing());
        assert!(progress.cancel() && !progress.installing());
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(temp.listing(), ["install.json", EXE]);
        assert_eq!(std::fs::read(temp.0.join("bin").join(EXE)).unwrap(), b"old ttyak");
    }

    #[tokio::test]
    async fn only_the_release_s_own_archive_is_downloaded() {
        let zip = archive(&[(EXE, b"new ttyak")]);
        for (case, url) in [
            ("another host", "https://example.invalid/v9.9.9/ttyak-9.9.9-x86_64-linux.zip"),
            ("another version's archive", "{base}/v0.9.0/ttyak-0.9.0-x86_64-linux.zip"),
            ("another platform's", "{base}/v9.9.9/ttyak-9.9.9-x86_64-windows.zip"),
            ("another scheme", "{http}/v9.9.9/ttyak-9.9.9-x86_64-linux.zip"),
            ("a path trick", "{base}/v9.9.9/../v0.9.0/ttyak-9.9.9-x86_64-linux.zip"),
            ("a query", "{base}/v9.9.9/ttyak-9.9.9-x86_64-linux.zip?v=1"),
        ] {
            let temp = Temp::new("pinned");
            let (install, mut latest, fixture) = release(&temp, zip.clone(), Some(sums_for(&zip)), None, None).await;
            latest.asset.url = url.replace("{base}", &fixture.base).replace("{http}", &fixture.base.replacen("http://", "ftp://", 1));
            let e = fixture.update(&latest, &install, &Progress::default()).await.unwrap_err();
            assert!(format!("{e:#}").contains("not the release's"), "{case}: {e:#}");
            assert_eq!(fixture.requests(), 0, "{case}: nothing downloaded");
            assert_eq!((std::fs::read(&install.exe).unwrap(), temp.listing()), (b"old ttyak".to_vec(), vec!["install.json".to_string(), EXE.into()]), "{case}");
        }
        // An https release refuses the same address over http.
        let github = Release { base: "https://github.com/dmytrobsp/ttyak/releases/download".into(), platform: "x86_64-linux", max_zip: MAX_ZIP_BYTES, max_sums: MAX_SUMS_BYTES, verifier: &NoSignatures };
        assert_eq!(github.urls(&v("9.9.9")), ("https://github.com/dmytrobsp/ttyak/releases/download/v9.9.9/ttyak-9.9.9-x86_64-linux.zip".to_string(), "https://github.com/dmytrobsp/ttyak/releases/download/v9.9.9/SHA256SUMS".to_string()));
        let mut plain_http = latest("9.9.9");
        plain_http.asset.url = "http://github.com/dmytrobsp/ttyak/releases/download/v9.9.9/ttyak-9.9.9-x86_64-linux.zip".into();
        let temp = Temp::new("pinned-http");
        let install = Install { method: Method::Script, exe: temp.0.join("bin").join(EXE), writable: true };
        std::fs::write(&install.exe, b"old ttyak").unwrap();
        let e = self_update_from(&plain_http, &install, &Progress::default(), &github).await.unwrap_err();
        assert!(format!("{e:#}").contains("not the release's"), "{e:#}");
        // An older release, its own archive and all: never put in (no downgrade).
        let temp = Temp::new("pinned-old");
        let (install, mut latest, fixture) = release(&temp, zip.clone(), Some(sums_for(&zip)), None, None).await;
        latest.version = v("0.1.0");
        latest.asset.url = format!("{}/v0.1.0/ttyak-0.1.0-x86_64-linux.zip", fixture.base);
        let e = fixture.update(&latest, &install, &Progress::default()).await.unwrap_err();
        assert!(format!("{e:#}").contains("not newer"), "{e:#}");
        assert_eq!(fixture.requests(), 0);
    }

    #[tokio::test]
    async fn too_big_is_refused() {
        let zip = archive(&[(EXE, b"new ttyak")]);
        let limit = zip.len() as u64 - 1;
        for (case, quirk) in [("a length above the limit", Some(Quirk::ClaimsHuge)), ("more bytes than the limit, no length", Some(Quirk::NoLength)), ("a length just above the limit", None)] {
            let temp = Temp::new("too-big");
            let (install, latest, fixture) = release(&temp, zip.clone(), Some(sums_for(&zip)), None, quirk).await;
            let max_zip = if quirk == Some(Quirk::ClaimsHuge) { MAX_ZIP_BYTES } else { limit };
            let e = fixture.update_with(&latest, &install, &Progress::default(), max_zip, MAX_SUMS_BYTES).await.unwrap_err();
            assert!(format!("{e:#}").contains("too big"), "{case}: {e:#}");
            assert_eq!((std::fs::read(&install.exe).unwrap(), temp.listing()), (b"old ttyak".to_vec(), vec!["install.json".to_string(), EXE.into()]), "{case}");
        }
        // Within the limit, no length given: fine.
        let temp = Temp::new("unsized");
        let (install, latest, fixture) = release(&temp, zip.clone(), Some(sums_for(&zip)), None, Some(Quirk::NoLength)).await;
        let progress = Progress::default();
        fixture.update_with(&latest, &install, &progress, zip.len() as u64, MAX_SUMS_BYTES).await.unwrap();
        assert_eq!((progress.bytes(), std::fs::read(&install.exe).unwrap()), ((zip.len() as u64, None), b"new ttyak".to_vec()));
        // A SHA256SUMS of more than 64 KB.
        let temp = Temp::new("big-sums");
        let padded = format!("{}{}", format!("{}  ttyak-0.0.0-x86_64-linux.zip\n", "00".repeat(32)).repeat(1000), sums_for(&zip));
        assert!(padded.len() as u64 > MAX_SUMS_BYTES);
        let (install, latest, fixture) = release(&temp, zip.clone(), Some(padded), None, None).await;
        let e = fixture.update(&latest, &install, &Progress::default()).await.unwrap_err();
        assert!(format!("{e:#}").contains("SHA256SUMS") && format!("{e:#}").contains("too big"), "{e:#}");
        assert_eq!((std::fs::read(&install.exe).unwrap(), temp.listing()), (b"old ttyak".to_vec(), vec!["install.json".to_string(), EXE.into()]));
    }

    /// Signatures as the files say: one ending ` team <ID>` is signed by that team; a `, broken`
    /// after it: a signature that does not hold; `, unreadable`: its team cannot be read; `, fails`:
    /// checking it fails; anything else unsigned.
    struct Fake;

    impl Verifier for Fake {
        fn team(&self, exe: &Path) -> Result<Option<String>> {
            let text = String::from_utf8(std::fs::read(exe)?)?;
            ensure!(!text.ends_with(", unreadable"), "codesign could not read {}", exe.display());
            Ok(text.rsplit_once(" team ").map(|(_, t)| t.trim_end_matches(", broken").trim_end_matches(", fails").to_string()))
        }

        fn holds(&self, exe: &Path, team: &str) -> Result<bool> {
            let text = std::fs::read(exe)?;
            ensure!(!text.ends_with(b", fails"), "could not run codesign");
            Ok(self.team(exe)?.as_deref() == Some(team) && !text.ends_with(b", broken"))
        }
    }

    #[test]
    fn signer_checked_when_the_running_one_is_signed() {
        let temp = Temp::new("signer");
        let (running, new) = (temp.0.join("bin/running"), temp.0.join("bin/new"));
        let check = |old: &str, update: &str| {
            std::fs::write(&running, old).unwrap();
            std::fs::write(&new, update).unwrap();
            check_signer(&Fake, &running, &new).map_err(|e| format!("{e:#}"))
        };
        assert_eq!(check("ttyak team AAAAAAAAAA", "ttyak team AAAAAAAAAA"), Ok(()), "the same team");
        let other = check("ttyak team AAAAAAAAAA", "ttyak team BBBBBBBBBB").unwrap_err();
        assert!(other.contains("BBBBBBBBBB") && other.contains("AAAAAAAAAA") && other.contains("old one stays"), "{other}");
        let unsigned = check("ttyak team AAAAAAAAAA", "ttyak").unwrap_err();
        assert!(unsigned.contains("not signed") && unsigned.contains("old one stays"), "{unsigned}");
        let broken = check("ttyak team AAAAAAAAAA", "ttyak team AAAAAAAAAA, broken").unwrap_err();
        assert!(broken.contains("does not hold") && broken.contains("old one stays"), "{broken}");
        // A build of one's own (unsigned): nothing checked.
        assert_eq!(check("ttyak", "ttyak team BBBBBBBBBB, broken"), Ok(()));
        assert_eq!(check("ttyak", "ttyak"), Ok(()));
        // Whatever cannot be read or checked refuses (never taken as unsigned).
        for (old, update, says) in [
            ("ttyak team AAAAAAAAAA, unreadable", "ttyak team AAAAAAAAAA", "this ttyak's signature"),
            ("ttyak team AAAAAAAAAA", "ttyak team AAAAAAAAAA, unreadable", "the new ttyak's signature"),
            ("ttyak team AAAAAAAAAA", "ttyak team AAAAAAAAAA, fails", "could not run codesign"),
        ] {
            let e = check(old, update).unwrap_err();
            assert!(e.contains(says), "{old} → {update}: {e}");
        }
    }

    #[test]
    fn team_identifier_read_from_codesign() {
        let developer_id = "Executable=/x/ttyak\nIdentifier=ttyak\nFormat=Mach-O thin (arm64)\nCodeDirectory v=20500 size=1 flags=0x10000(runtime) hashes=1+2 location=embedded\nSignature size=9000\nAuthority=Developer ID Application: A (AAAAAAAAAA)\nAuthority=Developer ID Certification Authority\nAuthority=Apple Root CA\nTimestamp=23 Oct 2026 at 12:00:00\nInfo.plist=not bound\nTeamIdentifier=AAAAAAAAAA\nRuntime Version=15.0.0\n";
        assert_eq!(team_identifier(developer_id).unwrap().as_deref(), Some("AAAAAAAAAA"));
        // Ad hoc (what the linker gives a build on Apple silicon); no line at all.
        assert_eq!(team_identifier("Identifier=ttyak-1a2b\nSignature=adhoc\nTeamIdentifier=not set\n").unwrap(), None);
        assert_eq!(team_identifier("Identifier=ttyak\n").unwrap(), None);
        // A Developer ID without a Team ID that can be read, or one that could be read into a code
        // requirement: an error, never "unsigned".
        for said in [developer_id.replace("TeamIdentifier=AAAAAAAAAA\n", ""), developer_id.replace("TeamIdentifier=AAAAAAAAAA", "TeamIdentifier=not set"), "TeamIdentifier=AA\" or true\n".to_string(), developer_id.replace("=AAAAAAAAAA", "=")] {
            assert!(team_identifier(&said).is_err(), "{said}");
        }
    }

    #[tokio::test]
    async fn another_signer_leaves_the_old_binary() {
        let temp = Temp::new("other-signer");
        let zip = archive(&[(EXE, b"new ttyak team BBBBBBBBBB")]);
        let (install, latest, fixture) = release(&temp, zip.clone(), Some(sums_for(&zip)), None, None).await;
        std::fs::write(&install.exe, b"old ttyak team AAAAAAAAAA").unwrap();
        let e = fixture.update_verified(&latest, &install, &Progress::default(), &Fake).await.unwrap_err();
        assert!(format!("{e:#}").contains("BBBBBBBBBB"), "{e:#}");
        assert_eq!(std::fs::read(&install.exe).unwrap(), b"old ttyak team AAAAAAAAAA");
        assert_eq!((temp.listing(), version_recorded(&temp)), (vec!["install.json".to_string(), EXE.into()], "0.9.9".into()), "nothing staged left");
        // The same team: put in place.
        let temp = Temp::new("same-signer");
        let zip = archive(&[(EXE, b"new ttyak team AAAAAAAAAA")]);
        let (install, latest, fixture) = release(&temp, zip.clone(), Some(sums_for(&zip)), None, None).await;
        std::fs::write(&install.exe, b"old ttyak team AAAAAAAAAA").unwrap();
        fixture.update_verified(&latest, &install, &Progress::default(), &Fake).await.unwrap();
        assert_eq!((std::fs::read(&install.exe).unwrap(), version_recorded(&temp)), (b"new ttyak team AAAAAAAAAA".to_vec(), "9.9.9".into()));
    }

    #[tokio::test]
    async fn a_signature_that_cannot_be_checked_leaves_the_old_binary() {
        for (old, new) in [("old ttyak team AAAAAAAAAA, unreadable", "new ttyak team AAAAAAAAAA"), ("old ttyak team AAAAAAAAAA", "new ttyak team AAAAAAAAAA, unreadable"), ("old ttyak team AAAAAAAAAA", "new ttyak team AAAAAAAAAA, fails")] {
            let temp = Temp::new("unchecked-signer");
            let zip = archive(&[(EXE, new.as_bytes())]);
            let (install, latest, fixture) = release(&temp, zip.clone(), Some(sums_for(&zip)), None, None).await;
            std::fs::write(&install.exe, old).unwrap();
            let e = fixture.update_verified(&latest, &install, &Progress::default(), &Fake).await.unwrap_err();
            assert!(format!("{e:#}").contains("old one stays"), "{old} → {new}: {e:#}");
            assert_eq!(std::fs::read(&install.exe).unwrap(), old.as_bytes(), "{old} → {new}");
            assert_eq!((temp.listing(), version_recorded(&temp)), (vec!["install.json".to_string(), EXE.into()], "0.9.9".into()), "{old} → {new}");
        }
    }

    /// The real `codesign`: a build of one's own has no Developer ID; `TTYAK_TEST_SIGNED` names a
    /// release ttyak (signed) to check it against, when given.
    #[cfg(target_os = "macos")]
    #[test]
    fn codesign_reads_signatures() {
        let me = std::env::current_exe().unwrap();
        assert_eq!(Codesign.team(&me).unwrap(), None, "this test program");
        let temp = Temp::new("codesign");
        let text = temp.0.join("bin/ttyak");
        std::fs::write(&text, b"not a program").unwrap();
        assert_eq!(Codesign.team(&text).unwrap(), None);
        assert!(!Codesign.holds(&text, "AAAAAAAAAA").unwrap());
        if let Some(signed) = std::env::var_os("TTYAK_TEST_SIGNED") {
            let signed = PathBuf::from(signed);
            let team = Codesign.team(&signed).unwrap().expect("a Developer ID signature");
            assert!(Codesign.holds(&signed, &team).unwrap());
            assert!(!Codesign.holds(&signed, "AAAAAAAAAA").unwrap(), "another team");
            assert!(!Codesign.holds(&me, &team).unwrap(), "ad hoc");
            assert!(check_signer(&Codesign, &signed, &me).is_err() && check_signer(&Codesign, &me, &signed).is_ok());
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_swap_renames_aside() {
        /// A program running from the old ttyak.exe (Windows keeps a running executable's file: it
        /// can be renamed, not deleted), stopped even when an assertion fails.
        struct Running(std::process::Child);
        impl Drop for Running {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let temp = Temp::new("windows-swap");
        let dir = temp.0.join("bin");
        let (exe, staged) = (dir.join(EXE), staged(&dir));
        std::fs::copy(PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join(r"System32\PING.EXE"), &exe).unwrap();
        let old = std::fs::read(&exe).unwrap();
        let running = Running(std::process::Command::new(&exe).args(["-n", "60", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap());
        std::fs::write(&staged, b"new").unwrap();
        swap(&staged, &exe).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert_eq!(std::fs::read(dir.join(ASIDE)).unwrap(), old, "the running one aside, for the next start");
        assert!(!staged.exists());
        // That one still running at the next update: this one goes aside under a name of its own, and
        // (not running) is deleted at once.
        std::fs::write(&staged, b"newer").unwrap();
        swap(&staged, &exe).unwrap();
        assert_eq!((std::fs::read(&exe).unwrap(), temp.listing()), (b"newer".to_vec(), vec![EXE.to_string(), ASIDE.to_string()]));
        drop(running);
        // The new one cannot be moved in: the old one is put back.
        let e = swap(&staged, &exe).unwrap_err();
        assert!(format!("{e:#}").contains("stays"), "{e:#}");
        assert_eq!(std::fs::read(&exe).unwrap(), b"newer");
    }
}

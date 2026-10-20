//! Slack sign-ins of the browsers on this machine: `klack auth login --from-browser firefox|chrome`
//! takes the `d` cookie from a browser you are already signed in to Slack with, and [`discover`]
//! lists every workspace each browser profile is signed in to. A cookie database is copied to a
//! temporary folder and read there (nothing of the browser is changed); Chrome's cookies are
//! decrypted with its key from the system keyring (Secret Service on Linux, the Keychain on
//! macOS). Chrome on Windows encrypts them so that only Chrome can read them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use slack_core::session::{self, LoggedInTeam, SessionSource, WorkspaceSession};
use tokio::time::timeout;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Source {
    Firefox,
    Chrome,
}

/// A `d` cookie found in a browser profile.
#[derive(Debug)]
pub struct Found {
    pub profile: PathBuf,
    pub cookie: String,
    /// Last used (microseconds since 1970), to try the freshest first.
    pub last_used: i64,
}

/// A browser profile, named as the browser shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserProfile {
    /// Firefox, or a browser of Chrome's family (Chrome, Chromium, Edge, Brave).
    pub source: Source,
    /// The profile's folder.
    pub dir: PathBuf,
    /// The browser and the profile's name: "Chrome · Work", "Firefox · default-release".
    pub label: String,
    /// The Google account a Chrome-family profile is signed in to.
    pub account: Option<String>,
}

/// A workspace a browser profile is signed in to, and its session (or why it does not work).
pub struct Candidate {
    pub team: LoggedInTeam,
    pub session: Result<WorkspaceSession, String>,
}

/// What one browser profile is signed in to; `problem` says why it could not be looked at.
pub struct Group {
    pub profile: BrowserProfile,
    pub problem: Option<String>,
    pub candidates: Vec<Candidate>,
}

/// How long each step of [`discover`] (a key, a page, a check) may take.
const STEP_TIMEOUT: Duration = Duration::from_secs(10);

/// Why `--from-browser chrome` does not work on Windows.
const CHROME_ON_WINDOWS: &str = "Chrome's cookies cannot be read on this system (app-bound encryption): use Firefox, or sign in with `klack auth login` by a code from your email or a pasted cookie";

/// The files a profile's cookies are kept in.
const FIREFOX_DB: &[&str] = &["cookies.sqlite"];
const CHROME_DBS: &[&str] = &["Network/Cookies", "Cookies"];

/// Every Slack session cookie in `source`'s profiles (or just in `profile`), freshest first.
pub async fn find(source: Source, profile: Option<&Path>) -> Result<Vec<Found>> {
    if source == Source::Chrome && cfg!(windows) {
        bail!("{CHROME_ON_WINDOWS}");
    }
    let profiles: Vec<BrowserProfile> = match profile {
        // One profile, or a browser's folder of profiles.
        Some(p) if is_profile(p) => vec![profile_at(source, p)],
        Some(p) => profiles_with(p, &[FIREFOX_DB, CHROME_DBS].concat()).iter().map(|d| profile_at(source, d)).collect(),
        None => browser_profiles().into_iter().filter(|b| b.source == source).collect(),
    };
    if profiles.is_empty() {
        bail!("no {} profile found (--browser-profile names one)", label(source));
    }
    let mut found = Vec::new();
    let mut keys: HashMap<String, Vec<u8>> = HashMap::new();
    for p in profiles {
        let result = match source {
            Source::Firefox => firefox_cookies(&p.dir),
            Source::Chrome => {
                let family = family(&p);
                if !keys.contains_key(family) {
                    keys.insert(family.to_string(), chrome_key(family).await?);
                }
                chrome_cookies(&p.dir, &keys[family])
            }
        };
        match result {
            Ok(list) => found.extend(list),
            Err(e) => tracing::debug!("{}: {e:#}", p.dir.display()),
        }
    }
    found.sort_by(|a, b| b.last_used.cmp(&a.last_used));
    Ok(found)
}

fn is_profile(p: &Path) -> bool {
    FIREFOX_DB.iter().chain(CHROME_DBS).any(|d| p.join(d).is_file())
}

pub fn label(source: Source) -> &'static str {
    match source {
        Source::Firefox => "Firefox",
        Source::Chrome => "Chrome",
    }
}

/// The browser of a profile as its label names it: "Firefox", or "Chrome", "Chromium", "Edge", "Brave".
fn family(p: &BrowserProfile) -> &str {
    p.label.split(" · ").next().unwrap_or(label(p.source))
}

fn home() -> PathBuf {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default()
}

/// Every Firefox and Chrome-family profile of this machine, named as its browser shows it.
pub fn browser_profiles() -> Vec<BrowserProfile> {
    let mut out: Vec<BrowserProfile> = firefox_roots().iter().flat_map(|root| firefox_profiles_in(root)).collect();
    out.extend(chrome_roots().iter().flat_map(|(family, root)| chrome_family_profiles(family, root)));
    listed(out, cfg!(windows))
}

/// The profile at `dir`, named as [`browser_profiles`] names it (or by its folder when it is not
/// in a browser's usual place).
pub fn profile_at(source: Source, dir: &Path) -> BrowserProfile {
    let root = dir.parent().unwrap_or(dir);
    let mut found = match source {
        Source::Firefox => firefox_profiles_in(root),
        Source::Chrome => {
            let family = chrome_roots().into_iter().find(|(_, r)| r == root).map_or("Chrome", |(f, _)| f);
            chrome_family_profiles(family, root)
        }
    };
    match found.iter().position(|p| p.dir == dir) {
        Some(at) => found.swap_remove(at),
        None => BrowserProfile { source, dir: dir.to_path_buf(), label: format!("{} · {}", label(source), folder_name(dir)), account: None },
    }
}

fn folder_name(dir: &Path) -> String {
    dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

/// The profile folders under `root` that hold one of the files `db` (relative to the profile), by name.
fn profiles_with(root: &Path, db: &[&str]) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
    let mut out: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| db.iter().any(|d| p.join(d).is_file())).collect();
    out.sort();
    out
}

/// Where Firefox keeps its profiles.
fn firefox_roots() -> Vec<PathBuf> {
    let h = home();
    if cfg!(target_os = "macos") {
        vec![h.join("Library/Application Support/Firefox/Profiles")]
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(|a| vec![PathBuf::from(a).join(r"Mozilla\Firefox\Profiles")]).unwrap_or_default()
    } else {
        vec![h.join(".mozilla/firefox"), h.join("snap/firefox/common/.mozilla/firefox"), h.join(".var/app/org.mozilla.firefox/.mozilla/firefox")]
    }
}

/// Where the browsers of Chrome's family keep their profiles, each with the browser's name.
fn chrome_roots() -> Vec<(&'static str, PathBuf)> {
    let h = home();
    if cfg!(target_os = "macos") {
        let support = h.join("Library/Application Support");
        [("Chrome", "Google/Chrome"), ("Chromium", "Chromium"), ("Edge", "Microsoft Edge"), ("Brave", "BraveSoftware/Brave-Browser")].into_iter().map(|(f, d)| (f, support.join(d))).collect()
    } else if cfg!(windows) {
        // Not readable there (see `listed`).
        Vec::new()
    } else {
        let config = h.join(".config");
        let mut r: Vec<(&str, PathBuf)> = [("Chrome", "google-chrome"), ("Chromium", "chromium"), ("Edge", "microsoft-edge"), ("Brave", "BraveSoftware/Brave-Browser")].into_iter().map(|(f, d)| (f, config.join(d))).collect();
        r.push(("Chrome", h.join(".var/app/com.google.Chrome/config/google-chrome")));
        r.push(("Chromium", h.join(".var/app/org.chromium.Chromium/config/chromium")));
        r
    }
}

/// The profiles of the Chrome-family browser `family` under `root`, named as its `Local State`
/// shows them (`profile.info_cache.<folder>.name`, and the Google account in `.user_name`).
fn chrome_family_profiles(family: &str, root: &Path) -> Vec<BrowserProfile> {
    let state = std::fs::read_to_string(root.join("Local State")).ok().and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok()).unwrap_or_default();
    profiles_with(root, CHROME_DBS)
        .into_iter()
        .map(|dir| {
            let folder = folder_name(&dir);
            let info = &state["profile"]["info_cache"][&folder];
            let shown = |k: &str| info[k].as_str().filter(|v| !v.is_empty()).map(String::from);
            let label = format!("{family} · {}", shown("name").unwrap_or(folder));
            BrowserProfile { source: Source::Chrome, dir, label, account: shown("user_name") }
        })
        .collect()
}

/// The Firefox profiles under `root`, named as `profiles.ini` names them (next to the profiles on
/// Linux, one level up on macOS and Windows), else by their folder.
fn firefox_profiles_in(root: &Path) -> Vec<BrowserProfile> {
    let ini = [Some(root), root.parent()].into_iter().flatten().find_map(|d| std::fs::read_to_string(d.join("profiles.ini")).ok()).unwrap_or_default();
    // Each section's `Path=` (its last part: the folder) and `Name=`.
    let mut names: Vec<(String, String)> = Vec::new();
    let (mut path, mut name) = (None, None);
    for line in ini.lines().map(str::trim).chain(["["]) {
        if line.starts_with('[') {
            if let (Some(p), Some(n)) = (path.take(), name.take()) {
                names.push((p, n));
            }
            (path, name) = (None, None);
        } else if let Some(p) = line.strip_prefix("Path=") {
            path = p.rsplit(['/', '\\']).next().map(String::from);
        } else if let Some(n) = line.strip_prefix("Name=") {
            name = Some(n.to_string());
        }
    }
    profiles_with(root, FIREFOX_DB)
        .into_iter()
        .map(|dir| {
            let folder = folder_name(&dir);
            let shown = names.iter().find(|(p, _)| *p == folder).map_or(folder.clone(), |(_, n)| n.clone());
            BrowserProfile { source: Source::Firefox, dir, label: format!("Firefox · {shown}"), account: None }
        })
        .collect()
}

/// Every workspace each browser profile of this machine is signed in to, checked.
#[cfg(test)]
pub async fn discover() -> Vec<Group> {
    discover_in(browser_profiles()).await
}

/// Every workspace each of `profiles` is signed in to: its `d` cookies (freshest first), the
/// workspaces Slack's sign-in page lists for each, and a session for each (`session_from_cookie`),
/// all at once but at most six requests to Slack together, over one client. A profile that cannot
/// be looked at (its browser's key, a step that takes longer than 10 s, a sign-in page klack can't
/// read) gets a `problem`; nothing here fails.
pub async fn discover_in(profiles: Vec<BrowserProfile>) -> Vec<Group> {
    let keys = read_keys(&profiles).await;
    discover_with_keys(profiles, &keys).await
}

/// How the browsers' keys are read while looking: `one_at_a_time` (one browser after another), each waited for
/// `timeout` at most.
#[derive(Debug, Clone, Copy)]
struct KeyReads {
    one_at_a_time: bool,
    timeout: Duration,
}

/// On macOS each read raises a Keychain dialog: one browser after another (never several dialogs at once), each given
/// a minute to be answered. Elsewhere nothing is asked: all at once, within [`STEP_TIMEOUT`].
const KEY_READS: KeyReads = if cfg!(target_os = "macos") { KeyReads { one_at_a_time: true, timeout: Duration::from_secs(60) } } else { KeyReads { one_at_a_time: false, timeout: STEP_TIMEOUT } };

/// At most this many requests to Slack at once while looking (a machine with many browser
/// profiles signed in to many workspaces would otherwise send them all together).
const CHECKS_AT_ONCE: usize = 6;

/// What a profile's group says when Slack's sign-in page no longer has the list klack reads.
const PAGE_CHANGED: &str = "Slack's sign-in page changed — klack couldn't read it";

/// One sign-in page as looked at: its workspaces, `None` for a page without the list, or why there is none.
type Page = Result<Option<Vec<LoggedInTeam>>, String>;

/// `step` once one of `limit`'s places is free, within [`STEP_TIMEOUT`] (the wait for a place does not count).
async fn limited<F: std::future::Future>(limit: &tokio::sync::Semaphore, step: F) -> Result<F::Output, tokio::time::error::Elapsed> {
    // The semaphore is never closed, so the place is always given.
    let _place = limit.acquire().await.ok();
    timeout(STEP_TIMEOUT, step).await
}

/// The workspaces of a profile's sign-in pages (one per cookie, freshest first), each account once (the freshest
/// cookie's), with the index of its cookie; when there are none, why (a page that could not be read or got no answer).
fn teams_of(pages: Vec<Page>) -> (Vec<(LoggedInTeam, usize)>, Option<String>) {
    let mut teams: Vec<(LoggedInTeam, usize)> = Vec::new();
    let mut failed = None;
    for (cookie, page) in pages.into_iter().enumerate() {
        match page {
            Ok(Some(list)) => {
                for t in list {
                    if !teams.iter().any(|(o, _)| o.team_id == t.team_id && o.email == t.email) {
                        teams.push((t, cookie));
                    }
                }
            }
            Ok(None) => failed = Some(PAGE_CHANGED.to_string()),
            Err(e) => failed = Some(e),
        }
    }
    if !teams.is_empty() {
        failed = None;
    }
    (teams, failed)
}

/// The profiles klack can read on this platform: on Windows Chrome's family encrypts its cookies
/// so that only the browser can read them, so those are left out there.
fn listed(profiles: Vec<BrowserProfile>, windows: bool) -> Vec<BrowserProfile> {
    profiles.into_iter().filter(|p| !(windows && p.source == Source::Chrome)).collect()
}

/// Whether Chrome is on this machine where its cookies can't be read: Windows (app-bound encryption), where
/// [`browser_profiles`] leaves it out. Elsewhere `false`.
pub fn unreadable_chrome_here() -> bool {
    cfg!(windows) && std::env::var_os("LOCALAPPDATA").is_some_and(|a| PathBuf::from(a).join(r"Google\Chrome\User Data").exists())
}

/// When `profile`'s cookies were last written: the newest modification time of its cookie database and the
/// database's `-wal` / `-journal`; `None` when it has none.
pub fn stamp(profile: &BrowserProfile) -> Option<std::time::SystemTime> {
    let dbs = match profile.source {
        Source::Firefox => FIREFOX_DB,
        Source::Chrome => CHROME_DBS,
    };
    dbs.iter().flat_map(|db| ["", "-wal", "-journal"].map(|side| profile.dir.join(format!("{db}{side}")))).filter_map(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok()).max()
}

/// The keys of the Chrome-family browsers, read once: each browser's key, or why it can't be read. Kept while the way
/// "in your browser" watches, so on macOS the Keychain is asked once.
pub struct Keys(HashMap<String, Result<Vec<u8>, String>>);

impl Keys {
    /// Whether `profile` can be read with these keys: Firefox always, a Chrome-family profile when its browser's key
    /// was read.
    pub fn has(&self, profile: &BrowserProfile) -> bool {
        profile.source == Source::Firefox || matches!(self.0.get(family(profile)), Some(Ok(_)))
    }
}

/// A hash of a `d` cookie, so a new one is told apart without keeping the value.
pub fn cookie_hash(cookie: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    cookie.hash(&mut h);
    h.finish()
}

/// The hashes of `profile`'s `d` cookies, read from its cookie database only (nothing is asked of Slack); an error
/// when its browser has no key in `keys` or the database can't be read (a browser writing it, say).
pub async fn d_hashes(profile: &BrowserProfile, keys: &Keys) -> Result<std::collections::HashSet<u64>> {
    let key = match profile.source {
        Source::Firefox => None,
        Source::Chrome => match keys.0.get(family(profile)) {
            Some(Ok(key)) => Some(key.clone()),
            _ => bail!("no key for {}", family(profile)),
        },
    };
    Ok(read_cookies(profile, key).await?.iter().map(|f| cookie_hash(&f.cookie)).collect())
}

/// The key of each Chrome-family browser of `profiles` (on macOS one Keychain question per browser).
pub async fn read_keys(profiles: &[BrowserProfile]) -> Keys {
    read_keys_with(profiles, KEY_READS, |family| async move { chrome_key(&family).await }).await
}

/// [`read_keys`], reading a Chrome-family browser's key with `key` as `reads` says.
async fn read_keys_with<K, F>(profiles: &[BrowserProfile], reads: KeyReads, key: K) -> Keys
where
    K: Fn(String) -> F,
    F: std::future::Future<Output = Result<Vec<u8>>>,
{
    // Each browser's key once (on macOS the Keychain asks once per browser).
    let mut families: Vec<String> = Vec::new();
    for p in profiles.iter().filter(|p| p.source == Source::Chrome && !cfg!(windows)) {
        if !families.iter().any(|f| f == family(p)) {
            families.push(family(p).to_string());
        }
    }
    let read = |f: &str| {
        let (f, read) = (f.to_string(), key(f.to_string()));
        async move {
            match timeout(reads.timeout, read).await {
                Ok(Ok(key)) => Ok(key),
                Ok(Err(e)) => Err(format!("can't read {f}'s key: {e:#}")),
                Err(_) => Err(format!("can't read {f}'s key: no answer in {} s", reads.timeout.as_secs())),
            }
        }
    };
    let got = if reads.one_at_a_time {
        let mut got = Vec::new();
        for f in &families {
            got.push(read(f).await);
        }
        got
    } else {
        futures_util::future::join_all(families.iter().map(|f| read(f))).await
    };
    Keys(families.into_iter().zip(got).collect())
}

/// [`discover_in`] with the browsers' keys read already (`keys`).
pub async fn discover_with_keys(profiles: Vec<BrowserProfile>, keys: &Keys) -> Vec<Group> {
    let profiles = listed(profiles, cfg!(windows));
    // One client for every request, made when the first cookie is found.
    let http = std::sync::OnceLock::new();
    let limit = tokio::sync::Semaphore::new(CHECKS_AT_ONCE);
    futures_util::future::join_all(profiles.into_iter().map(|profile| {
        let key = keys.0.get(family(&profile));
        profile_group(profile, key, &http, &limit)
    }))
    .await
}

/// [`discover_in`], reading a Chrome-family browser's key with `key` as `reads` says.
#[cfg(test)]
async fn discover_with<K, F>(profiles: Vec<BrowserProfile>, reads: KeyReads, key: K) -> Vec<Group>
where
    K: Fn(String) -> F,
    F: std::future::Future<Output = Result<Vec<u8>>>,
{
    let keys = read_keys_with(&profiles, reads, key).await;
    discover_with_keys(profiles, &keys).await
}

/// The `d` cookies of `profile` (Chrome's family: decrypted with `key`), read off the async threads: copying the
/// database, SQLite and the key derivation block.
async fn read_cookies(profile: &BrowserProfile, key: Option<Vec<u8>>) -> Result<Vec<Found>> {
    let (source, dir) = (profile.source, profile.dir.clone());
    tokio::task::spawn_blocking(move || match (source, key) {
        (Source::Firefox, _) => firefox_cookies(&dir),
        (Source::Chrome, Some(key)) => chrome_cookies(&dir, &key),
        (Source::Chrome, None) => Err(anyhow!("no key")),
    })
    .await
    .map_err(|e| anyhow!("{e}"))?
}

/// What `profile` is signed in to; `key` is its browser's key (Chrome's family). Requests go out over the client in
/// `http` (made by the first that needs it), each once `limit` has a place.
async fn profile_group(profile: BrowserProfile, key: Option<&Result<Vec<u8>, String>>, http: &std::sync::OnceLock<Result<slack_core::reqwest::Client, String>>, limit: &tokio::sync::Semaphore) -> Group {
    let mut group = Group { profile, problem: None, candidates: Vec::new() };
    let key = match (group.profile.source, key) {
        (Source::Chrome, Some(Err(problem))) => {
            group.problem = Some(problem.clone());
            return group;
        }
        (Source::Chrome, Some(Ok(key))) => Some(key.clone()),
        _ => None,
    };
    let mut cookies = match read_cookies(&group.profile, key).await {
        Ok(c) => c,
        Err(e) => {
            group.problem = Some(format!("can't read the cookies: {e:#}"));
            return group;
        }
    };
    cookies.sort_by(|a, b| b.last_used.cmp(&a.last_used));
    let mut seen = Vec::new();
    cookies.retain(|f| !seen.contains(&f.cookie) && {
        seen.push(f.cookie.clone());
        true
    });
    if cookies.is_empty() {
        return group;
    }
    let http = match http.get_or_init(|| session::web_client().map_err(|e| format!("can't reach Slack: {e:#}"))) {
        Ok(http) => http,
        Err(problem) => {
            group.problem = Some(problem.clone());
            return group;
        }
    };
    // The workspaces of each cookie, each account once (the freshest cookie's).
    let pages = futures_util::future::join_all(cookies.iter().map(|f| limited(limit, session::signed_in_teams_with(http, &f.cookie)))).await;
    let pages = pages
        .into_iter()
        .map(|page| match page {
            Ok(Ok(list)) => Ok(list),
            Ok(Err(e)) => Err(format!("can't reach Slack: {e:#}")),
            Err(_) => Err(format!("Slack did not answer in {} s", STEP_TIMEOUT.as_secs())),
        })
        .collect();
    let (teams, problem) = teams_of(pages);
    if teams.is_empty() {
        group.problem = problem;
        return group;
    }
    let source = SessionSource::Browser { browser: label(group.profile.source).to_lowercase(), profile: group.profile.dir.clone(), label: group.profile.label.clone() };
    let checks = futures_util::future::join_all(teams.iter().map(|(t, cookie)| limited(limit, session::session_from_cookie_with(http, &t.url, &cookies[*cookie].cookie)))).await;
    group.candidates = teams
        .into_iter()
        .zip(checks)
        .map(|((team, _), check)| {
            let session = match check {
                Ok(Ok(s)) => Ok(WorkspaceSession { source: Some(source.clone()), ..s }),
                Ok(Err(e)) => Err(format!("{e:#}")),
                Err(_) => Err(format!("Slack did not answer in {} s", STEP_TIMEOUT.as_secs())),
            };
            Candidate { team, session }
        })
        .collect();
    group
}

/// A copy of `db` (and its journal / WAL) in a fresh temporary folder, so a running browser's
/// lock does not matter and nothing of it is written.
struct Copy {
    dir: PathBuf,
    db: PathBuf,
}

impl Copy {
    fn of(db: &Path) -> Result<Copy> {
        let dir = std::env::temp_dir().join(format!("klack-cookies-{}-{}", std::process::id(), rand_suffix()));
        // Owner-only: the copies hold session cookies (Firefox's in plain text).
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(std::fs::DirBuilder::new().recursive(true), 0o700).create(&dir)?;
        #[cfg(not(unix))]
        std::fs::create_dir_all(&dir)?;
        let name = db.file_name().ok_or_else(|| anyhow!("no file name"))?;
        let copy = Copy { dir: dir.clone(), db: dir.join(name) };
        copy_private(db, &copy.db).with_context(|| format!("copying {}", db.display()))?;
        for extra in ["-wal", "-journal"] {
            let side = PathBuf::from(format!("{}{extra}", db.display()));
            if side.is_file() {
                let _ = copy_private(&side, &dir.join(format!("{}{extra}", name.to_string_lossy())));
            }
        }
        Ok(copy)
    }
}

/// Copy `from` to `to`, readable by the owner only (`fs::copy` keeps the source's mode).
fn copy_private(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::copy(from, to)?;
    #[cfg(unix)]
    std::fs::set_permissions(to, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    Ok(())
}

impl Drop for Copy {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn rand_suffix() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos())
}

fn firefox_cookies(profile: &Path) -> Result<Vec<Found>> {
    let copy = Copy::of(&profile.join("cookies.sqlite"))?;
    let con = rusqlite::Connection::open(&copy.db)?;
    let mut stmt = con.prepare("SELECT value, lastAccessed FROM moz_cookies WHERE name = 'd' AND host IN ('.slack.com', 'slack.com')")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    Ok(rows.flatten().filter(|(v, _)| v.starts_with("xoxd-")).map(|(cookie, last_used)| Found { profile: profile.to_path_buf(), cookie, last_used }).collect())
}

fn chrome_cookies(profile: &Path, key_password: &[u8]) -> Result<Vec<Found>> {
    let db = [profile.join("Network/Cookies"), profile.join("Cookies")].into_iter().find(|p| p.is_file()).ok_or_else(|| anyhow!("no cookie database"))?;
    let copy = Copy::of(&db)?;
    let con = rusqlite::Connection::open(&copy.db)?;
    let version: i64 = con.query_row("SELECT value FROM meta WHERE key = 'version'", [], |r| r.get::<_, String>(0)).ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut stmt = con.prepare("SELECT value, encrypted_value, last_access_utc FROM cookies WHERE name = 'd' AND host_key IN ('.slack.com', 'slack.com')")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?, r.get::<_, i64>(2)?)))?;
    let mut out = Vec::new();
    for (plain, encrypted, last_access) in rows.flatten() {
        let value = if !plain.is_empty() { plain } else { decrypt_chrome(&encrypted, key_password, version)? };
        if value.starts_with("xoxd-") {
            // Chrome counts microseconds from 1601.
            out.push(Found { profile: profile.to_path_buf(), cookie: value, last_used: last_access - 11_644_473_600_000_000 });
        }
    }
    Ok(out)
}

/// PBKDF2 rounds for Chrome's cookie key: 1 on Linux, 1003 on macOS.
const PBKDF2_ROUNDS: u32 = if cfg!(target_os = "macos") { 1003 } else { 1 };

/// Decrypt a Chrome cookie (`v10` / `v11`: AES-128-CBC, IV of 16 spaces, key = PBKDF2-HMAC-SHA1 of
/// the keyring password with salt "saltysalt"); from database version 24 the plaintext starts with
/// a SHA-256 of the host, which is dropped.
fn decrypt_chrome(encrypted: &[u8], password: &[u8], db_version: i64) -> Result<String> {
    use aes::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
    let (prefix, body) = encrypted.split_at(3.min(encrypted.len()));
    // v10 on Linux is the fixed "peanuts" password (no keyring); v11 the keyring's.
    let password: &[u8] = match prefix {
        b"v10" if cfg!(target_os = "linux") => b"peanuts",
        b"v10" | b"v11" => password,
        _ => bail!("unknown cookie encryption ({})", String::from_utf8_lossy(prefix)),
    };
    let key = pbkdf2_sha1(password, b"saltysalt", PBKDF2_ROUNDS);
    let mut buf = body.to_vec();
    let plain = cbc::Decryptor::<aes::Aes128>::new(&key.into(), &[b' '; 16].into()).decrypt_padded_mut::<Pkcs7>(&mut buf).map_err(|_| anyhow!("the cookie does not decrypt (another key?)"))?;
    let plain = if db_version >= 24 && plain.len() >= 32 { &plain[32..] } else { plain };
    Ok(String::from_utf8(plain.to_vec())?)
}

/// One 16-byte block of PBKDF2-HMAC-SHA1 (all a 128-bit key needs).
fn pbkdf2_sha1(password: &[u8], salt: &[u8], rounds: u32) -> [u8; 16] {
    use hmac::{Hmac, Mac};
    let prf = |data: &[u8]| -> [u8; 20] {
        let mut mac = Hmac::<sha1::Sha1>::new_from_slice(password).expect("HMAC takes any key length");
        mac.update(data);
        mac.finalize().into_bytes().into()
    };
    let mut u = prf(&[salt, &1u32.to_be_bytes()].concat());
    let mut t = u;
    for _ in 1..rounds {
        u = prf(&u);
        for (a, b) in t.iter_mut().zip(u) {
            *a ^= b;
        }
    }
    let mut key = [0u8; 16];
    key.copy_from_slice(&t[..16]);
    key
}

/// The keyring entries of the Chrome-family browsers: (family, Secret Service `application`, Keychain service).
const KEY_ENTRIES: [(&str, &str, &str); 4] = [("Chrome", "chrome", "Chrome Safe Storage"), ("Chromium", "chromium", "Chromium Safe Storage"), ("Brave", "brave", "Brave Safe Storage"), ("Edge", "microsoft-edge", "Microsoft Edge Safe Storage")];

/// The entries to try for `family`'s key: its own first, then the others' — or its own only (`own_only`).
fn key_entries(family: &str, own_only: bool) -> Vec<(&'static str, &'static str, &'static str)> {
    let mut entries = KEY_ENTRIES.to_vec();
    entries.sort_by_key(|(f, _, _)| *f != family);
    if own_only {
        entries.retain(|(f, _, _)| *f == family);
    }
    entries
}

/// The cookie-key password of the Chrome-family browser `family` ("Chrome", "Chromium", "Edge",
/// "Brave") from the system keyring: its own entry first, then the others'. On macOS its own entry
/// only: the Keychain asks about each entry read, and another browser's is never this one's key.
async fn chrome_key(family: &str) -> Result<Vec<u8>> {
    let entries = key_entries(family, cfg!(target_os = "macos"));
    #[cfg(target_os = "linux")]
    {
        let apps: Vec<&str> = entries.iter().map(|(_, app, _)| *app).collect();
        secret_service_password(&apps).await
    }
    #[cfg(target_os = "macos")]
    {
        for (_, _, service) in entries {
            // Not std's Command: a Keychain that does not answer (asked over ssh) must not hold
            // up the timeouts of the callers.
            let out = tokio::process::Command::new("security").args(["find-generic-password", "-w", "-s", service]).kill_on_drop(true).output().await?;
            if out.status.success() {
                return Ok(String::from_utf8_lossy(&out.stdout).trim().as_bytes().to_vec());
            }
        }
        bail!("no {family} Safe Storage entry in the Keychain")
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = entries;
        bail!("{CHROME_ON_WINDOWS}")
    }
}

/// The secret of the first Secret Service item whose `application` is one of `apps` (only the
/// browsers' "Safe Storage" items; nothing else in the keyring is looked at).
#[cfg(target_os = "linux")]
async fn secret_service_password(apps: &[&str]) -> Result<Vec<u8>> {
    use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
    let conn = zbus::Connection::session().await.context("no D-Bus session")?;
    let service = zbus::Proxy::new(&conn, "org.freedesktop.secrets", "/org/freedesktop/secrets", "org.freedesktop.Secret.Service").await?;
    let (_, session): (OwnedValue, OwnedObjectPath) = service.call("OpenSession", &("plain", Value::from(""))).await.context("the keyring (Secret Service) is not available")?;
    for app in apps {
        let attrs = std::collections::HashMap::from([("application", *app)]);
        let (unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = service.call("SearchItems", &(attrs,)).await?;
        if !locked.is_empty() && unlocked.is_empty() {
            let paths: Vec<ObjectPath> = locked.iter().map(|p| p.as_ref()).collect();
            let _: (Vec<OwnedObjectPath>, OwnedObjectPath) = service.call("Unlock", &(paths,)).await?;
        }
        let Some(item) = unlocked.first().or(locked.first()) else { continue };
        let item = zbus::Proxy::new(&conn, "org.freedesktop.secrets", item.as_ref(), "org.freedesktop.Secret.Item").await?;
        let (_, _, secret, _): (OwnedObjectPath, Vec<u8>, Vec<u8>, String) = item.call("GetSecret", &(session.as_ref(),)).await.context("reading the keyring item")?;
        return Ok(secret);
    }
    bail!("no Chrome / Chromium Safe Storage item in the keyring")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encrypt like Chrome does, to check the decryption against.
    fn chrome_encrypt(plain: &[u8], password: &[u8], prefix: &[u8]) -> Vec<u8> {
        use aes::cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
        let key = pbkdf2_sha1(password, b"saltysalt", PBKDF2_ROUNDS);
        let mut out = prefix.to_vec();
        let mut buf = vec![0u8; plain.len() + 16];
        buf[..plain.len()].copy_from_slice(plain);
        let sealed = cbc::Encryptor::<aes::Aes128>::new(&key.into(), &[b' '; 16].into()).encrypt_padded_mut::<Pkcs7>(&mut buf, plain.len()).unwrap();
        out.extend_from_slice(sealed);
        out
    }

    #[test]
    fn pbkdf2_matches_the_rfc_6070_vector() {
        // RFC 6070: P = "password", S = "salt", c = 1 → 0c60c80f961f0e71f3a9b524af6012062fe037a6.
        assert_eq!(pbkdf2_sha1(b"password", b"salt", 1), [0x0c, 0x60, 0xc8, 0x0f, 0x96, 0x1f, 0x0e, 0x71, 0xf3, 0xa9, 0xb5, 0x24, 0xaf, 0x60, 0x12, 0x06]);
        assert_eq!(pbkdf2_sha1(b"password", b"salt", 2)[..4], [0xea, 0x6c, 0x01, 0x4d]);
    }

    #[test]
    fn chrome_cookies_decrypt() {
        // Database version 24: a 32-byte host hash before the value.
        let mut plain = vec![7u8; 32];
        plain.extend(b"xoxd-abc%2Fdef");
        let enc = chrome_encrypt(&plain, b"keyring-password", b"v11");
        assert_eq!(decrypt_chrome(&enc, b"keyring-password", 24).unwrap(), "xoxd-abc%2Fdef");
        // Older databases have no hash.
        assert_eq!(decrypt_chrome(&chrome_encrypt(b"xoxd-old", b"pw", b"v11"), b"pw", 23).unwrap(), "xoxd-old");
        assert!(decrypt_chrome(&enc, b"wrong", 24).is_err(), "another key does not decrypt");
        assert!(decrypt_chrome(b"v99abc", b"pw", 24).is_err());
    }

    #[test]
    fn firefox_cookies_are_read_from_a_copy() {
        let profile = std::env::temp_dir().join(format!("klack-test-ff-{}", rand_suffix()));
        std::fs::create_dir_all(&profile).unwrap();
        {
            // The columns Firefox's moz_cookies has that matter here.
            let con = rusqlite::Connection::open(profile.join("cookies.sqlite")).unwrap();
            con.execute_batch("CREATE TABLE moz_cookies (id INTEGER PRIMARY KEY, name TEXT, value TEXT, host TEXT, lastAccessed INTEGER);
                INSERT INTO moz_cookies (name, value, host, lastAccessed) VALUES
                ('d', 'xoxd-older', '.slack.com', 100), ('d', 'xoxd-newer', '.slack.com', 200),
                ('d', 'xoxd-elsewhere', '.example.com', 300), ('b', 'x', '.slack.com', 400), ('d', 'not-a-session', '.slack.com', 500);").unwrap();
        }
        let mut found = firefox_cookies(&profile).unwrap();
        found.sort_by(|a, b| b.last_used.cmp(&a.last_used));
        assert_eq!(found.iter().map(|f| f.cookie.as_str()).collect::<Vec<_>>(), ["xoxd-newer", "xoxd-older"]);
        std::fs::remove_dir_all(&profile).unwrap();
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("klack-test-{name}-{}", rand_suffix()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }

    #[test]
    fn chrome_profiles_are_named_as_chrome_shows_them() {
        let root = temp("chrome-names");
        touch(&root.join("Default/Network/Cookies"));
        touch(&root.join("Profile 1/Cookies"));
        // Not a profile: no cookie database.
        std::fs::create_dir_all(root.join("System Profile")).unwrap();
        std::fs::write(root.join("Local State"), r#"{"profile": {"info_cache": {"Default": {"name": "Work", "user_name": "person@example.com"}, "Profile 1": {"user_name": ""}}}}"#).unwrap();
        let found = chrome_family_profiles("Chrome", &root);
        let shown: Vec<(&str, Option<&str>, PathBuf)> = found.iter().map(|p| (p.label.as_str(), p.account.as_deref(), p.dir.clone())).collect();
        assert_eq!(shown, [("Chrome · Work", Some("person@example.com"), root.join("Default")), ("Chrome · Profile 1", None, root.join("Profile 1"))]);
        assert!(found.iter().all(|p| p.source == Source::Chrome));
        // The family is the root's.
        assert_eq!(chrome_family_profiles("Brave", &root)[0].label, "Brave · Work");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn firefox_profiles_use_their_ini_names() {
        // Linux: profiles.ini next to the profile folders.
        let root = temp("firefox-names");
        touch(&root.join("abc123.default-release/cookies.sqlite"));
        touch(&root.join("zz9.unlisted/cookies.sqlite"));
        std::fs::create_dir_all(root.join("Crash Reports")).unwrap();
        std::fs::write(root.join("profiles.ini"), "[General]\nStartWithLastProfile=1\n\n[Profile0]\nName=default-release\nIsRelative=1\nPath=abc123.default-release\nDefault=1\n").unwrap();
        let labels: Vec<(String, PathBuf)> = firefox_profiles_in(&root).into_iter().map(|p| (p.label, p.dir)).collect();
        assert_eq!(labels, [("Firefox · default-release".to_string(), root.join("abc123.default-release")), ("Firefox · zz9.unlisted".to_string(), root.join("zz9.unlisted"))]);
        // macOS and Windows: profiles.ini one level up, paths under Profiles/.
        let base = temp("firefox-mac");
        touch(&base.join("Profiles/q1w2.work/cookies.sqlite"));
        std::fs::write(base.join("profiles.ini"), "[Profile1]\r\nName=work\r\nIsRelative=1\r\nPath=Profiles/q1w2.work\r\n").unwrap();
        let found = firefox_profiles_in(&base.join("Profiles"));
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].label.as_str(), found[0].account.as_deref(), found[0].source), ("Firefox · work", None, Source::Firefox));
        std::fs::remove_dir_all(&root).unwrap();
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[tokio::test]
    async fn an_unreadable_key_is_a_problem_not_an_error() {
        let chrome = temp("chrome-key");
        touch(&chrome.join("Default/Cookies"));
        touch(&chrome.join("Profile 1/Cookies"));
        let firefox = temp("firefox-key");
        {
            let con = rusqlite::Connection::open(firefox.join("cookies.sqlite")).unwrap();
            con.execute_batch("CREATE TABLE moz_cookies (id INTEGER PRIMARY KEY, name TEXT, value TEXT, host TEXT, lastAccessed INTEGER);").unwrap();
        }
        let profile = |source, dir: PathBuf, label: &str| BrowserProfile { source, dir, label: label.into(), account: None };
        let profiles = vec![profile(Source::Chrome, chrome.join("Default"), "Chrome · Work"), profile(Source::Firefox, firefox.clone(), "Firefox · default"), profile(Source::Chrome, chrome.join("Profile 1"), "Chrome · Profile 1")];
        let asked = std::sync::atomic::AtomicUsize::new(0);
        let groups = discover_with(profiles, KEY_READS, |_family| {
            asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async { Err(anyhow!("the keyring is locked")) }
        })
        .await;
        let problems: Vec<(&str, Option<&str>, usize)> = groups.iter().map(|g| (g.profile.label.as_str(), g.problem.as_deref(), g.candidates.len())).collect();
        let key_problem = Some("can't read Chrome's key: the keyring is locked");
        if cfg!(windows) {
            // Chrome's family is not listed there at all.
            assert_eq!(problems, [("Firefox · default", None, 0)]);
        } else {
            assert_eq!(problems, [("Chrome · Work", key_problem, 0), ("Firefox · default", None, 0), ("Chrome · Profile 1", key_problem, 0)]);
        }
        // The key is asked for once per browser, not per profile.
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), if cfg!(windows) { 0 } else { 1 });
        std::fs::remove_dir_all(&chrome).unwrap();
        std::fs::remove_dir_all(&firefox).unwrap();
    }

    #[tokio::test]
    async fn keys_can_be_read_one_browser_after_another() {
        use std::sync::Mutex;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let profile = |label: &str| BrowserProfile { source: Source::Chrome, dir: PathBuf::from(format!("/nowhere/{label}")), label: label.into(), account: None };
        let profiles = || vec![profile("Chrome · Work"), profile("Edge · Default"), profile("Chrome · Home"), profile("Brave · Default")];
        for (one_at_a_time, most_at_once) in [(true, 1), (false, 3)] {
            let (now, most) = (AtomicUsize::new(0), AtomicUsize::new(0));
            let asked = Mutex::new(Vec::new());
            let reads = KeyReads { one_at_a_time, timeout: Duration::from_secs(5) };
            discover_with(profiles(), reads, |family| {
                asked.lock().unwrap().push(family);
                let n = now.fetch_add(1, Ordering::SeqCst) + 1;
                most.fetch_max(n, Ordering::SeqCst);
                let now = &now;
                async move {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    now.fetch_sub(1, Ordering::SeqCst);
                    // A denied dialog: that browser only, nothing else is asked.
                    Err(anyhow!("denied"))
                }
            })
            .await;
            // Each browser once, in the order its first profile comes.
            assert_eq!(*asked.lock().unwrap(), if cfg!(windows) { Vec::<String>::new() } else { vec!["Chrome".to_string(), "Edge".into(), "Brave".into()] });
            if !cfg!(windows) {
                assert_eq!(most.load(Ordering::SeqCst), most_at_once, "one_at_a_time: {one_at_a_time}");
            }
        }
        // A dialog nobody answers in time is a problem of that browser only.
        let reads = KeyReads { one_at_a_time: true, timeout: Duration::from_millis(50) };
        let groups = discover_with(vec![profile("Chrome · Work")], reads, |_| async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok(vec![])
        })
        .await;
        if !cfg!(windows) {
            assert_eq!(groups[0].problem.as_deref(), Some("can't read Chrome's key: no answer in 0 s"));
        }
    }

    #[test]
    fn on_macos_only_the_browser_s_own_keychain_entry_is_read() {
        let services = |family: &str, own_only: bool| key_entries(family, own_only).into_iter().map(|(_, _, s)| s).collect::<Vec<_>>();
        // The Keychain asks about each entry read: only the browser's own.
        assert_eq!(services("Edge", true), ["Microsoft Edge Safe Storage"]);
        assert_eq!(services("Brave", true), ["Brave Safe Storage"]);
        // The keyring (Linux) asks nothing per entry: its own first, then the others'.
        assert_eq!(services("Edge", false), ["Microsoft Edge Safe Storage", "Chrome Safe Storage", "Chromium Safe Storage", "Brave Safe Storage"]);
        assert_eq!(KEY_READS.one_at_a_time, cfg!(target_os = "macos"));
        assert_eq!(KEY_READS.timeout, if cfg!(target_os = "macos") { Duration::from_secs(60) } else { STEP_TIMEOUT });
    }

    fn team(id: &str, email: &str) -> LoggedInTeam {
        LoggedInTeam { team_id: id.into(), team_name: format!("team {id}"), domain: id.to_lowercase(), url: format!("https://{}.slack.com/", id.to_lowercase()), email: email.into() }
    }

    #[test]
    fn a_page_without_the_list_is_a_problem() {
        let (teams, problem) = teams_of(vec![Ok(None)]);
        assert!(teams.is_empty());
        assert_eq!(problem.as_deref(), Some(PAGE_CHANGED));
        // A signed-out cookie's page has an empty list: nothing to show, and no problem.
        assert_eq!(teams_of(vec![Ok(Some(vec![]))]), (vec![], None));
        assert_eq!(teams_of(vec![Err("Slack did not answer in 10 s".into())]).1.as_deref(), Some("Slack did not answer in 10 s"));
        // Another cookie's page that worked wins; each account once, the freshest cookie's.
        let a = team("T0000000001", "person@example.com");
        let b = team("T0000000002", "person@example.com");
        let (teams, problem) = teams_of(vec![Ok(None), Ok(Some(vec![a.clone()])), Ok(Some(vec![a.clone(), b.clone()]))]);
        assert_eq!(teams, vec![(a, 1), (b, 2)]);
        assert_eq!(problem, None);
    }

    #[tokio::test]
    async fn at_most_six_checks_run_at_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let limit = tokio::sync::Semaphore::new(CHECKS_AT_ONCE);
        let (now, most) = (AtomicUsize::new(0), AtomicUsize::new(0));
        let steps = (0..20).map(|_| {
            limited(&limit, async {
                let n = now.fetch_add(1, Ordering::SeqCst) + 1;
                most.fetch_max(n, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(20)).await;
                now.fetch_sub(1, Ordering::SeqCst);
            })
        });
        let done = futures_util::future::join_all(steps).await;
        assert!(done.iter().all(|d| d.is_ok()));
        assert_eq!(most.load(Ordering::SeqCst), CHECKS_AT_ONCE);
    }

    #[test]
    fn chrome_is_not_listed_on_windows() {
        let profile = |source, label: &str| BrowserProfile { source, dir: PathBuf::from(label), label: label.into(), account: None };
        let all = vec![profile(Source::Chrome, "Chrome · Work"), profile(Source::Firefox, "Firefox · default"), profile(Source::Chrome, "Brave · Default")];
        let labels = |list: Vec<BrowserProfile>| list.into_iter().map(|p| p.label).collect::<Vec<_>>();
        assert_eq!(labels(listed(all.clone(), true)), ["Firefox · default"]);
        assert_eq!(labels(listed(all, false)), ["Chrome · Work", "Firefox · default", "Brave · Default"]);
    }

    #[test]
    fn stamp_follows_the_wal() {
        let dir = temp("stamp");
        let p = BrowserProfile { source: Source::Firefox, dir: dir.clone(), label: "Firefox · default".into(), account: None };
        assert_eq!(stamp(&p), None, "no cookie database yet");
        let t0 = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        std::fs::File::create(dir.join("cookies.sqlite")).unwrap().set_modified(t0).unwrap();
        assert_eq!(stamp(&p), Some(t0));
        // The browser writes its WAL later: the profile's stamp is the WAL's.
        let t1 = t0 + Duration::from_secs(10);
        std::fs::File::create(dir.join("cookies.sqlite-wal")).unwrap().set_modified(t1).unwrap();
        assert_eq!(stamp(&p), Some(t1));
        // And a journal newer still.
        let t2 = t1 + Duration::from_secs(10);
        std::fs::File::create(dir.join("cookies.sqlite-journal")).unwrap().set_modified(t2).unwrap();
        assert_eq!(stamp(&p), Some(t2));
        // Chrome's database is under Network/.
        let chrome = temp("stamp-chrome");
        std::fs::create_dir_all(chrome.join("Network")).unwrap();
        std::fs::File::create(chrome.join("Network/Cookies")).unwrap().set_modified(t1).unwrap();
        assert_eq!(stamp(&BrowserProfile { source: Source::Chrome, dir: chrome.clone(), label: "Chrome · Work".into(), account: None }), Some(t1));
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&chrome).unwrap();
    }

    #[tokio::test]
    async fn d_cookies_are_read_as_hashes_only() {
        let dir = temp("d-hashes");
        {
            let con = rusqlite::Connection::open(dir.join("cookies.sqlite")).unwrap();
            con.execute_batch("CREATE TABLE moz_cookies (id INTEGER PRIMARY KEY, name TEXT, value TEXT, host TEXT, lastAccessed INTEGER);
                INSERT INTO moz_cookies (name, value, host, lastAccessed) VALUES ('d', 'xoxd-one', '.slack.com', 1), ('d', 'xoxd-two', '.slack.com', 2), ('b', 'x', '.slack.com', 3);").unwrap();
        }
        let firefox = BrowserProfile { source: Source::Firefox, dir: dir.clone(), label: "Firefox · default".into(), account: None };
        let keys = Keys(HashMap::new());
        let got = d_hashes(&firefox, &keys).await.unwrap();
        assert_eq!(got, std::collections::HashSet::from([cookie_hash("xoxd-one"), cookie_hash("xoxd-two")]));
        // A Chrome-family profile whose browser has no key: not read, and said so.
        let chrome = BrowserProfile { source: Source::Chrome, dir: dir.clone(), label: "Chrome · Work".into(), account: None };
        assert!(!keys.has(&chrome));
        assert!(d_hashes(&chrome, &keys).await.is_err());
        let keys = Keys(HashMap::from([("Chrome".to_string(), Err("can't read Chrome's key: locked".to_string()))]));
        assert!(!keys.has(&chrome));
        let keys = Keys(HashMap::from([("Chrome".to_string(), Ok(b"pw".to_vec()))]));
        assert!(keys.has(&chrome) && keys.has(&firefox));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn chrome_here_is_only_a_windows_question() {
        if !cfg!(windows) {
            assert!(!unreadable_chrome_here());
        }
    }

    #[cfg(unix)]
    #[test]
    fn cookie_copies_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let profile = temp("copy-mode");
        let db = profile.join("cookies.sqlite");
        std::fs::write(&db, "db").unwrap();
        std::fs::write(profile.join("cookies.sqlite-wal"), "wal").unwrap();
        std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o644)).unwrap();
        let copy = Copy::of(&db).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&copy.dir), 0o700, "the folder");
        assert_eq!(mode(&copy.db), 0o600, "the database");
        assert_eq!(mode(&copy.dir.join("cookies.sqlite-wal")), 0o600, "its WAL");
        let dir = copy.dir.clone();
        drop(copy);
        assert!(!dir.exists());
        std::fs::remove_dir_all(&profile).unwrap();
    }

    /// Reads this machine's real browsers and checks every sign-in found, printing only counts.
    #[tokio::test]
    #[ignore = "reads the real browsers of this machine (prints counts only)"]
    async fn discover_real_browsers_counts_only() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let groups = discover().await;
        let signins: usize = groups.iter().map(|g| g.candidates.len()).sum();
        let working = groups.iter().flat_map(|g| &g.candidates).filter(|c| c.session.is_ok()).count();
        let problems = groups.iter().filter(|g| g.problem.is_some()).count();
        println!("profiles: {}, sign-ins: {signins}, working: {working}, profiles with a problem: {problems}", groups.len());
    }
}

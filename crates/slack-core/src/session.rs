//! Browser-session credentials (`xoxc-…` token + `d` cookie) and how to obtain them.
//!
//! The Slack web/desktop client authenticates API calls with a per-workspace `xoxc`
//! token plus the `d` session cookie (see `docs/protocol/auth.md`).
//! Tokens are never logged; session files are written owner-only.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

/// User-Agent of the official Linux desktop app; the web endpoints expect a browser.
pub const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Slack/4.52.178 Chrome/152.0.7977.130 Electron/44.4.5 Safari/537.36 Sonic Slack_SSB/4.52.178";

/// Credentials for one workspace.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceSession {
    pub team_id: String,
    pub team_name: String,
    /// e.g. `https://acme.slack.com/`
    pub url: String,
    pub user_id: String,
    pub user_name: String,
    pub token: String,
    /// Raw value of the `d` cookie as sent by browsers (already URL-encoded).
    pub cookie_d: String,
    /// Where the session came from, to offer it first when signing in again (none in files
    /// written before klack remembered it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SessionSource>,
}

/// How a session was obtained: names and paths only, nothing secret.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionSource {
    /// Taken from a browser profile the user is signed in with: `browser` is `firefox` or
    /// `chrome`, `profile` the profile's folder, `label` how it is shown ("Chrome · Work").
    Browser { browser: String, profile: PathBuf, label: String },
    /// Signed in in klack's temporary browser window.
    Window,
    /// The `d` cookie given by hand (flag, environment or prompt).
    Cookie,
}

impl std::fmt::Debug for WorkspaceSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceSession")
            .field("team_id", &self.team_id)
            .field("team_name", &self.team_name)
            .field("url", &self.url)
            .field("user_id", &self.user_id)
            .field("user_name", &self.user_name)
            .field("token", &"<redacted>")
            .field("cookie_d", &"<redacted>")
            .field("source", &self.source)
            .finish()
    }
}

impl WorkspaceSession {
    pub fn api_base(&self) -> String {
        format!("{}/api/", self.url.trim_end_matches('/'))
    }

    pub fn cookie_header(&self) -> String {
        format!("d={}", self.cookie_d)
    }
}

/// All workspaces of one profile.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Profile {
    pub workspaces: Vec<WorkspaceSession>,
    /// Team id opened at start-up.
    #[serde(default)]
    pub default_team: Option<String>,
}

/// klack's folders, plain "klack" on every platform: `~/.config/klack`, `~/.cache/klack` and
/// `~/.local/share/klack` on Linux, `~/Library/{Application Support,Caches}/klack` on macOS,
/// `%APPDATA%\klack` and `%LOCALAPPDATA%\klack` on Windows.
pub fn project_dirs() -> Option<directories::ProjectDirs> {
    directories::ProjectDirs::from_path(PathBuf::from("klack"))
}

/// Move the folders of the old name ("slacktui", under the `io.github.rslack` qualifier on
/// macOS and Windows) to klack's, where klack has none yet; returns what was moved, for the log.
pub fn adopt_old_dirs() -> Vec<String> {
    let (Some(old), Some(new)) = (directories::ProjectDirs::from("io.github", "rslack", "slacktui"), project_dirs()) else { return vec![] };
    adopt(&[(old.config_dir(), new.config_dir()), (old.cache_dir(), new.cache_dir()), (old.data_local_dir(), new.data_local_dir()), (old.data_dir(), new.data_dir())])
}

/// Rename each existing `old` folder to its `new` one when that does not exist yet.
fn adopt(pairs: &[(&Path, &Path)]) -> Vec<String> {
    let mut moved = Vec::new();
    for (old, new) in pairs {
        if !old.is_dir() || new.exists() {
            continue;
        }
        let result = new.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::rename(old, new));
        moved.push(match result {
            Ok(()) => format!("moved {} to {}", old.display(), new.display()),
            Err(e) => format!("could not move {} to {}: {e}", old.display(), new.display()),
        });
    }
    moved
}

pub fn config_dir(profile: &str) -> Result<PathBuf> {
    let dirs = project_dirs().ok_or_else(|| anyhow!("no home directory"))?;
    Ok(dirs.config_dir().join(profile))
}

pub fn cache_dir(profile: &str) -> Result<PathBuf> {
    let dirs = project_dirs().ok_or_else(|| anyhow!("no home directory"))?;
    Ok(dirs.cache_dir().join(profile))
}

impl Profile {
    fn path(profile: &str) -> Result<PathBuf> {
        Ok(config_dir(profile)?.join("session.json"))
    }

    pub fn load(profile: &str) -> Result<Self> {
        let path = Self::path(profile)?;
        let text = std::fs::read_to_string(&path).with_context(|| format!("no session for profile '{profile}' — run `klack auth login`"))?;
        Ok(serde_json::from_str(&text)?)
    }

    /// Read a session file, or a new empty profile when there is none; a file that exists but
    /// cannot be read is an error, so that saving does not replace its sessions.
    pub fn read_or_new(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Profile::default());
        }
        Self::read(path).with_context(|| format!("cannot read {}", path.display()))
    }

    /// Read a session file (another profile's, for `klack profile list`).
    pub fn read(path: &Path) -> Result<Self> {
        Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
    }

    pub fn save(&self, profile: &str) -> Result<PathBuf> {
        let path = Self::path(profile)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        write_private(&path, serde_json::to_string_pretty(self)?.as_bytes())?;
        Ok(path)
    }

    /// Insert or replace the session of its account (team + user: one profile may hold two
    /// accounts of one workspace).
    /// The account keeps its place, so the one started and the Alt+1…9 order stay.
    pub fn upsert(&mut self, ws: WorkspaceSession) {
        if self.default_team.is_none() {
            self.default_team = Some(ws.team_id.clone());
        }
        match self.workspaces.iter_mut().find(|w| w.team_id == ws.team_id && w.user_id == ws.user_id) {
            Some(old) => *old = ws,
            None => self.workspaces.push(ws),
        }
    }

    /// Every account of the workspace `team` (its id or name).
    pub fn matching(&self, team: &str) -> Vec<&WorkspaceSession> {
        self.workspaces.iter().filter(|w| w.team_id == team || w.team_name == team).collect()
    }

    /// Forget the account `user_id` of `team_id`; false when there is none. When its team was the
    /// default and has no account left, the first remaining workspace becomes the default.
    pub fn remove(&mut self, team_id: &str, user_id: &str) -> bool {
        let before = self.workspaces.len();
        self.workspaces.retain(|w| w.team_id != team_id || w.user_id != user_id);
        if self.default_team.as_deref().is_some_and(|t| !self.workspaces.iter().any(|w| w.team_id == t)) {
            self.default_team = self.workspaces.first().map(|w| w.team_id.clone());
        }
        self.workspaces.len() != before
    }

    pub fn workspace(&self, team: Option<&str>) -> Option<&WorkspaceSession> {
        let team = team.or(self.default_team.as_deref());
        match team {
            Some(t) => self.workspaces.iter().find(|w| w.team_id == t || w.team_name == t),
            None => self.workspaces.first(),
        }
    }
}

/// Write a file readable only by the owner.
/// Write `bytes` to `path` readable by the owner only (atomically, through a temporary file).
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    {
        #[cfg(unix)]
        let mut f = {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?
        };
        #[cfg(not(unix))]
        let mut f = std::fs::File::create(&tmp)?;
        std::io::Write::write_all(&mut f, bytes)?;
    }
    std::fs::rename(tmp, path)?;
    Ok(())
}

/// Extract the web-client token embedded in a server-rendered workspace page.
pub fn extract_api_token(html: &str) -> Option<String> {
    let re = regex::Regex::new(r#""api_token":"(xoxc-[^"]+)""#).ok()?;
    re.captures(html).map(|c| c[1].to_string())
}

/// Fetch the `xoxc` token for a workspace using the `d` cookie, then confirm it with
/// `auth.test`.
pub async fn session_from_cookie(workspace_url: &str, cookie_d: &str) -> Result<WorkspaceSession> {
    session_from_cookie_with(&web_client()?, workspace_url, cookie_d).await
}

/// The client for Slack's web pages (a workspace's pages, the sign-in page): a browser's user agent.
/// One can be shared by many requests, as finding the sign-ins of every browser does.
pub fn web_client() -> Result<reqwest::Client> {
    crate::api::http_client(|| reqwest::Client::builder().user_agent(USER_AGENT))
}

/// [`session_from_cookie`] with the client `http`.
pub async fn session_from_cookie_with(http: &reqwest::Client, workspace_url: &str, cookie_d: &str) -> Result<WorkspaceSession> {
    let base = workspace_url.trim_end_matches('/');
    let html = http
        .get(format!("{base}/customize/emoji"))
        .header(reqwest::header::COOKIE, format!("d={cookie_d}"))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let token = extract_api_token(&html).ok_or_else(|| anyhow!("signed-out or unexpected page (no api_token) — is the session still valid?"))?;
    let resp: serde_json::Value = http
        .post(format!("{base}/api/auth.test"))
        .header(reqwest::header::COOKIE, format!("d={cookie_d}"))
        .form(&[("token", token.as_str())])
        .send()
        .await?
        .json()
        .await?;
    if resp["ok"].as_bool() != Some(true) {
        bail!("auth.test failed: {}", resp["error"].as_str().unwrap_or("unknown"));
    }
    let s = |k: &str| resp[k].as_str().unwrap_or_default().to_string();
    Ok(WorkspaceSession {
        team_id: s("team_id"),
        team_name: s("team"),
        url: s("url"),
        user_id: s("user_id"),
        user_name: s("user"),
        token,
        cookie_d: cookie_d.to_string(),
        source: None,
    })
}

/// A workspace a `d` cookie is signed in to, as Slack's sign-in page lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoggedInTeam {
    pub team_id: String,
    pub team_name: String,
    /// The workspace's address name: `acme` of `acme.slack.com`.
    pub domain: String,
    /// e.g. `https://acme.slack.com/`
    pub url: String,
    /// The email the account signs in with.
    pub email: String,
}

/// Slack's sign-in page; seen with a `d` cookie it lists every workspace that cookie is signed in to.
const SIGNIN_URL: &str = "https://slack.com/signin";

/// The workspaces Slack's sign-in page lists as signed in: its JSON (HTML-escaped inside the
/// page) has `"loggedInTeams":[{team_id, team_name, team_domain, user_email, …}]`. A signed-out or
/// unexpected page gives none.
pub fn parse_logged_in_teams(html: &str) -> Vec<LoggedInTeam> {
    find_logged_in_teams(html).unwrap_or_default()
}

/// Like [`parse_logged_in_teams`], but a page without the list (Slack changed its page) is `None`:
/// a signed-out page still has it, empty.
pub fn find_logged_in_teams(html: &str) -> Option<Vec<LoggedInTeam>> {
    const NAME: &str = "loggedInTeams";
    for (at, _) in html.match_indices(NAME) {
        let rest = unescape_html(&html[at + NAME.len()..]);
        // `":[…]` after the name (anything else, like `loggedInTeamsCount`, is not the list).
        let Some(list) = rest.trim_start_matches(['"', '\\']).trim_start().strip_prefix(':') else { continue };
        let list = list.trim_start();
        if !list.starts_with('[') {
            continue;
        }
        let Some(Ok(entries)) = serde_json::Deserializer::from_str(list).into_iter::<Vec<serde_json::Value>>().next() else { continue };
        let teams = entries
            .iter()
            .filter_map(|t| {
                let s = |k: &str| t[k].as_str().unwrap_or_default().to_string();
                let (team_id, domain) = (s("team_id"), s("team_domain"));
                if team_id.is_empty() || domain.is_empty() {
                    return None;
                }
                Some(LoggedInTeam { team_id, team_name: s("team_name"), url: format!("https://{domain}.slack.com/"), domain, email: s("user_email") })
            })
            .collect();
        return Some(teams);
    }
    None
}

/// Undo the HTML escaping of an attribute value (`&quot;` and friends; `&amp;` last, so that an
/// escaped `&amp;quot;` stays `&quot;`).
fn unescape_html(text: &str) -> String {
    let mut out = text.to_string();
    for (from, to) in [("&quot;", "\""), ("&#34;", "\""), ("&#x22;", "\""), ("&#39;", "'"), ("&#x27;", "'"), ("&apos;", "'"), ("&lt;", "<"), ("&gt;", ">"), ("&#x2F;", "/"), ("&#47;", "/"), ("&amp;", "&")] {
        if out.contains(from) {
            out = out.replace(from, to);
        }
    }
    out
}

/// The workspaces the `d` cookie `cookie_d` is signed in to, from Slack's sign-in page.
pub async fn signed_in_teams(cookie_d: &str) -> Result<Vec<LoggedInTeam>> {
    Ok(signed_in_teams_with(&web_client()?, cookie_d).await?.unwrap_or_default())
}

/// [`signed_in_teams`] with the client `http`; `None` when the page has no list of workspaces at
/// all (see [`find_logged_in_teams`]).
pub async fn signed_in_teams_with(http: &reqwest::Client, cookie_d: &str) -> Result<Option<Vec<LoggedInTeam>>> {
    let html = http.get(SIGNIN_URL).header(reqwest::header::COOKIE, format!("d={cookie_d}")).send().await?.error_for_status()?.text().await?;
    Ok(find_logged_in_teams(&html))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_folders_are_adopted_once() {
        let base = std::env::temp_dir().join(format!("klack-adopt-test-{}", std::process::id()));
        let (old, new) = (base.join("old/slacktui"), base.join("new/klack"));
        std::fs::create_dir_all(old.join("default")).unwrap();
        std::fs::write(old.join("default/session.json"), "{}").unwrap();
        let moved = adopt(&[(old.as_path(), new.as_path())]);
        assert_eq!(moved.len(), 1, "{moved:?}");
        assert!(new.join("default/session.json").is_file() && !old.exists());
        // Nothing left to move, and an existing new folder is never overwritten.
        assert!(adopt(&[(old.as_path(), new.as_path())]).is_empty());
        std::fs::create_dir_all(&old).unwrap();
        assert!(adopt(&[(old.as_path(), new.as_path())]).is_empty());
        assert!(new.join("default/session.json").is_file());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn extracts_token() {
        let html = r#"<script>var boot = {"api_token":"xoxc-123-456-abc","x":1}</script>"#;
        assert_eq!(extract_api_token(html).as_deref(), Some("xoxc-123-456-abc"));
        assert_eq!(extract_api_token("<html>signin</html>"), None);
    }

    #[test]
    fn debug_redacts_secrets() {
        let ws = WorkspaceSession {
            team_id: "T1".into(),
            team_name: "t".into(),
            url: "https://t.slack.com/".into(),
            user_id: "U1".into(),
            user_name: "u".into(),
            token: "xoxc-secret".into(),
            cookie_d: "cookie-secret".into(),
            source: None,
        };
        let dbg = format!("{ws:?}");
        assert!(!dbg.contains("secret"));
        assert_eq!(ws.api_base(), "https://t.slack.com/api/");
    }

    fn account(team: &str, name: &str, user: &str) -> WorkspaceSession {
        WorkspaceSession { team_id: team.into(), team_name: name.into(), user_id: user.into(), user_name: format!("person-{user}"), ..Default::default() }
    }

    #[test]
    fn two_accounts_of_one_team_are_kept() {
        let mut p = Profile::default();
        p.upsert(account("T0000000001", "acme", "U0000000001"));
        p.upsert(account("T0000000001", "acme", "U0000000002"));
        assert_eq!(p.workspaces.len(), 2);
        assert_eq!(p.default_team.as_deref(), Some("T0000000001"));
        // The first saved is the one started.
        assert_eq!(p.workspace(None).unwrap().user_id, "U0000000001");
    }

    #[test]
    fn the_same_account_is_replaced() {
        let mut p = Profile::default();
        p.upsert(account("T0000000001", "acme", "U0000000001"));
        p.upsert(account("T0000000002", "other", "U0000000001"));
        p.upsert(WorkspaceSession { source: Some(SessionSource::Cookie), ..account("T0000000001", "acme", "U0000000001") });
        assert_eq!(p.workspaces.len(), 2);
        let acme = p.matching("acme");
        assert_eq!(acme.len(), 1);
        assert_eq!(acme[0].source, Some(SessionSource::Cookie));
        // Forgetting an account touches only that team + user.
        assert!(!p.remove("T0000000001", "U0000000009"));
        assert!(p.remove("T0000000001", "U0000000001"));
        assert_eq!(p.workspaces.len(), 1);
        // The default no longer names a team without accounts.
        assert_eq!(p.default_team.as_deref(), Some("T0000000002"));
        assert!(p.remove("T0000000002", "U0000000001"));
        assert_eq!(p.default_team, None);
    }

    #[test]
    fn signing_in_again_keeps_the_order() {
        let mut p = Profile::default();
        p.upsert(account("T0000000001", "acme", "U0000000001"));
        p.upsert(account("T0000000001", "acme", "U0000000002"));
        p.upsert(account("T0000000002", "other", "U0000000001"));
        p.upsert(WorkspaceSession { source: Some(SessionSource::Cookie), ..account("T0000000001", "acme", "U0000000001") });
        let order: Vec<(&str, &str)> = p.workspaces.iter().map(|w| (w.team_id.as_str(), w.user_id.as_str())).collect();
        assert_eq!(order, [("T0000000001", "U0000000001"), ("T0000000001", "U0000000002"), ("T0000000002", "U0000000001")]);
        assert_eq!(p.workspace(None).unwrap().user_id, "U0000000001");
        assert_eq!(p.workspaces[0].source, Some(SessionSource::Cookie));
    }

    #[test]
    fn a_broken_session_file_is_not_replaced() {
        let dir = std::env::temp_dir().join(format!("klack-session-read-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // No file: a new, empty profile.
        assert!(Profile::read_or_new(&dir.join("session.json")).unwrap().workspaces.is_empty());
        std::fs::write(dir.join("session.json"), "{ not json").unwrap();
        assert!(Profile::read_or_new(&dir.join("session.json")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn old_session_files_still_load() {
        let text = r#"{"workspaces":[
            {"team_id":"T0000000001","team_name":"acme","url":"https://acme.slack.com/","user_id":"U0000000001","user_name":"one","token":"xoxc-made-up","cookie_d":"made-up"},
            {"team_id":"T0000000002","team_name":"other","url":"https://other.slack.com/","user_id":"U0000000001","user_name":"one","token":"xoxc-made-up","cookie_d":"made-up"},
            {"team_id":"T0000000001","team_name":"acme","url":"https://acme.slack.com/","user_id":"U0000000002","user_name":"two","token":"xoxc-made-up","cookie_d":"made-up"}
        ],"default_team":"T0000000001"}"#;
        let p: Profile = serde_json::from_str(text).unwrap();
        assert!(p.workspaces.iter().all(|w| w.source.is_none()));
        let start = p.workspace(None).unwrap();
        assert_eq!((start.team_id.as_str(), start.user_id.as_str()), ("T0000000001", "U0000000001"));
        let users: Vec<&str> = p.matching("acme").iter().map(|w| w.user_id.as_str()).collect();
        assert_eq!(users, ["U0000000001", "U0000000002"]);
        assert_eq!(p.matching("T0000000002").len(), 1);
        assert!(p.matching("nope").is_empty());
        // Saving it again writes no source for the old ones.
        assert!(!serde_json::to_string(&p).unwrap().contains("source"));
    }

    #[test]
    fn source_round_trips() {
        let sources = [
            SessionSource::Browser { browser: "chrome".into(), profile: PathBuf::from("/home/person/.config/google-chrome/Profile 1"), label: "Chrome · Work".into() },
            SessionSource::Window,
            SessionSource::Cookie,
        ];
        for source in sources {
            let ws = WorkspaceSession { source: Some(source.clone()), ..account("T0000000001", "acme", "U0000000001") };
            let text = serde_json::to_string(&ws).unwrap();
            let back: WorkspaceSession = serde_json::from_str(&text).unwrap();
            assert_eq!(back.source, Some(source), "{text}");
        }
        let text = serde_json::to_string(&WorkspaceSession { source: Some(SessionSource::Window), ..Default::default() }).unwrap();
        assert!(text.contains(r#""source":{"kind":"window"}"#), "{text}");
    }

    #[test]
    fn the_signin_page_names_every_workspace() {
        let teams = parse_logged_in_teams(include_str!("../tests/fixtures/signin-page.html"));
        let acme = LoggedInTeam { team_id: "T0000000001".into(), team_name: "Acme & Sons".into(), domain: "acme".into(), url: "https://acme.slack.com/".into(), email: "person@example.com".into() };
        let widgets = LoggedInTeam { team_id: "T0000000002".into(), team_name: "Widgets Inc".into(), domain: "widgets-inc".into(), url: "https://widgets-inc.slack.com/".into(), email: "person@example.com".into() };
        assert_eq!(teams, [acme, widgets]);
    }

    #[test]
    fn a_page_without_the_list_is_not_a_signed_out_page() {
        // Signed out (checked 2026-10-20): the list is there, empty.
        assert_eq!(find_logged_in_teams(r#"<div data-props="{&quot;loggedInTeams&quot;:[],&quot;x&quot;:1}"></div>"#), Some(vec![]));
        assert_eq!(find_logged_in_teams(include_str!("../tests/fixtures/signin-page.html")).map(|t| t.len()), Some(2));
        // No list at all, or one that cannot be read: Slack changed its page.
        assert_eq!(find_logged_in_teams("<html><title>Sign in | Slack</title></html>"), None);
        assert_eq!(find_logged_in_teams("loggedInTeams&quot;:[{&quot;team_id&quot;:&quot;T0000000001"), None);
        assert_eq!(find_logged_in_teams(r#"{"loggedInTeamsCount":2}"#), None);
    }

    #[test]
    fn a_page_without_teams_gives_none() {
        // Signed out: an empty list, or none at all.
        assert!(parse_logged_in_teams(r#"<div data-props="{&quot;loggedInTeams&quot;:[],&quot;x&quot;:1}"></div>"#).is_empty());
        assert!(parse_logged_in_teams("<html><title>Sign in | Slack</title></html>").is_empty());
        // Garbage: a cut-off list, not a list, entries without an id or domain.
        assert!(parse_logged_in_teams("loggedInTeams&quot;:[{&quot;team_id&quot;:&quot;T0000000001").is_empty());
        assert!(parse_logged_in_teams(r#"{"loggedInTeams":{"team_id":"T0000000001"}}"#).is_empty());
        assert!(parse_logged_in_teams(r#"{"loggedInTeams":[{"team_name":"x"},{"team_id":"T0000000001"},7]}"#).is_empty());
        assert!(parse_logged_in_teams("loggedInTeams").is_empty());
    }
}

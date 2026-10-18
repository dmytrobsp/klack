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

pub fn config_dir(profile: &str) -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("io.github", "rslack", "slacktui").ok_or_else(|| anyhow!("no home directory"))?;
    Ok(dirs.config_dir().join(profile))
}

pub fn cache_dir(profile: &str) -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("io.github", "rslack", "slacktui").ok_or_else(|| anyhow!("no home directory"))?;
    Ok(dirs.cache_dir().join(profile))
}

impl Profile {
    fn path(profile: &str) -> Result<PathBuf> {
        Ok(config_dir(profile)?.join("session.json"))
    }

    pub fn load(profile: &str) -> Result<Self> {
        let path = Self::path(profile)?;
        let text = std::fs::read_to_string(&path).with_context(|| format!("no session for profile '{profile}' — run `slacktui login`"))?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn save(&self, profile: &str) -> Result<PathBuf> {
        let path = Self::path(profile)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        write_private(&path, serde_json::to_string_pretty(self)?.as_bytes())?;
        Ok(path)
    }

    /// Insert or replace the session for its team.
    pub fn upsert(&mut self, ws: WorkspaceSession) {
        self.workspaces.retain(|w| w.team_id != ws.team_id);
        if self.default_team.is_none() {
            self.default_team = Some(ws.team_id.clone());
        }
        self.workspaces.push(ws);
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
    let base = workspace_url.trim_end_matches('/');
    let http = reqwest::Client::builder().user_agent(USER_AGENT).build()?;
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        };
        let dbg = format!("{ws:?}");
        assert!(!dbg.contains("secret"));
        assert_eq!(ws.api_base(), "https://t.slack.com/api/");
    }
}

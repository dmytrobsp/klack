//! Signing in with the code Slack emails: the sequence of Web API calls that turns an
//! emailed code into a session (docs/superpowers/specs/2026-10-20-sign-in-anywhere-design.md).
//! Nothing here asks Slack to send a code (that needs a captcha) and nothing prints a secret.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use reqwest::cookie::{CookieStore, Jar};
use serde_json::Value;

use crate::session::{self, SessionSource, WorkspaceSession};

/// Slack's sign-in page, where the person asks for the code.
pub const SIGNIN_PAGE: &str = "https://slack.com/signin";

/// The sign-in page with `email` already typed in.
pub fn signin_link(email: &str) -> String {
    let mut out = format!("{SIGNIN_PAGE}?email=");
    for b in email.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A code as people type it (`REP-OEU`, `rep oeu`) as Slack takes it (`REPOEU`); `None` unless it is six letters or digits.
pub fn normalize_code(typed: &str) -> Option<String> {
    let code: String = typed.chars().filter(|c| !c.is_whitespace() && !matches!(c, '-' | '‐' | '–' | '—')).flat_map(char::to_uppercase).collect();
    (code.len() == 6 && code.chars().all(|c| c.is_ascii_alphanumeric())).then_some(code)
}

/// Whether `typed` looks like an email address: text, one `@`, then a name with a dot in it.
pub fn email_ok(typed: &str) -> bool {
    let typed = typed.trim();
    match typed.split_once('@') {
        Some((user, host)) => !user.is_empty() && !host.contains('@') && host.contains('.') && !host.starts_with('.') && !host.ends_with('.'),
        None => false,
    }
}

/// Why a code was not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeError {
    Failed,
    Expired,
    RateLimited,
    Other(String),
}

impl std::fmt::Display for CodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodeError::Failed => write!(f, "that code didn't work — type it again, or ask for a new one on the phone"),
            CodeError::Expired => write!(f, "that code has expired — ask for a new one on the phone"),
            CodeError::RateLimited => write!(f, "Slack says too many tries — wait a while, then ask for a new code"),
            CodeError::Other(e) => write!(f, "Slack refused the code: {e}"),
        }
    }
}

impl std::error::Error for CodeError {}

/// `signin.confirmCode`'s answer: whether the email has a workspace.
pub fn parse_confirm(reply: &Value) -> Result<bool, CodeError> {
    if reply["ok"].as_bool() == Some(true) {
        return Ok(reply["has_workspace"].as_bool().unwrap_or(false));
    }
    Err(match reply["error"].as_str() {
        Some("failed") => CodeError::Failed,
        Some("expired") => CodeError::Expired,
        Some("ratelimited") => CodeError::RateLimited,
        Some(other) => CodeError::Other(other.to_string()),
        None => CodeError::Other("no answer".to_string()),
    })
}

/// A workspace the confirmed email can sign in to.
#[derive(Clone, PartialEq, Eq)]
pub struct CodeTeam {
    pub team_id: String,
    pub name: String,
    pub domain: String,
    /// e.g. `https://acme.slack.com/`
    pub url: String,
    /// One-time sign-in link: a secret.
    pub magic_login_url: String,
    pub sso_required: bool,
    pub two_factor_required: bool,
}

impl std::fmt::Debug for CodeTeam {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodeTeam")
            .field("team_id", &self.team_id)
            .field("name", &self.name)
            .field("domain", &self.domain)
            .field("url", &self.url)
            .field("magic_login_url", &"<redacted>")
            .field("sso_required", &self.sso_required)
            .field("two_factor_required", &self.two_factor_required)
            .finish()
    }
}

fn team_from(v: &Value) -> Option<CodeTeam> {
    let text = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    let (team_id, magic_login_url) = (text("id"), text("magic_login_url"));
    if team_id.is_empty() || magic_login_url.is_empty() {
        return None;
    }
    let domain = text("domain");
    let url = session::workspace_url(&text("url")).unwrap_or_else(|_| format!("https://{domain}.slack.com/"));
    Some(CodeTeam { team_id, name: text("name"), domain, url, magic_login_url, sso_required: v["sso_required"].as_bool().unwrap_or(false), two_factor_required: v["two_factor_required"].as_bool().unwrap_or(false) })
}

/// `signin.findWorkspaces`' answer: `current_teams`, then what the `current_orgs` hold.
pub fn parse_workspaces(reply: &Value) -> Result<Vec<CodeTeam>> {
    if reply["ok"].as_bool() != Some(true) {
        return Err(anyhow!("Slack refused to list the workspaces: {}", reply["error"].as_str().unwrap_or("no answer")));
    }
    let mut candidates: Vec<&Value> = reply["current_teams"].as_array().into_iter().flatten().collect();
    // UNVERIFIED (no Grid account to try): an org is assumed to carry its workspaces in a `teams`
    // array, and to be a sign-in target itself when it has a `magic_login_url`.
    for org in reply["current_orgs"].as_array().into_iter().flatten() {
        match org["teams"].as_array() {
            Some(teams) => candidates.extend(teams),
            None if org["magic_login_url"].is_string() => candidates.push(org),
            None => {}
        }
    }
    let mut out: Vec<CodeTeam> = Vec::new();
    for team in candidates.into_iter().filter_map(team_from) {
        if !out.iter().any(|t| t.team_id == team.team_id) {
            out.push(team);
        }
    }
    Ok(out)
}

/// The value of cookie `name` in a `Cookie:` header (`a=1; d=xoxd-…; x=2`).
pub fn cookie_value(header: &str, name: &str) -> Option<String> {
    header.split(';').filter_map(|part| part.trim().split_once('=')).find(|(n, _)| *n == name).map(|(_, v)| v.to_string())
}

/// One run of the code sign-in: a client with its own cookie jar, kept from the sign-in page to the magic link.
pub struct CodeSignIn {
    email: String,
    http: reqwest::Client,
    jar: Arc<Jar>,
    /// `https://slack.com`; another one only in tests.
    base: String,
}

impl std::fmt::Debug for CodeSignIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodeSignIn").field("email", &self.email).finish()
    }
}

fn unexpected(status: impl std::fmt::Display) -> CodeError {
    CodeError::Other(format!("Slack did not answer as expected (HTTP {status})"))
}

/// An error of a call made after the code was accepted: Slack's answer alone, not "refused the code".
fn after_the_code(e: CodeError) -> anyhow::Error {
    match e {
        CodeError::Other(m) => anyhow!(m),
        e => anyhow!(e),
    }
}

impl CodeSignIn {
    fn new(email: &str, base: &str) -> Result<CodeSignIn> {
        let jar = Arc::new(Jar::default());
        let http = crate::api::http_client(|| reqwest::Client::builder().user_agent(session::USER_AGENT).cookie_provider(jar.clone()).connect_timeout(Duration::from_secs(15)).timeout(Duration::from_secs(30)))?;
        Ok(CodeSignIn { email: email.trim().to_string(), http, jar, base: base.to_string() })
    }

    /// Test only: talk to a local server instead of Slack.
    #[cfg(test)]
    fn at(email: &str, base: &str) -> Result<CodeSignIn> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        CodeSignIn::new(email, base)
    }

    /// Opens Slack's sign-in page, which hands out the cookies the next calls need.
    pub async fn start(email: &str) -> Result<CodeSignIn> {
        let signin = CodeSignIn::new(email, "https://slack.com")?;
        signin.http.get(format!("{}/signin", signin.base)).send().await?.error_for_status()?;
        Ok(signin)
    }

    pub fn email(&self) -> &str {
        &self.email
    }

    /// Asks Slack whether `code` (see [`normalize_code`]) is the one it emailed; `Ok` says whether the email has a workspace.
    pub async fn confirm(&self, code: &str) -> Result<bool, CodeError> {
        let form = reqwest::multipart::Form::new().text("email", self.email.clone()).text("code", code.to_string());
        parse_confirm(&self.call("signin.confirmCode", form).await?)
    }

    /// The workspaces of the confirmed email.
    pub async fn workspaces(&self) -> Result<Vec<CodeTeam>> {
        let form = reqwest::multipart::Form::new().text("email", self.email.clone()).text("reason", "get_started_workspaces");
        parse_workspaces(&self.call("signin.findWorkspaces", form).await.map_err(after_the_code)?)
    }

    /// Follows the team's magic link, which sets the `d` cookie, and turns that into a session.
    pub async fn redeem(&self, team: &CodeTeam) -> Result<WorkspaceSession> {
        // The link is a one-time secret and reqwest's errors print the URL, so the error is rebuilt without it.
        self.http.get(&team.magic_login_url).send().await.map_err(|e| anyhow!("Slack did not sign in to {}: {}", team.name, e.without_url()))?;
        let header = self.jar.cookies(&"https://slack.com/".parse()?).and_then(|h| h.to_str().ok().map(str::to_string)).unwrap_or_default();
        let d = cookie_value(&header, "d").ok_or_else(|| anyhow!("Slack did not sign in to {}", team.name))?;
        // A client without the jar, so `session_from_cookie_with` sends the only Cookie header.
        let session = session::session_from_cookie_with(&session::web_client()?, &team.url, &d).await.map_err(|e| anyhow!("Slack did not sign in to {}: {e:#}", team.name))?;
        Ok(WorkspaceSession { source: Some(SessionSource::Code { email: self.email.clone() }), ..session })
    }

    async fn call(&self, method: &str, form: reqwest::multipart::Form) -> Result<Value, CodeError> {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let reply = self.http.post(format!("{}/api/{method}?_x_id=noversion-{now}", self.base)).multipart(form).send().await.map_err(|_| unexpected("no answer"))?;
        let status = reply.status();
        if !status.is_success() {
            return Err(unexpected(status.as_u16()));
        }
        reply.json::<Value>().await.map_err(|_| unexpected(status.as_u16()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_link_fills_in_the_email() {
        assert_eq!(signin_link("you@gmail.com"), "https://slack.com/signin?email=you%40gmail.com");
        assert!(signin_link("a+b c@x.io").ends_with("a%2Bb%20c%40x.io"));
        assert!(signin_link("ü@x.io").ends_with("%C3%BC%40x.io"));
    }

    #[test]
    fn codes_are_taken_as_people_type_them() {
        for typed in ["REP-OEU", " rep-oeu ", "REP OEU", "rep–oeu", "repoeu"] {
            assert_eq!(normalize_code(typed), Some("REPOEU".to_string()), "{typed:?}");
        }
        for typed in ["REP-OE", "REP-OEUX", "RE?-OEU", ""] {
            assert_eq!(normalize_code(typed), None, "{typed:?}");
        }
    }

    #[test]
    fn emails_are_checked_loosely() {
        for ok in ["you@gmail.com", " a.b@c.co "] {
            assert!(email_ok(ok), "{ok:?}");
        }
        for bad in ["", "you", "@x.io", "you@x", "a@b@c.io"] {
            assert!(!email_ok(bad), "{bad:?}");
        }
    }

    #[test]
    fn confirm_replies_map_to_errors() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!("../tests/fixtures/confirm-code-ok.json")).unwrap();
        assert_eq!(parse_confirm(&fixture), Ok(true));
        assert_eq!(parse_confirm(&json!({"ok":true,"has_workspace":true})), Ok(true));
        assert_eq!(parse_confirm(&json!({"ok":false,"error":"failed"})), Err(CodeError::Failed));
        assert_eq!(parse_confirm(&json!({"ok":false,"error":"expired"})), Err(CodeError::Expired));
        assert_eq!(parse_confirm(&json!({"ok":false,"error":"ratelimited"})), Err(CodeError::RateLimited));
        assert_eq!(parse_confirm(&json!({"ok":false,"error":"captcha_failed"})), Err(CodeError::Other("captcha_failed".into())));
        assert!(matches!(parse_confirm(&json!({"ok":false})), Err(CodeError::Other(_))));
        assert_eq!(CodeError::Failed.to_string(), "that code didn't work — type it again, or ask for a new one on the phone");
        assert_eq!(CodeError::Expired.to_string(), "that code has expired — ask for a new one on the phone");
        assert_eq!(CodeError::RateLimited.to_string(), "Slack says too many tries — wait a while, then ask for a new code");
        assert_eq!(CodeError::Other("x".into()).to_string(), "Slack refused the code: x");
    }

    #[test]
    fn workspaces_come_from_teams_and_orgs() {
        let reply: serde_json::Value = serde_json::from_str(include_str!("../tests/fixtures/find-workspaces.json")).unwrap();
        let teams = parse_workspaces(&reply).unwrap();
        let ids: Vec<&str> = teams.iter().map(|t| t.team_id.as_str()).collect();
        assert_eq!(ids, ["T0000000001", "T0000000002", "T0000000004"]);
        assert_eq!(teams[0].name, "Acme & Sons");
        assert_eq!(teams[0].url, "https://acme.slack.com/");
        assert!(!teams[0].sso_required && !teams[0].two_factor_required);
        assert!(teams[1].sso_required && teams[1].two_factor_required);
        let text = format!("{:?}", teams[1]);
        assert!(!text.contains("magic-login"), "{text}");
        assert!(parse_workspaces(&json!({"ok":false,"error":"nope"})).unwrap_err().to_string().contains("nope"));
    }

    #[test]
    fn a_cookie_is_found_by_name() {
        assert_eq!(cookie_value("b=1; d=xoxd-a%2Fb; d-s=9", "d"), Some("xoxd-a%2Fb".to_string()));
        assert_eq!(cookie_value("b=1", "d"), None);
    }

    #[tokio::test]
    async fn a_page_that_is_not_json_says_so_without_its_body() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let _ = sock.read(&mut buf).await;
            let body = "<html>see https://secret.example/magic</html>";
            let reply = format!("HTTP/1.1 429 Too Many Requests\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            sock.write_all(reply.as_bytes()).await.unwrap();
        });
        let signin = CodeSignIn::at("person@example.com", &base).unwrap();
        match signin.confirm("AAAAAA").await {
            Err(CodeError::Other(m)) => assert!(m.contains("429") && !m.contains("secret"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn a_listing_error_is_not_called_a_refused_code() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let _ = sock.read(&mut buf).await;
            sock.write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
        });
        // The code was taken already: the listing's error is Slack's answer alone.
        let text = format!("{:#}", CodeSignIn::at("person@example.com", &base).unwrap().workspaces().await.unwrap_err());
        assert_eq!(text, "Slack did not answer as expected (HTTP 500)");
    }

    #[tokio::test]
    async fn a_failed_magic_link_does_not_show_the_link() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let signin = CodeSignIn::at("person@example.com", "http://127.0.0.1:9").unwrap();
        let team = CodeTeam { team_id: "T0000000001".into(), name: "Acme".into(), domain: "acme".into(), url: "https://acme.slack.com/".into(), magic_login_url: format!("http://127.0.0.1:{port}/magic-login/secret-token-123"), sso_required: false, two_factor_required: false };
        let text = format!("{:#}", signin.redeem(&team).await.unwrap_err());
        assert!(text.contains("Acme"), "{text}");
        assert!(!text.contains("secret") && !text.contains("magic-login") && !text.contains(&port.to_string()), "{text}");
    }

    /// Live: Slack's real sign-in page and a wrong code for a made-up address. Sends no email.
    /// Run with `cargo test -p slack-core -- --ignored a_wrong_code_is_refused_live --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn a_wrong_code_is_refused_live() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let signin = CodeSignIn::start("person@example.com").await.unwrap();
        let header = signin.jar.cookies(&"https://slack.com/".parse().unwrap()).map(|h| h.to_str().unwrap_or_default().to_string()).unwrap_or_default();
        let names: Vec<&str> = header.split(';').filter_map(|p| p.trim().split_once('=').map(|(n, _)| n)).collect();
        println!("cookie names after the sign-in page: {names:?}");
        assert!(names.contains(&"b") && names.contains(&"x"));
        match signin.confirm("AAAAAA").await {
            Err(e) => println!("confirm error variant: {}", match e { CodeError::Failed => "Failed".to_string(), CodeError::Expired => "Expired".to_string(), CodeError::RateLimited => "RateLimited".to_string(), CodeError::Other(m) => format!("Other({m})") }),
            Ok(b) => panic!("a wrong code was accepted: {b}"),
        }
    }
}

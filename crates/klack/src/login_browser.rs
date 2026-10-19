//! Signing in through a browser window (`klack login`): a Chromium-based browser opens on the
//! workspace's sign-in page with a temporary profile of its own; once Slack has signed you in, its
//! `d` cookie is read over the DevTools protocol, the window closes and the profile is deleted.
//! Your own browser profile is never read or changed.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use slack_core::session::{self, WorkspaceSession};
use tokio_tungstenite::tungstenite::Message;

/// How long to wait for you to finish signing in.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(600);

/// No Chromium-based browser was found (the caller falls back to pasting the cookie).
#[derive(Debug)]
pub struct NoBrowser;

impl std::fmt::Display for NoBrowser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no Chrome, Chromium, Edge or Brave found (set KLACK_BROWSER to one)")
    }
}

impl std::error::Error for NoBrowser {}

/// A browser to start: its command and the folder its temporary profile goes under (a flatpak
/// can only write inside its own folder).
pub struct Browser {
    pub label: String,
    program: Vec<String>,
    profile_root: PathBuf,
}

/// The first Chromium-based browser found: `$KLACK_BROWSER`, the usual installs, then flatpaks.
pub fn find() -> Option<Browser> {
    let cache = session::project_dirs().map(|d| d.cache_dir().join("login-browser")).unwrap_or_else(std::env::temp_dir);
    let native = |path: PathBuf| Browser { label: path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), program: vec![path.to_string_lossy().to_string()], profile_root: cache.clone() };
    if let Some(p) = std::env::var_os("KLACK_BROWSER").filter(|p| !p.is_empty()) {
        return Some(native(PathBuf::from(p)));
    }
    if let Some(path) = installed().into_iter().find(|p| p.is_file()) {
        return Some(native(path));
    }
    flatpak()
}

/// Where browsers are usually installed on this platform.
fn installed() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(target_os = "macos") {
        for app in ["Google Chrome.app/Contents/MacOS/Google Chrome", "Chromium.app/Contents/MacOS/Chromium", "Microsoft Edge.app/Contents/MacOS/Microsoft Edge", "Brave Browser.app/Contents/MacOS/Brave Browser"] {
            out.push(Path::new("/Applications").join(app));
            if let Some(home) = std::env::var_os("HOME") {
                out.push(Path::new(&home).join("Applications").join(app));
            }
        }
    } else if cfg!(windows) {
        for base in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"].iter().filter_map(std::env::var_os) {
            for exe in [r"Google\Chrome\Application\chrome.exe", r"Microsoft\Edge\Application\msedge.exe", r"BraveSoftware\Brave-Browser\Application\brave.exe", r"Chromium\Application\chrome.exe"] {
                out.push(Path::new(&base).join(exe));
            }
        }
    } else {
        let names = ["google-chrome-stable", "google-chrome", "chromium", "chromium-browser", "microsoft-edge-stable", "microsoft-edge", "brave-browser"];
        for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
            out.extend(names.iter().map(|n| dir.join(n)));
        }
    }
    out
}

/// A Chromium-based browser installed as a flatpak (Linux).
fn flatpak() -> Option<Browser> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let roots = [home.join(".local/share/flatpak/app"), PathBuf::from("/var/lib/flatpak/app")];
    ["com.google.Chrome", "org.chromium.Chromium", "com.microsoft.Edge", "com.brave.Browser"].into_iter().find(|id| roots.iter().any(|r| r.join(id).is_dir())).map(|id| Browser {
        label: format!("{id} (flatpak)"),
        program: vec!["flatpak".into(), "run".into(), id.into()],
        profile_root: home.join(".var/app").join(id),
    })
}

/// Open `browser` on `workspace_url`, wait until Slack has signed you in there and return the
/// session; the window is closed and its temporary profile deleted however it ends.
pub async fn sign_in(browser: &Browser, workspace_url: &str) -> Result<WorkspaceSession> {
    let profile = browser.profile_root.join(format!("klack-login-{}", std::process::id()));
    std::fs::create_dir_all(&profile).with_context(|| format!("creating {}", profile.display()))?;
    let mut window = Window { child: None, profile: profile.clone() };
    let mut cmd = Command::new(&browser.program[0]);
    cmd.args(&browser.program[1..])
        .arg(format!("--user-data-dir={}", profile.display()))
        .args(["--remote-debugging-port=0", "--no-first-run", "--no-default-browser-check", "--new-window", workspace_url])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    window.child = Some(cmd.spawn().with_context(|| format!("starting {}", browser.label))?);
    tracing::info!(profile = %profile.display(), "browser sign-in: {} started", browser.label);
    let result = tokio::select! {
        r = capture(&mut window, workspace_url) => r,
        _ = tokio::signal::ctrl_c() => Err(anyhow!("cancelled")),
    };
    window.close().await;
    result
}

/// The browser process and its temporary profile, removed on close (and on drop).
struct Window {
    child: Option<Child>,
    profile: PathBuf,
}

impl Window {
    fn exited(&mut self) -> bool {
        self.child.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(Some(_))))
    }

    async fn close(&mut self) {
        if let Some(port) = active_port(&self.profile) {
            if let Ok(mut ws) = connect(port).await {
                let _ = ws.send(Message::Text(json!({"id": 1, "method": "Browser.close"}).to_string().into())).await;
            }
        }
        if let Some(mut child) = self.child.take() {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline && matches!(child.try_wait(), Ok(None)) {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        // The browser may still be writing for a moment after it exits.
        for _ in 0..20 {
            if std::fs::remove_dir_all(&self.profile).is_ok() || !self.profile.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.profile);
    }
}

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Poll the browser's cookies until a `d` cookie signs in to the workspace.
async fn capture(window: &mut Window, workspace_url: &str) -> Result<WorkspaceSession> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let port = loop {
        if let Some(port) = active_port(&window.profile) {
            break port;
        }
        if window.exited() || Instant::now() > deadline {
            return Err(anyhow!("the browser did not open its DevTools port"));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    let mut ws = connect(port).await?;
    let deadline = Instant::now() + SIGN_IN_TIMEOUT;
    let (mut id, mut tried) = (0u64, String::new());
    loop {
        if window.exited() {
            return Err(anyhow!("the browser window was closed before signing in"));
        }
        if Instant::now() > deadline {
            return Err(anyhow!("not signed in after {} minutes", SIGN_IN_TIMEOUT.as_secs() / 60));
        }
        id += 1;
        if let Some(cookie) = call(&mut ws, id, "Storage.getCookies").await.ok().and_then(|v| slack_cookie(&v)) {
            // A new cookie: try it (one set halfway through signing in may not work yet).
            if cookie != tried {
                match session::session_from_cookie(workspace_url, &cookie).await {
                    Ok(s) => return Ok(s),
                    Err(e) => tracing::debug!("browser sign-in: cookie not usable yet: {e:#}"),
                }
                tried = cookie;
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// The browser-wide DevTools socket on `port`.
async fn connect(port: u16) -> Result<Socket> {
    let v: Value = slack_core::reqwest::get(format!("http://127.0.0.1:{port}/json/version")).await?.json().await?;
    let url = v["webSocketDebuggerUrl"].as_str().ok_or_else(|| anyhow!("no DevTools socket"))?;
    Ok(tokio_tungstenite::connect_async(url).await?.0)
}

/// Send DevTools command `method` and wait for its answer.
async fn call(ws: &mut Socket, id: u64, method: &str) -> Result<Value> {
    ws.send(Message::Text(json!({"id": id, "method": method}).to_string().into())).await?;
    while let Some(msg) = ws.next().await {
        if let Message::Text(text) = msg? {
            let v: Value = serde_json::from_str(&text)?;
            if v["id"].as_u64() == Some(id) {
                return Ok(v);
            }
        }
    }
    Err(anyhow!("the DevTools socket closed"))
}

/// The port Chromium wrote into its profile (`DevToolsActivePort`: port, then the path).
fn active_port(profile: &Path) -> Option<u16> {
    parse_active_port(&std::fs::read_to_string(profile.join("DevToolsActivePort")).ok()?)
}

fn parse_active_port(text: &str) -> Option<u16> {
    text.lines().next()?.trim().parse().ok().filter(|p| *p > 0)
}

/// The `d` session cookie for slack.com in a `Storage.getCookies` answer.
fn slack_cookie(answer: &Value) -> Option<String> {
    answer["result"]["cookies"].as_array()?.iter().find(|c| c["name"] == "d" && c["domain"].as_str().is_some_and(|d| d.trim_start_matches('.') == "slack.com")).and_then(|c| c["value"].as_str()).filter(|v| v.starts_with("xoxd-")).map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devtools_port_file() {
        assert_eq!(parse_active_port("42315\n/devtools/browser/0b1d-…\n"), Some(42315));
        assert_eq!(parse_active_port(""), None);
        assert_eq!(parse_active_port("0\n"), None);
    }

    #[test]
    fn the_session_cookie_is_found() {
        let answer = json!({"id": 3, "result": {"cookies": [
            {"name": "b", "domain": ".slack.com", "value": "x"},
            {"name": "d", "domain": ".example.com", "value": "xoxd-elsewhere"},
            {"name": "d-s", "domain": ".slack.com", "value": "1792"},
            {"name": "d", "domain": ".slack.com", "value": "xoxd-abc%2Fdef"}
        ]}});
        assert_eq!(slack_cookie(&answer).as_deref(), Some("xoxd-abc%2Fdef"));
        let before = json!({"id": 1, "result": {"cookies": [{"name": "d", "domain": ".slack.com", "value": "not-yet"}]}});
        assert_eq!(slack_cookie(&before), None, "only a real session cookie");
        assert_eq!(slack_cookie(&json!({"id": 2, "error": {"message": "x"}})), None);
    }
}

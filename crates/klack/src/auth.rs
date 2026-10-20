//! `klack auth login | logout | status`: signing a profile in to workspaces, forgetting a sign-in
//! on this machine and checking which still work. Sessions are kept per team + user, so one
//! profile may hold two accounts of one workspace.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use slack_core::session::{self, Profile, SessionSource, WorkspaceSession, workspace_url};
use slack_core::signin::{self, CodeError, CodeSignIn, CodeTeam};

use crate::login_import::Group;
use crate::pick::{Item, Row};
use crate::{login_browser, login_import, pick, qr};

/// How long `auth status` waits for each workspace's answer.
const STATUS_CHECK_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the check at start waits for each workspace's answer: no answer only means "can't check" and the app
/// starts anyway, so a network that hangs does not hold up every start for long.
const START_CHECK_TIMEOUT: Duration = Duration::from_secs(3);

/// The flags of `klack auth login`.
pub struct LoginFlags {
    pub workspace: Option<String>,
    pub cookie: Option<String>,
    pub no_browser: bool,
    pub from_browser: Option<login_import::Source>,
    pub browser_profile: Option<PathBuf>,
    pub email: Option<String>,
}

/// Sign `profile` in as `flags` say and save the session. Without flags, in a terminal, the guide
/// lists the sign-ins of the browsers and the ways to sign in afresh ([`guide`]); `--email` signs
/// in with the code Slack emails there ([`by_email`]); `--workspace` alone lists the ways to sign in
/// to that workspace; `--from-browser`, `--no-browser` and `--cookie` / `$KLACK_COOKIE_D` pick the
/// way. Without a terminal nothing is asked: the workspace must be given, and the way is the
/// browser window, then the cookie (from the environment or standard input).
pub async fn login(profile: &str, flags: LoginFlags) -> Result<()> {
    let LoginFlags { workspace, cookie, no_browser, from_browser, browser_profile, email } = flags;
    let asking = pick::interactive();
    if let Some(email) = email {
        return by_email(profile, &email, asking).await;
    }
    let given = cookie.or_else(|| std::env::var("KLACK_COOKIE_D").ok());
    let way_given = given.is_some() || no_browser || from_browser.is_some();
    if workspace.is_none() && !way_given && asking {
        guide(profile).await?;
        return Ok(());
    }
    // Only a missing file is a new profile: one that cannot be read is not replaced.
    let mut saved = Profile::read_or_new(&session::config_dir(profile)?.join("session.json"))?;
    let url = match workspace {
        Some(w) => workspace_url(&w).map_err(|e| anyhow!("--workspace {w}: {e}"))?,
        None if !asking => bail!("which workspace? add --workspace acme.slack.com"),
        None => match ask_workspace()? {
            Some(url) => url,
            None => return nothing_signed_in(),
        },
    };
    let ws = if let Some(source) = from_browser {
        from_browser_session(source, browser_profile.as_deref(), &url, asking).await?
    } else if let Some(c) = given {
        Some(cookie_session(&url, &c).await?)
    } else if asking && no_browser {
        match pick::ask_secret(COOKIE_PROMPT)? {
            Some(c) => Some(cookie_session(&url, &c).await?),
            None => None,
        }
    } else if asking {
        // `--workspace` alone: the ways to sign in to it, the browsers signed in to it first.
        let Some(groups) = looking("Looking for Slack in your browsers…", login_import::discover()).await? else { return nothing_signed_in() };
        match by_way(&groups, &url).await? {
            Some(Ok(s)) => Some(s),
            Some(Err((_, why))) => bail!("{why}"),
            None => None,
        }
    } else {
        Some(window_then_cookie(&url, no_browser).await?)
    };
    let Some(ws) = ws else { return nothing_signed_in() };
    finish(profile, &mut saved, vec![Ok(ws)])?;
    Ok(())
}

/// The prompt for the `d` cookie.
const COOKIE_PROMPT: &str = "d cookie (devtools → Application → Cookies → slack.com): ";

fn nothing_signed_in() -> Result<()> {
    println!("Nothing signed in.");
    Ok(())
}

/// A session of `url` from a browser of `source` (`profile`: one profile or a folder of them):
/// the one profile that signs in there, or the one picked when several do (`asking`; else the
/// freshest). `None` when the question was left with Esc.
async fn from_browser_session(source: login_import::Source, profile: Option<&std::path::Path>, url: &str, asking: bool) -> Result<Option<WorkspaceSession>> {
    let found = login_import::find(source, profile).await.with_context(|| format!("reading {}'s cookies", login_import::label(source)))?;
    if found.is_empty() {
        bail!("no Slack session in {} — sign in to Slack there first", login_import::label(source));
    }
    // Every profile that signs in to the workspace, one per account (freshest first).
    let mut working: Vec<(PathBuf, WorkspaceSession)> = Vec::new();
    for f in &found {
        match session::session_from_cookie(url, &f.cookie).await {
            Ok(s) if !working.iter().any(|(_, w)| w.user_id == s.user_id) => working.push((f.profile.clone(), s)),
            Ok(_) => {}
            Err(e) => tracing::debug!("{}: {e:#}", f.profile.display()),
        }
    }
    if working.is_empty() {
        bail!("{} has Slack cookies, but none signs in to {url} (signed out, or another workspace)", login_import::label(source));
    }
    let labels: Vec<String> = working.iter().map(|(dir, _)| login_import::profile_at(source, dir).label).collect();
    let pick = if working.len() > 1 && asking {
        let rows = working.iter().zip(&labels).map(|((_, s), label)| item(label, "", &format!("as {}", s.user_name), true)).collect();
        match pick::choose(&format!("{} profiles sign in to {}:", working.len(), host_of(url)), rows, false)? {
            pick::Outcome::Picked(at) => at[0],
            pick::Outcome::Cancelled => return Ok(None),
        }
    } else {
        // Not asked: the freshest (the list starts with it).
        0
    };
    let (dir, s) = working.swap_remove(pick);
    let label = labels[pick].clone();
    eprintln!("took the session from {label}");
    Ok(Some(WorkspaceSession { source: Some(SessionSource::Browser { browser: login_import::label(source).to_lowercase(), profile: dir, label }), ..s }))
}

/// A session of `url` with the `d` cookie `cookie` (as pasted: `d=` and spaces around are dropped).
async fn cookie_session(url: &str, cookie: &str) -> Result<WorkspaceSession> {
    let s = session::session_from_cookie(url, cookie.trim().trim_start_matches("d=")).await.context("sign-in with the cookie failed")?;
    Ok(WorkspaceSession { source: Some(SessionSource::Cookie), ..s })
}

/// What `auth login --workspace` did before the guide, and still does without a terminal: the
/// browser window (unless `no_browser`), then the cookie from standard input.
async fn window_then_cookie(url: &str, no_browser: bool) -> Result<WorkspaceSession> {
    if !no_browser {
        match login_browser::find() {
            Some(browser) => match window_session(&browser, url).await {
                Ok(s) => return Ok(s),
                Err(e) => eprintln!("browser sign-in did not finish: {e:#}"),
            },
            None => eprintln!("{}", login_browser::NoBrowser),
        }
    }
    eprint!("value of the `d` cookie (devtools → Application → Cookies → slack.com): ");
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    cookie_session(url, &line).await
}

/// A session of `url` signed in to in a new window of `browser`.
async fn window_session(browser: &login_browser::Browser, url: &str) -> Result<WorkspaceSession> {
    println!("Opening {} to sign in to {} — sign in there; the window closes by itself (Ctrl+C cancels).", browser.label, host_of(url));
    let s = login_browser::sign_in(browser, url).await?;
    Ok(WorkspaceSession { source: Some(SessionSource::Window), ..s })
}

/// Shows `line` while `work` runs, then erases it; `None` when Ctrl+C was pressed meanwhile (like Esc). Keys pressed
/// meanwhile are not echoed (an Enter would move the line out of reach of the erase); the question that follows throws
/// them away.
async fn looking<T>(line: &str, work: impl std::future::Future<Output = T>) -> Result<Option<T>> {
    use std::io::Write;
    let mut out = std::io::stdout();
    write!(out, "{line}")?;
    out.flush()?;
    // Ctrl+C is caught here rather than ending klack, so the echo is always put back.
    let quiet = NoEcho::start();
    let done = tokio::select! {
        d = work => Some(d),
        _ = tokio::signal::ctrl_c() => None,
    };
    drop(quiet);
    crossterm::execute!(out, crossterm::cursor::MoveToColumn(0), crossterm::terminal::Clear(crossterm::terminal::ClearType::CurrentLine))?;
    Ok(done)
}

/// The terminal's echo off for as long as it lives (Ctrl+C still works); on other systems, and when standard input is
/// not a terminal, nothing.
struct NoEcho {
    #[cfg(unix)]
    saved: Option<libc::termios>,
}

impl NoEcho {
    fn start() -> NoEcho {
        #[cfg(unix)]
        {
            // SAFETY: tcgetattr / tcsetattr only read and write the terminal settings of standard input into a
            // termios owned here; a failure (not a terminal) leaves them alone.
            unsafe {
                let mut t: libc::termios = std::mem::zeroed();
                if libc::tcgetattr(libc::STDIN_FILENO, &mut t) != 0 {
                    return NoEcho { saved: None };
                }
                let saved = t;
                t.c_lflag &= !(libc::ECHO | libc::ECHONL);
                if libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &t) != 0 {
                    return NoEcho { saved: None };
                }
                NoEcho { saved: Some(saved) }
            }
        }
        #[cfg(not(unix))]
        NoEcho {}
    }
}

impl Drop for NoEcho {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(saved) = &self.saved {
            // SAFETY: puts back the settings read in `start`.
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, saved);
            }
        }
    }
}

/// Asks for the workspace (`.slack.com` shown dim after a name) until it names one; `None` on Esc.
fn ask_workspace() -> Result<Option<String>> {
    loop {
        let Some(typed) = pick::ask("Workspace: ", |t| if t.contains('.') || t.contains('/') { String::new() } else { ".slack.com".into() })? else { return Ok(None) };
        match workspace_url(&typed) {
            Ok(url) => return Ok(Some(url)),
            Err(e) => println!("{e}"),
        }
    }
}

/// A sign-in that did not work: the workspace, and why.
type Failed = (String, String);

/// Asks how to sign in to `url` (the browser profiles of `groups` signed in to it first) and does
/// it. `None` when a question was left with Esc.
async fn by_way(groups: &[Group], url: &str) -> Result<Option<Result<WorkspaceSession, Failed>>> {
    let host = host_of(url);
    let browser = login_browser::find();
    let (rows, listed) = ways(groups, url, browser.as_ref().map(|b| b.label.as_str()));
    let items = item_rows(&rows);
    let pick::Outcome::Picked(at) = pick::choose(&format!("How to sign in to {host}?"), rows, false)? else { return Ok(None) };
    let Some(&way) = picked_items(&items, &listed, &at).first() else { return Ok(None) };
    let failed = |e: anyhow::Error| Err((host.clone(), format!("{e:#}")));
    Ok(Some(match way {
        Way::Browser(g, c) => match &groups[g].candidates[c].session {
            Ok(s) => Ok(s.clone()),
            Err(e) => Err((host.clone(), e.clone())),
        },
        Way::Window => match &browser {
            Some(b) => match window_session(b, url).await {
                // Ctrl+C while the window waits: like Esc (on a line of its own, after the terminal's `^C`).
                Err(e) if e.is::<login_browser::Cancelled>() => {
                    println!();
                    return Ok(None);
                }
                r => r.or_else(failed),
            },
            None => Err((host.clone(), login_browser::NoBrowser.to_string())),
        },
        Way::Cookie => match pick::ask_secret(COOKIE_PROMPT)? {
            Some(c) => cookie_session(url, &c).await.or_else(failed),
            None => return Ok(None),
        },
    }))
}

/// Each account (team + user) once, the first time it comes: the same account found in two browser profiles and
/// marked in both is signed in to once.
fn once_each(results: Vec<Result<WorkspaceSession, Failed>>) -> Vec<Result<WorkspaceSession, Failed>> {
    let mut seen: Vec<(String, String)> = Vec::new();
    results
        .into_iter()
        .filter(|r| match r {
            Ok(s) if seen.iter().any(|(t, u)| *t == s.team_id && *u == s.user_id) => false,
            Ok(s) => {
                seen.push((s.team_id.clone(), s.user_id.clone()));
                true
            }
            Err(_) => true,
        })
        .collect()
}

/// Saves the sessions that worked to `profile_name` and prints a line for each workspace:
/// `✓ Signed in …` or `✗ <workspace>: <why>`. Returns how many were saved.
fn finish(profile_name: &str, saved: &mut Profile, results: Vec<Result<WorkspaceSession, Failed>>) -> Result<usize> {
    let (good, bad): (Vec<_>, Vec<_>) = once_each(results).into_iter().partition(|r| r.is_ok());
    let good: Vec<WorkspaceSession> = good.into_iter().flatten().collect();
    for s in &good {
        saved.upsert(s.clone());
    }
    if !good.is_empty() {
        saved.save(profile_name)?;
    }
    for s in &good {
        println!("✓ Signed in to {} as {} — profile {profile_name} saved.", s.team_name, s.user_name);
    }
    for (team, why) in bad.into_iter().filter_map(Result::err) {
        println!("✗ {team}: {why}");
    }
    if good.is_empty() {
        println!("Nothing signed in.");
    }
    Ok(good.len())
}

/// The indices of the item rows of `rows` (what `items` of [`picked_items`] follow).
fn item_rows(rows: &[Row]) -> Vec<usize> {
    rows.iter().enumerate().filter(|(_, r)| matches!(r, Row::Item(_))).map(|(i, _)| i).collect()
}

/// The item of each picked row: `at` are row indices, `item_rows` the rows that are items, `items` one per item row.
fn picked_items<T: Copy>(item_rows: &[usize], items: &[T], at: &[usize]) -> Vec<T> {
    at.iter().filter_map(|r| item_rows.iter().position(|i| i == r)).map(|k| items[k]).collect()
}

/// The guided sign-in: lists the Slack sign-ins of every browser profile and the three ways to sign in afresh (Space
/// marks several, Enter takes them). The marked sign-ins are saved first (so a failure while asking does not lose
/// them), then each marked way runs in list order; Esc in a way shows the list again, rebuilt. Returns how many
/// sessions were saved; Esc at the list leaves without saving (or creating) anything more.
pub async fn guide(profile_name: &str) -> Result<usize> {
    let path = session::config_dir(profile_name)?.join("session.json");
    let mut saved = Profile::read_or_new(&path)?;
    println!("{}", opening_line(profile_name, path.exists()));
    let Some(groups) = looking("Looking for Slack in your browsers…", login_import::discover()).await? else {
        nothing_signed_in()?;
        return Ok(0);
    };
    // `reported`: `finish` has said what was signed in to (or that nothing was).
    let (mut total, mut reported) = (0, false);
    'list: loop {
        let (rows, choices) = rows(&groups, &saved, Browsers::Some);
        let items = item_rows(&rows);
        let pick::Outcome::Picked(at) = pick::choose("Sign in to:", rows, true)? else { break };
        let picked = picked_items(&items, &choices, &at);
        let found: Vec<Result<WorkspaceSession, Failed>> = picked
            .iter()
            .filter_map(|choice| match choice {
                Choice::Candidate(g, c) => groups[*g].candidates[*c].session.clone().ok().map(Ok),
                _ => None,
            })
            .collect();
        if !found.is_empty() {
            total += finish(profile_name, &mut saved, found)?;
            reported = true;
        }
        for choice in picked {
            let step = match choice {
                Choice::Candidate(..) => continue,
                Choice::Code => code_way(&saved, &Hint::default()).await?,
                // Task Y5 builds this way.
                Choice::InBrowser => {
                    println!("in your browser — coming in the next step");
                    Step::Back
                }
                Choice::Cookie => cookie_way().await?,
            };
            match step {
                // The way has said why already.
                Step::Done(results) if results.is_empty() => {}
                Step::Done(results) => {
                    total += finish(profile_name, &mut saved, results)?;
                    reported = true;
                }
                Step::Back => continue 'list,
            }
        }
        break;
    }
    if !reported {
        nothing_signed_in()?;
    }
    Ok(total)
}

/// How a way of signing in ended: `Done` with what it signed in to (maybe nothing, having said why), or `Back` (Esc)
/// to the guide's list.
enum Step<T> {
    Done(T),
    Back,
}

/// What signing in again knows about the sign-in it replaces; the guide on its own knows nothing.
#[derive(Debug, Clone, Default)]
pub struct Hint {
    /// The address the session's code was sent to: filled in at the email prompt.
    pub email: Option<String>,
    /// Team id and name: that workspace is marked when the code's list has it.
    pub team: Option<(String, String)>,
    /// The sign-in link of the way "in your browser". Task Y7 fills this in and reads it.
    #[allow(dead_code)]
    pub link: Option<String>,
}

/// The message for a code that is not six letters or digits.
const BAD_CODE: &str = "a code is 6 letters or digits, like ABC-123";
/// The line under the code screen until there is something to say.
const CODE_HINT: &str = "Enter checks the code · Esc goes back";

/// An email as typed, without spaces around it and without control characters (it is printed on the code screen).
fn clean_email(typed: &str) -> String {
    typed.trim().chars().filter(|c| !c.is_control()).collect()
}

fn no_workspace(email: &str) -> String {
    format!("no workspace signed in with {email} — sign up in Slack first")
}

/// The code screen printed above the code prompt: how many terminal lines it took, and the message line under it.
struct CodeScreen {
    rows: usize,
    width: u16,
    /// Whether it can be erased: the screen, the message line and the prompt all fit the terminal.
    erasable: bool,
}

impl CodeScreen {
    /// Prints the screen of `email` for this terminal's size, then the hint line; the prompt goes on the next line.
    fn show(email: &str) -> Result<CodeScreen> {
        let (width, height) = crossterm::terminal::size()?;
        let lines = qr::code_screen(email, width, height);
        for l in &lines {
            println!("{l}");
        }
        let rows = qr::rows_on(&lines, width);
        let screen = CodeScreen { rows, width, erasable: rows + qr::PROMPT_LINES <= height as usize };
        println!("\x1b[2m{}\x1b[0m", screen.fitted(CODE_HINT));
        Ok(screen)
    }

    /// `text` cut to one line of the terminal, so the message line stays one line.
    fn fitted(&self, text: &str) -> String {
        text.chars().take((self.width as usize).saturating_sub(1).max(1)).collect()
    }

    /// Puts `text` on the message line, in place of what was there (the cursor is at the start of the prompt line).
    fn say(&self, text: &str) -> Result<()> {
        use crossterm::{cursor::MoveToPreviousLine, terminal::{Clear, ClearType}};
        crossterm::execute!(std::io::stdout(), MoveToPreviousLine(1), Clear(ClearType::CurrentLine))?;
        println!("{}", self.fitted(text));
        Ok(())
    }

    /// Erases the screen and the message line, when they still fit the terminal (else they stay in the scrollback).
    fn erase(&self) -> Result<()> {
        use crossterm::{cursor::MoveToPreviousLine, terminal::{Clear, ClearType}};
        if self.erasable {
            crossterm::execute!(std::io::stdout(), MoveToPreviousLine((self.rows + 1) as u16), Clear(ClearType::FromCursorDown))?;
        }
        Ok(())
    }
}

/// The way "with a code from your email": the email (filled in from `hint`), the QR code of Slack's sign-in page for
/// the phone, the code Slack emails, then the workspaces of that email to pick from (`saved`: the profile, for
/// "already signed in"), each marked one signed in to. Esc (or Ctrl+C) at any question goes `Back`.
async fn code_way(saved: &Profile, hint: &Hint) -> Result<Step<Vec<Result<WorkspaceSession, Failed>>>> {
    let mut typed = hint.email.clone().unwrap_or_default();
    let email = loop {
        let Some(t) = pick::ask_with("Email: ", &typed, |t| if t.is_empty() { "the address you sign in to Slack with".into() } else { String::new() })? else { return Ok(Step::Back) };
        let email = clean_email(&t);
        if signin::email_ok(&email) {
            break email;
        }
        println!("that is not an email address — like you@example.com");
        typed = t;
    };
    let Some(started) = looking("Asking Slack…", CodeSignIn::start(&email)).await? else { return Ok(Step::Back) };
    let s = match started {
        Ok(s) => s,
        Err(e) => {
            println!("✗ Slack's sign-in page did not open: {e:#}");
            return Ok(Step::Done(Vec::new()));
        }
    };
    let screen = CodeScreen::show(&email)?;
    loop {
        // The code is shown as typed: it is short and works once.
        let Some(typed) = pick::ask("Code: ", |t| if t.is_empty() { "___-___".into() } else { String::new() })? else {
            screen.erase()?;
            return Ok(Step::Back);
        };
        let Some(code) = signin::normalize_code(&typed) else {
            screen.say(BAD_CODE)?;
            continue;
        };
        let Some(answer) = looking("Checking the code…", s.confirm(&code)).await? else {
            screen.erase()?;
            return Ok(Step::Back);
        };
        match answer {
            Ok(_) => break,
            // The phone can ask for another code: the prompt comes again.
            Err(e @ (CodeError::Failed | CodeError::Expired)) => screen.say(&e.to_string())?,
            Err(e) => {
                screen.erase()?;
                println!("✗ {e}");
                return Ok(Step::Done(Vec::new()));
            }
        }
    }
    screen.erase()?;
    let Some(listed) = looking("Finding your workspaces…", s.workspaces()).await? else { return Ok(Step::Back) };
    let teams = match listed {
        Ok(t) => t,
        Err(e) => {
            println!("✗ {e:#}");
            return Ok(Step::Done(Vec::new()));
        }
    };
    if teams.is_empty() {
        println!("{}", no_workspace(&email));
        return Ok(Step::Done(Vec::new()));
    }
    // None can be used here: each is said why, without a list that only Esc leaves.
    if teams.iter().all(|t| why_not(t).is_some()) {
        return Ok(Step::Done(teams.iter().map(|t| Err((team_name(t).to_string(), why_not(t).unwrap_or_default().to_string()))).collect()));
    }
    let (rows, marked) = team_rows(&teams, saved, hint.team.as_ref().map(|(id, _)| id.as_str()));
    let pick::Outcome::Picked(at) = pick::choose_marked("Sign in to:", rows, &marked)? else { return Ok(Step::Back) };
    let mut done = Vec::new();
    for t in at.into_iter().map(|i| &teams[i]) {
        let Some(r) = looking(&format!("Signing in to {}…", team_name(t)), s.redeem(t)).await? else {
            // Ctrl+C: the workspaces signed in to so far are kept, the others are left.
            if done.is_empty() {
                return Ok(Step::Back);
            }
            break;
        };
        done.push((team_name(t).to_string(), r));
    }
    Ok(Step::Done(redeemed(done)))
}

/// Each workspace's name and how its sign-in went, as [`finish`] takes them: one that failed is said why, and the
/// others are signed in all the same.
fn redeemed(teams: Vec<(String, Result<WorkspaceSession>)>) -> Vec<Result<WorkspaceSession, Failed>> {
    teams.into_iter().map(|(name, r)| r.map_err(|e| (name, format!("{e:#}")))).collect()
}

/// How a workspace of the code's list is named: its name, else its domain.
fn team_name(t: &CodeTeam) -> &str {
    if t.name.is_empty() { &t.domain } else { &t.name }
}

/// Why a workspace of the code's list can't be signed in to here; `None` when it can.
fn why_not(t: &CodeTeam) -> Option<&'static str> {
    if t.sso_required {
        Some("needs your browser (SSO)")
    } else if t.two_factor_required {
        // Slack's page would ask for the two-factor code (`auth.loginMagic`); not tried without an account to test with.
        Some("needs your browser (two-factor)")
    } else {
        None
    }
}

/// The workspaces of the code's list (dim with why when one can't be used; "already signed in" when `profile` has it,
/// by team: the list names no user), and the rows marked at the start: the one named by `team_hint`, else the only
/// one that can be used.
fn team_rows(teams: &[CodeTeam], profile: &Profile, team_hint: Option<&str>) -> (Vec<Row>, Vec<usize>) {
    let rows: Vec<Row> = teams
        .iter()
        .map(|t| match why_not(t) {
            Some(why) => item(team_name(t), &host_of(&t.url), why, false),
            None => {
                let already = profile.workspaces.iter().any(|w| w.team_id == t.team_id);
                item(team_name(t), &host_of(&t.url), if already { "already signed in" } else { "" }, true)
            }
        })
        .collect();
    let usable: Vec<usize> = (0..teams.len()).filter(|&i| why_not(&teams[i]).is_none()).collect();
    let marked = match team_hint {
        Some(id) => usable.iter().copied().filter(|&i| teams[i].team_id == id).collect(),
        None if usable.len() == 1 => usable,
        None => Vec::new(),
    };
    (rows, marked)
}

/// The cookie way as W had it (Task Y4 replaces it): the `d` cookie, then the workspace it is for.
async fn cookie_way() -> Result<Step<Vec<Result<WorkspaceSession, Failed>>>> {
    let Some(c) = pick::ask_secret(COOKIE_PROMPT)? else { return Ok(Step::Back) };
    let Some(url) = ask_workspace()? else { return Ok(Step::Back) };
    Ok(Step::Done(vec![cookie_session(&url, &c).await.map_err(|e| (host_of(&url), format!("{e:#}")))]))
}

/// `auth login --email ADDRESS`: the code way for that address. In a terminal it is [`code_way`] with the address
/// filled in; without one Slack's sign-in link is printed, the code is read from standard input, and every listed
/// workspace that can be used is signed in to.
async fn by_email(profile_name: &str, email: &str, asking: bool) -> Result<()> {
    let mut saved = Profile::read_or_new(&session::config_dir(profile_name)?.join("session.json"))?;
    let results = if asking {
        match code_way(&saved, &Hint { email: Some(email.to_string()), ..Hint::default() }).await? {
            Step::Done(r) => r,
            Step::Back => Vec::new(),
        }
    } else {
        code_from_stdin(email).await?
    };
    let count = if results.is_empty() {
        nothing_signed_in()?;
        0
    } else {
        finish(profile_name, &mut saved, results)?
    };
    email_outcome(asking, count)
}

/// How `auth login --email` ends after saving `saved` sessions: without a terminal nothing saved is an error, so a
/// script can tell (what failed has been printed already); in a terminal the person has seen it.
fn email_outcome(asking: bool, saved: usize) -> Result<()> {
    if !asking && saved == 0 {
        bail!("nothing signed in");
    }
    Ok(())
}

/// The code way without a terminal: the link and the question on standard error, the code from standard input.
async fn code_from_stdin(email: &str) -> Result<Vec<Result<WorkspaceSession, Failed>>> {
    let email = clean_email(email);
    if !signin::email_ok(&email) {
        bail!("--email {email}: that is not an email address");
    }
    let s = CodeSignIn::start(&email).await.context("Slack's sign-in page did not open")?;
    eprintln!("{}", signin::signin_link(&email));
    eprint!("type the code Slack emails you: ");
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    // A piped code is not echoed: what follows starts a line of its own.
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        eprintln!();
    }
    let code = signin::normalize_code(&line).ok_or_else(|| anyhow!(BAD_CODE))?;
    s.confirm(&code).await.map_err(|e| anyhow!("{e}"))?;
    let teams = s.workspaces().await?;
    if teams.is_empty() {
        bail!("{}", no_workspace(&email));
    }
    let mut out = Vec::new();
    for t in &teams {
        out.extend(match why_not(t) {
            Some(why) => vec![Err((team_name(t).to_string(), why.to_string()))],
            None => redeemed(vec![(team_name(t).to_string(), s.redeem(t).await)]),
        });
    }
    Ok(out)
}

/// The guide's first line: the profile it signs in, or that there is no such profile yet (`exists`: it has a
/// session file, even one without sign-ins).
fn opening_line(profile_name: &str, exists: bool) -> String {
    if exists { format!("Signing in profile {profile_name}.") } else { format!("Profile {profile_name} has no sign-ins yet.") }
}

/// Signs `ws`'s account in again, offering where its session came from first: a browser profile
/// that is still signed in to that workspace as that account is a one-row question (Enter takes
/// it); otherwise, or on Esc, the full guide. Returns whether anything was saved.
pub async fn sign_in_again(profile_name: &str, ws: &WorkspaceSession) -> Result<bool> {
    if let Some(SessionSource::Browser { profile, label, .. }) = &ws.source {
        let still = match login_import::browser_profiles().into_iter().find(|b| &b.dir == profile) {
            Some(b) => match looking(&format!("Looking at {label}…"), login_import::discover_in(vec![b])).await? {
                Some(groups) => still_signed_in(&groups, ws),
                None => {
                    nothing_signed_in()?;
                    return Ok(false);
                }
            },
            None => None,
        };
        match still {
            Some(s) => {
                let rows = vec![item("use it", "", &format!("as {}", s.user_name), true)];
                if let pick::Outcome::Picked(_) = pick::choose(&format!("{label} is still signed in to {} — use it?", ws.team_name), rows, false)? {
                    let mut saved = Profile::read_or_new(&session::config_dir(profile_name)?.join("session.json"))?;
                    return Ok(finish(profile_name, &mut saved, vec![Ok(s)])? > 0);
                }
            }
            None => println!("{label} is signed out of {} as well.", ws.team_name),
        }
    }
    Ok(guide(profile_name).await? > 0)
}

/// What an item of the guide's list stands for (one per item row, in row order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// A workspace a browser profile is signed in to: group, candidate.
    Candidate(usize, usize),
    /// `with a code from your email`
    Code,
    /// `in your browser`
    InBrowser,
    /// `paste a cookie`
    Cookie,
}

/// Whether this machine has browser profiles for the way "in your browser" to watch (Task Y5 finds out).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Browsers {
    Some,
    // Task Y5 tells these apart.
    #[cfg_attr(not(test), allow(dead_code))]
    None,
    #[cfg_attr(not(test), allow(dead_code))]
    OnlyUnreadable,
}

/// A way to sign in to one workspace (one per item row of [`ways`], in row order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Way {
    /// A browser profile signed in to it: group, candidate.
    Browser(usize, usize),
    Window,
    Cookie,
}

/// The guide's ways to sign in afresh, its last three rows.
const CODE_WAY: &str = "with a code from your email";
const IN_BROWSER: &str = "in your browser";
const PASTE_COOKIE: &str = "paste a cookie";
/// The ways to sign in to a workspace that is typed in.
const WINDOW: &str = "in a new browser window";
const WINDOW_NOTE: &str = "fresh — Google won't know you; may ask for a captcha";
const COOKIE: &str = "paste the d cookie by hand";

/// The host of a workspace's address: `acme.slack.com` of `https://acme.slack.com/`.
fn host_of(url: &str) -> String {
    url.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/').to_lowercase()
}

/// An item that can be picked when `enabled`.
fn item(text: &str, detail: &str, note: &str, enabled: bool) -> Row {
    Row::Item(Item { text: text.into(), detail: detail.into(), note: note.into(), enabled })
}

/// The guide's list: a heading per browser profile (a dim one with its problem when it could not be
/// looked at), a row per workspace found there, and the three ways to sign in afresh (the browser way
/// dim with why when `browsers` says there is none to use).
pub fn rows(groups: &[Group], profile: &Profile, browsers: Browsers) -> (Vec<Row>, Vec<Choice>) {
    let (mut rows, mut choices) = (Vec::new(), Vec::new());
    for (g, group) in groups.iter().enumerate() {
        let label = &group.profile.label;
        let account = group.profile.account.clone().unwrap_or_default();
        if let Some(problem) = &group.problem {
            let named = if account.is_empty() { label.clone() } else { format!("{label}  {account}") };
            rows.push(Row::Heading { title: String::new(), detail: format!("{named} — {problem}") });
            continue;
        }
        // A profile not signed in to Slack anywhere is not listed.
        if group.candidates.is_empty() {
            continue;
        }
        rows.push(Row::Heading { title: label.clone(), detail: account });
        for (c, found) in group.candidates.iter().enumerate() {
            let name = if found.team.team_name.is_empty() { &found.team.domain } else { &found.team.team_name };
            let row = match &found.session {
                Ok(s) => {
                    let already = profile.workspaces.iter().any(|w| w.team_id == s.team_id && w.user_id == s.user_id);
                    item(name, &host_of(&found.team.url), &format!("as {}{}", s.user_name, if already { "  already signed in" } else { "" }), true)
                }
                Err(e) => item(name, &host_of(&found.team.url), &reason(e, label), false),
            };
            rows.push(row);
            choices.push(Choice::Candidate(g, c));
        }
    }
    if !rows.is_empty() {
        rows.push(Row::Heading { title: String::new(), detail: String::new() });
    }
    let browser_note = match browsers {
        Browsers::Some => "",
        Browsers::None => "no browser found here — use the code",
        Browsers::OnlyUnreadable => "Chrome on Windows can't be read — use Firefox, or the code",
    };
    rows.push(item(CODE_WAY, "", "phone or any device", true));
    choices.push(Choice::Code);
    rows.push(item(IN_BROWSER, "", browser_note, browsers == Browsers::Some));
    choices.push(Choice::InBrowser);
    rows.push(item(PASTE_COOKIE, "", "", true));
    choices.push(Choice::Cookie);
    (rows, choices)
}

/// The note of a workspace whose check failed in the browser profile `label`.
fn reason(error: &str, label: &str) -> String {
    // What `session_from_cookie` says when Slack does not take the cookie for that workspace.
    if error.contains("no api_token") || error.starts_with("auth.test failed") {
        return format!("signed out in {label}");
    }
    let short: String = error.chars().take(60).collect();
    format!("can't check: {short}{}", if short.len() < error.len() { "…" } else { "" })
}

/// The ways to sign in to `url`: the browser profiles found signed in to it, the browser window (`browser` names it;
/// none found: shown dim) and the cookie.
fn ways(groups: &[Group], url: &str, browser: Option<&str>) -> (Vec<Row>, Vec<Way>) {
    let (mut rows, mut ways) = (Vec::new(), Vec::new());
    let host = host_of(url);
    for (g, group) in groups.iter().enumerate() {
        let label = &group.profile.label;
        for (c, found) in group.candidates.iter().enumerate() {
            let there = host_of(&found.team.url) == host || found.session.as_ref().is_ok_and(|s| host_of(&s.url) == host);
            if !there {
                continue;
            }
            let account = group.profile.account.as_deref().unwrap_or_default();
            rows.push(match &found.session {
                Ok(s) => item(label, account, &format!("as {}", s.user_name), true),
                Err(e) => item(label, account, &reason(e, label), false),
            });
            ways.push(Way::Browser(g, c));
        }
    }
    rows.push(match browser {
        Some(_) => item(WINDOW, "", WINDOW_NOTE, true),
        None => item(WINDOW, "", &login_browser::NoBrowser.to_string(), false),
    });
    ways.push(Way::Window);
    rows.push(item(COOKIE, "", "", true));
    ways.push(Way::Cookie);
    (rows, ways)
}

/// The session of `ws`'s account (team + user) among those `groups` found still signed in.
fn still_signed_in(groups: &[Group], ws: &WorkspaceSession) -> Option<WorkspaceSession> {
    groups.iter().flat_map(|g| &g.candidates).filter_map(|c| c.session.as_ref().ok()).find(|s| s.team_id == ws.team_id && s.user_id == ws.user_id).cloned()
}

/// How an account is named in lists and refusals: `acme (as Dmytro)`.
fn account_name(w: &WorkspaceSession) -> String {
    format!("{} (as {})", w.team_name, w.user_name)
}

/// A list row per account (its address dim after it), in the order given.
fn account_rows(accounts: &[&WorkspaceSession]) -> Vec<Row> {
    accounts.iter().map(|w| item(&account_name(w), &w.url, "", true)).collect()
}

/// The one account of workspace `team` (id or name) in `p`; refused when there is none, or when
/// the profile holds two accounts there (they are listed).
pub fn one_account<'a>(p: &'a Profile, profile: &str, team: &str) -> Result<&'a WorkspaceSession> {
    match p.matching(team).as_slice() {
        [] => bail!("profile {profile} has no workspace {team}"),
        [one] => Ok(one),
        several => {
            let who: Vec<String> = several.iter().map(|w| account_name(w)).collect();
            bail!("profile {profile} has {} accounts in {team}: {} — run this in a terminal to choose one", several.len(), who.join(", "))
        }
    }
}

/// The account of workspace `team` (id or name) in `p`, as [`one_account`] — except that in a
/// terminal two accounts there are listed to choose from. None when the question was left with Esc.
pub fn pick_account<'a>(p: &'a Profile, profile: &str, team: &str) -> Result<Option<&'a WorkspaceSession>> {
    let accounts = p.matching(team);
    if accounts.len() < 2 || !pick::interactive() {
        return one_account(p, profile, team).map(Some);
    }
    let pick::Outcome::Picked(at) = pick::choose(&format!("{team}: which account?"), account_rows(&accounts), false)? else { return Ok(None) };
    Ok(at.first().map(|i| accounts[*i]))
}

/// Forget sign-ins of `profile` on this machine (Slack is not signed out): the account of `team`,
/// or every one with `all`; with neither, in a terminal, the profile's accounts are listed (Space
/// marks, Enter forgets the marked, Esc changes nothing). The profile stays, empty if nothing is left.
pub async fn logout(profile: &str, team: Option<&str>, all: bool) -> Result<()> {
    let mut p = Profile::load(profile)?;
    let gone: Vec<(String, String)> = match (team, all) {
        (Some(_), true) => bail!("say --team or --all, not both"),
        (None, false) if !pick::interactive() => bail!("say --team or --all"),
        (None, false) if p.workspaces.is_empty() => {
            println!("profile {profile} has no sign-ins — nothing to forget");
            return Ok(());
        }
        (None, false) => {
            let accounts: Vec<&WorkspaceSession> = p.workspaces.iter().collect();
            let pick::Outcome::Picked(at) = pick::choose("Forget on this machine (Slack is not signed out):", account_rows(&accounts), true)? else {
                println!("nothing changed");
                return Ok(());
            };
            at.iter().map(|i| (accounts[*i].team_id.clone(), accounts[*i].user_id.clone())).collect()
        }
        (None, true) => p.workspaces.iter().map(|w| (w.team_id.clone(), w.user_id.clone())).collect(),
        (Some(t), false) => {
            let Some(w) = pick_account(&p, profile, t)? else {
                println!("nothing changed");
                return Ok(());
            };
            vec![(w.team_id.clone(), w.user_id.clone())]
        }
    };
    for (team_id, user_id) in &gone {
        if let Some(w) = p.workspaces.iter().find(|w| &w.team_id == team_id && &w.user_id == user_id) {
            println!("forgot {} as {} (this machine only; Slack is not signed out)", w.team_name, w.user_name);
        }
        p.remove(team_id, user_id);
    }
    p.save(profile)?;
    if p.workspaces.is_empty() {
        println!("profile {profile} has no sign-ins now");
    }
    Ok(())
}

/// How a session's source is shown.
pub fn source_label(source: Option<&SessionSource>) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    match source {
        Some(SessionSource::Browser { label, .. }) => Cow::Borrowed(label),
        Some(SessionSource::Window) => Cow::Borrowed("a browser window"),
        Some(SessionSource::Cookie) => Cow::Borrowed("cookie"),
        Some(SessionSource::Code { email }) => Cow::Owned(format!("code to {email}")),
        None => Cow::Borrowed("unknown"),
    }
}

/// Whether a session still works, as `auth.test` answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    Works,
    /// Slack says the session has ended.
    SignedOut,
    /// No answer, or another error: why. A network failure says nothing about the session.
    Unknown(String),
}

/// Whether `ws` still works (waiting `wait` at most).
async fn health(ws: &WorkspaceSession, wait: Duration) -> Health {
    let answer = tokio::time::timeout(wait, async { slack_core::Client::new(ws.clone())?.auth_test().await }).await;
    match answer {
        Ok(Ok(_)) => Health::Works,
        Ok(Err(e)) if slack_core::api::is_signed_out(&e) => Health::SignedOut,
        Ok(Err(e)) => Health::Unknown(format!("{e:#}")),
        Err(_) => Health::Unknown(format!("no answer in {} s", wait.as_secs())),
    }
}

/// Whether each workspace of `profile` still works, in its order (all asked at once, each waited for `wait` at most).
pub async fn check(profile: &Profile, wait: Duration) -> Vec<Health> {
    futures_util::future::join_all(profile.workspaces.iter().map(|w| health(w, wait))).await
}

/// What to do about a workspace whose session has ended (one per row of [`fix_rows`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fix {
    SignInAgain,
    Remove,
    Launch,
    Quit,
}

/// The question about a signed-out workspace.
fn question(ws: &WorkspaceSession) -> String {
    format!("{} (as {}): signed out", ws.team_name, ws.user_name)
}

/// The answers to [`question`]: launching without it only while another workspace can start
/// (`launchable`), else quitting.
fn fix_rows(launchable: bool) -> (Vec<Row>, Vec<Fix>) {
    let last = if launchable { (item("launch anyway", "", "without it", true), Fix::Launch) } else { (item("quit", "", "", true), Fix::Quit) };
    (vec![item("sign in again", "", "", true), item("remove it from the profile", "", "", true), last.0], vec![Fix::SignInAgain, Fix::Remove, last.1])
}

/// [`check`] at start (a short wait), with a line saying so while it runs when `asking`; None when Ctrl+C was pressed.
async fn checked(p: &Profile, asking: bool) -> Result<Option<Vec<Health>>> {
    let health = if asking { looking("Checking the sign-ins…", check(p, START_CHECK_TIMEOUT)).await? } else { Some(check(p, START_CHECK_TIMEOUT).await) };
    for (w, h) in p.workspaces.iter().zip(health.iter().flatten()) {
        if let Health::Unknown(why) = h {
            tracing::warn!("{}: can't check the session: {why}", w.team_name);
        }
    }
    Ok(health)
}

/// Before the app starts: each workspace of `p` is checked. In a terminal each signed-out one is
/// asked about, in profile order — sign in again (where the session came from first), remove it
/// from the profile, or launch anyway without it (only while another can start; else quit).
/// Without a terminal a warning per signed-out workspace, and the others start (an error when
/// none can). One that could not be checked starts as normal. Returns the profile to run and
/// its signed-out workspaces; None to quit.
pub async fn at_start(profile_name: &str, mut p: Profile) -> Result<Option<(Profile, HashSet<usize>)>> {
    let asking = pick::interactive();
    let Some(mut health) = checked(&p, asking).await? else { return Ok(None) };
    // The accounts (team + user) answered with "launch anyway".
    let mut skipped: Vec<(String, String)> = Vec::new();
    loop {
        let out: HashSet<usize> = health.iter().enumerate().filter(|(_, h)| **h == Health::SignedOut).map(|(i, _)| i).collect();
        let launchable = out.len() < p.workspaces.len();
        if !asking {
            for (_, w) in p.workspaces.iter().enumerate().filter(|(i, _)| out.contains(i)) {
                eprintln!("warning: {} — klack auth login", question(w));
            }
            if !launchable {
                bail!("every workspace of profile {profile_name} is signed out — klack auth login");
            }
            return Ok(Some((p, out)));
        }
        let asked = (0..p.workspaces.len()).find(|i| out.contains(i) && !skipped.iter().any(|(t, u)| *t == p.workspaces[*i].team_id && *u == p.workspaces[*i].user_id));
        let Some(i) = asked else { return Ok(Some((p, out))) };
        let ws = p.workspaces[i].clone();
        let (rows, fixes) = fix_rows(launchable);
        // Esc and Ctrl+C: quit, as `quit` does.
        let pick::Outcome::Picked(at) = pick::choose(&question(&ws), rows, false)? else { return Ok(None) };
        match at.first().map(|a| fixes[*a]) {
            Some(Fix::SignInAgain) => match sign_in_again(profile_name, &ws).await {
                Ok(true) => {
                    p = Profile::load(profile_name)?;
                    let Some(again) = checked(&p, asking).await? else { return Ok(None) };
                    health = again;
                }
                // Nothing saved: the same question again.
                Ok(false) => {}
                Err(e) => println!("✗ {e:#}"),
            },
            Some(Fix::Remove) => {
                p.remove(&ws.team_id, &ws.user_id);
                health.remove(i);
                p.save(profile_name)?;
                println!("forgot {} as {} (this machine only; Slack is not signed out)", ws.team_name, ws.user_name);
                if p.workspaces.is_empty() {
                    println!("profile {profile_name} has no sign-ins now — klack auth login");
                    return Ok(None);
                }
            }
            Some(Fix::Launch) => skipped.push((ws.team_id.clone(), ws.user_id.clone())),
            Some(Fix::Quit) | None => return Ok(None),
        }
    }
}

/// One line per workspace of `profile` — name, address, who you are, where the session came
/// from and whether it still works (checked concurrently) — with ★ on the one plain `klack` opens.
pub async fn status(profile: &str) -> Result<()> {
    let p = Profile::read_or_new(&session::config_dir(profile)?.join("session.json"))?;
    if p.workspaces.is_empty() {
        println!("profile {profile} has no sign-ins — klack auth login");
        return Ok(());
    }
    let start = p.workspace(None).map(|w| (w.team_id.clone(), w.user_id.clone()));
    let states = check(&p, STATUS_CHECK_TIMEOUT).await;
    println!("profile {profile}:");
    for (w, health) in p.workspaces.iter().zip(states) {
        let mark = if start.as_ref().is_some_and(|(t, u)| *t == w.team_id && *u == w.user_id) { '★' } else { ' ' };
        let state = match health {
            Health::Works => "works".to_string(),
            Health::SignedOut => "signed out".to_string(),
            Health::Unknown(why) => format!("can't check: {why}"),
        };
        println!("{mark} {}  {}  as {}  {}  {state}", w.team_name, w.url, w.user_name, source_label(w.source.as_ref()));
    }
    Ok(())
}

/// Ask `question` with y/N on the terminal (yes only for y / yes); `yes` (from `--yes`) answers it.
/// Without a terminal to ask in it is refused.
pub fn confirm(question: &str, yes: bool) -> Result<bool> {
    use std::io::IsTerminal;
    if yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        bail!("{question} — no terminal to ask in: add --yes");
    }
    eprint!("{question} [y/N] ");
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(matches!(line.trim().to_lowercase().as_str(), "y" | "yes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(team: &str, name: &str, user: &str) -> WorkspaceSession {
        WorkspaceSession { team_id: team.into(), team_name: name.into(), user_id: user.into(), user_name: format!("person-{user}"), ..Default::default() }
    }

    #[test]
    fn a_team_with_two_accounts_is_not_guessed() {
        let mut p = Profile::default();
        p.upsert(account("T0000000001", "acme", "U0000000001"));
        p.upsert(account("T0000000001", "acme", "U0000000002"));
        p.upsert(account("T0000000002", "other", "U0000000001"));
        assert_eq!(one_account(&p, "default", "other").unwrap().team_id, "T0000000002");
        let e = one_account(&p, "default", "acme").unwrap_err().to_string();
        assert!(e.contains("2 accounts") && e.contains("acme (as person-U0000000001), acme (as person-U0000000002)"), "{e}");
        assert!(one_account(&p, "default", "nope").is_err());
    }

    use crate::login_import::{BrowserProfile, Candidate, Source};
    use slack_core::session::LoggedInTeam;
    use slack_core::signin::CodeTeam;

    fn found(team: &str, name: &str, user: Option<&str>, label: &str) -> Candidate {
        let domain = name.to_lowercase();
        let t = LoggedInTeam { team_id: team.into(), team_name: name.into(), domain: domain.clone(), url: format!("https://{domain}.slack.com/"), email: "person@example.com".into() };
        let session = match user {
            Some(u) => Ok(WorkspaceSession { url: t.url.clone(), source: Some(SessionSource::Browser { browser: "firefox".into(), profile: label.into(), label: label.into() }), ..account(team, name, u) }),
            None => Err("signed-out or unexpected page (no api_token) — is the session still valid?".to_string()),
        };
        Candidate { team: t, session }
    }

    fn group(label: &str, account: Option<&str>, problem: Option<&str>, candidates: Vec<Candidate>) -> Group {
        let profile = BrowserProfile { source: if label.starts_with("Firefox") { Source::Firefox } else { Source::Chrome }, dir: PathBuf::from(format!("/profiles/{label}")), label: label.into(), account: account.map(String::from) };
        Group { profile, problem: problem.map(String::from), candidates }
    }

    /// The rows as text: `# title | detail` for a heading, `- text | detail | note` for an item (`(dim)` when it can't be picked).
    fn shown(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                Row::Heading { title, detail } => format!("# {title} | {detail}"),
                Row::Item(it) => format!("- {} | {} | {}{}", it.text, it.detail, it.note, if it.enabled { "" } else { " (dim)" }),
            })
            .collect()
    }

    #[test]
    fn rows_put_headings_failures_and_the_last_row() {
        let groups = vec![
            group("Chrome · Work", Some("person@example.com"), None, vec![found("T0000000001", "Acme", Some("U0000000001"), "Chrome · Work"), found("T0000000002", "Widgets", None, "Chrome · Work")]),
            // Nothing found there: not listed.
            group("Chrome · Empty", None, None, vec![]),
            group("Firefox · default", None, None, vec![found("T0000000002", "Widgets", Some("U0000000001"), "Firefox · default")]),
        ];
        let mut profile = Profile::default();
        profile.upsert(account("T0000000002", "Widgets", "U0000000001"));
        // Another account of Acme is not this one.
        profile.upsert(account("T0000000001", "Acme", "U0000000002"));
        let (rows, choices) = rows(&groups, &profile, Browsers::Some);
        assert_eq!(
            shown(&rows),
            [
                "# Chrome · Work | person@example.com",
                "- Acme | acme.slack.com | as person-U0000000001",
                "- Widgets | widgets.slack.com | signed out in Chrome · Work (dim)",
                "# Firefox · default | ",
                "- Widgets | widgets.slack.com | as person-U0000000001  already signed in",
                "#  | ",
                "- with a code from your email |  | phone or any device",
                "- in your browser |  | ",
                "- paste a cookie |  | ",
            ]
        );
        assert_eq!(choices, [Choice::Candidate(0, 0), Choice::Candidate(0, 1), Choice::Candidate(2, 0), Choice::Code, Choice::InBrowser, Choice::Cookie]);
    }

    #[test]
    fn a_problem_group_is_a_dim_heading() {
        let groups = vec![group("Chrome · Work", Some("person@example.com"), Some("can't read Chrome's key: the keyring is locked"), vec![])];
        let (rows, choices) = rows(&groups, &Profile::default(), Browsers::Some);
        assert_eq!(shown(&rows), ["#  | Chrome · Work  person@example.com — can't read Chrome's key: the keyring is locked", "#  | ", "- with a code from your email |  | phone or any device", "- in your browser |  | ", "- paste a cookie |  | "]);
        assert_eq!(choices, [Choice::Code, Choice::InBrowser, Choice::Cookie]);
    }

    #[test]
    fn nothing_found_still_offers_another_way() {
        let (rows, choices) = rows(&[], &Profile::default(), Browsers::Some);
        assert_eq!(shown(&rows), ["- with a code from your email |  | phone or any device", "- in your browser |  | ", "- paste a cookie |  | "]);
        assert_eq!(choices, [Choice::Code, Choice::InBrowser, Choice::Cookie]);
    }

    #[test]
    fn the_browser_way_is_dim_without_browsers() {
        let none = shown(&rows(&[], &Profile::default(), Browsers::None).0);
        assert_eq!(none[1], "- in your browser |  | no browser found here — use the code (dim)");
        let unreadable = shown(&rows(&[], &Profile::default(), Browsers::OnlyUnreadable).0);
        assert_eq!(unreadable[1], "- in your browser |  | Chrome on Windows can't be read — use Firefox, or the code (dim)");
        // The code and the cookie can always be picked.
        assert!(!unreadable[0].ends_with("(dim)") && !unreadable[2].ends_with("(dim)"));
    }

    fn code_team(id: &str, name: &str, sso: bool, two_factor: bool) -> CodeTeam {
        let domain = name.to_lowercase();
        CodeTeam { team_id: id.into(), name: name.into(), url: format!("https://{domain}.slack.com/"), domain, magic_login_url: "https://example.com/magic".into(), sso_required: sso, two_factor_required: two_factor }
    }

    #[test]
    fn code_teams_are_listed_with_why_not() {
        let teams = vec![code_team("T0000000001", "Acme", false, false), code_team("T0000000003", "Beta", true, false), code_team("T0000000004", "Gamma", false, true), code_team("T0000000002", "Widgets", false, false)];
        let mut profile = Profile::default();
        profile.upsert(account("T0000000002", "Widgets", "U0000000001"));
        let (rows, marked) = team_rows(&teams, &profile, None);
        assert_eq!(
            shown(&rows),
            [
                "- Acme | acme.slack.com | ",
                "- Beta | beta.slack.com | needs your browser (SSO) (dim)",
                "- Gamma | gamma.slack.com | needs your browser (two-factor) (dim)",
                "- Widgets | widgets.slack.com | already signed in",
            ]
        );
        // Two can be used (signing in again refreshes the one the profile has): none is guessed.
        assert_eq!(marked, Vec::<usize>::new());
        // Signing in again names the team: that one.
        assert_eq!(team_rows(&teams, &profile, Some("T0000000002")).1, [3]);
        // A hint naming a team that can't be used marks nothing.
        assert_eq!(team_rows(&teams, &profile, Some("T0000000003")).1, Vec::<usize>::new());
        // Only one can be used: it is marked.
        assert_eq!(team_rows(&teams[..2], &profile, None).1, [0]);
    }

    #[test]
    fn a_script_learns_that_nothing_was_signed_in() {
        // Without a terminal nothing saved is an error (a non-zero exit); in a terminal the person has seen why.
        assert!(email_outcome(false, 0).is_err_and(|e| e.to_string() == "nothing signed in"));
        assert!(email_outcome(false, 2).is_ok());
        assert!(email_outcome(true, 0).is_ok());
        assert!(email_outcome(true, 1).is_ok());
    }

    #[test]
    fn a_team_that_fails_does_not_stop_the_others() {
        let (s1, s2) = (account("T0000000001", "Acme", "U0000000001"), account("T0000000003", "Other", "U0000000001"));
        let out = redeemed(vec![("Acme".into(), Ok(s1)), ("Widgets".into(), Err(anyhow!("Slack did not sign in to Widgets"))), ("Other".into(), Ok(s2))]);
        let shown: Vec<String> = out.iter().map(|r| match r {
            Ok(s) => format!("ok {}", s.team_name),
            Err((team, why)) => format!("err {team}: {why}"),
        }).collect();
        assert_eq!(shown, ["ok Acme", "err Widgets: Slack did not sign in to Widgets", "ok Other"]);
    }

    #[test]
    fn a_failed_check_says_why() {
        assert_eq!(reason("auth.test failed: invalid_auth", "Firefox · default"), "signed out in Firefox · default");
        assert_eq!(reason("Slack did not answer in 10 s", "Firefox · default"), "can't check: Slack did not answer in 10 s");
    }

    #[test]
    fn ways_list_the_browsers_signed_in_to_that_workspace_first() {
        let groups = vec![
            group("Chrome · Work", Some("person@example.com"), None, vec![found("T0000000002", "Widgets", Some("U0000000001"), "Chrome · Work"), found("T0000000001", "Acme", Some("U0000000001"), "Chrome · Work")]),
            group("Firefox · default", None, None, vec![found("T0000000001", "Acme", None, "Firefox · default")]),
        ];
        let (rows, listed) = ways(&groups, "https://acme.slack.com/", Some("google-chrome"));
        assert_eq!(
            shown(&rows),
            [
                "- Chrome · Work | person@example.com | as person-U0000000001",
                "- Firefox · default |  | signed out in Firefox · default (dim)",
                "- in a new browser window |  | fresh — Google won't know you; may ask for a captcha",
                "- paste the d cookie by hand |  | ",
            ]
        );
        assert_eq!(listed, [Way::Browser(0, 1), Way::Browser(1, 0), Way::Window, Way::Cookie]);
        // No browser to open: the window can't be picked; the cookie always can.
        let (rows, listed) = ways(&[], "https://other.slack.com/", None);
        assert_eq!(shown(&rows), ["- in a new browser window |  | no Chrome, Chromium, Edge or Brave found (set KLACK_BROWSER to one) (dim)", "- paste the d cookie by hand |  | "]);
        assert_eq!(listed, [Way::Window, Way::Cookie]);
    }

    #[test]
    fn the_remembered_browser_is_still_signed_in_as_that_account() {
        let groups = vec![group("Firefox · default", None, None, vec![found("T0000000001", "Acme", Some("U0000000002"), "Firefox · default"), found("T0000000001", "Acme", Some("U0000000001"), "Firefox · default"), found("T0000000002", "Widgets", None, "Firefox · default")])];
        let again = still_signed_in(&groups, &account("T0000000001", "Acme", "U0000000001")).unwrap();
        assert_eq!((again.team_id.as_str(), again.user_id.as_str()), ("T0000000001", "U0000000001"));
        assert!(matches!(again.source, Some(SessionSource::Browser { .. })));
        // Signed out there, or never signed in there.
        assert!(still_signed_in(&groups, &account("T0000000002", "Widgets", "U0000000001")).is_none());
        assert!(still_signed_in(&groups, &account("T0000000003", "Other", "U0000000001")).is_none());
    }

    #[test]
    fn an_account_marked_twice_signs_in_once() {
        let first = WorkspaceSession { url: "first".into(), ..account("T0000000001", "Acme", "U0000000001") };
        let second = WorkspaceSession { url: "second".into(), ..account("T0000000001", "Acme", "U0000000001") };
        let other = account("T0000000001", "Acme", "U0000000002");
        let failed: Result<WorkspaceSession, Failed> = Err(("acme.slack.com".into(), "no".into()));
        let kept = once_each(vec![Ok(first), failed, Ok(second), Ok(other)]);
        let shown: Vec<String> = kept.iter().map(|r| match r {
            Ok(s) => format!("{} {}", s.user_id, s.url),
            Err((host, _)) => host.clone(),
        }).collect();
        assert_eq!(shown, ["U0000000001 first", "acme.slack.com", "U0000000002 "]);
    }

    #[test]
    fn picked_rows_map_to_their_items() {
        let rows = vec![Row::Heading { title: "A".into(), detail: String::new() }, item("a", "", "", true), item("b", "", "", true), Row::Heading { title: String::new(), detail: String::new() }, item("c", "", "", true)];
        let items = item_rows(&rows);
        assert_eq!(picked_items(&items, &['a', 'b', 'c'], &[1, 4]), ['a', 'c']);
        assert_eq!(picked_items(&items, &['a', 'b', 'c'], &[0]), Vec::<char>::new());
    }

    #[test]
    fn launch_anyway_only_when_another_works() {
        let ws = account("T0000000001", "acme", "U0000000001");
        assert_eq!(question(&ws), "acme (as person-U0000000001): signed out");
        let (rows, fixes) = fix_rows(true);
        assert_eq!(shown(&rows), ["- sign in again |  | ", "- remove it from the profile |  | ", "- launch anyway |  | without it"]);
        assert_eq!(fixes, [Fix::SignInAgain, Fix::Remove, Fix::Launch]);
        let (rows, fixes) = fix_rows(false);
        assert_eq!(shown(&rows), ["- sign in again |  | ", "- remove it from the profile |  | ", "- quit |  | "]);
        assert_eq!(fixes, [Fix::SignInAgain, Fix::Remove, Fix::Quit]);
    }

    #[test]
    fn logout_lists_each_account_with_its_address() {
        let mut p = Profile::default();
        for (team, name, user) in [("T0000000001", "acme", "U0000000001"), ("T0000000001", "acme", "U0000000002"), ("T0000000002", "other", "U0000000001")] {
            p.upsert(WorkspaceSession { url: format!("https://{name}.slack.com/"), ..account(team, name, user) });
        }
        let all: Vec<&WorkspaceSession> = p.workspaces.iter().collect();
        assert_eq!(
            shown(&account_rows(&all)),
            ["- acme (as person-U0000000001) | https://acme.slack.com/ | ", "- acme (as person-U0000000002) | https://acme.slack.com/ | ", "- other (as person-U0000000001) | https://other.slack.com/ | "]
        );
        assert_eq!(shown(&account_rows(&p.matching("acme"))).len(), 2, "--team acme: its two accounts");
    }

    #[tokio::test]
    async fn the_check_at_start_waits_a_short_time_only() {
        // A network that does not answer only means "can't check, launch": the start waits little for it.
        assert_eq!((START_CHECK_TIMEOUT, STATUS_CHECK_TIMEOUT), (Duration::from_secs(3), Duration::from_secs(10)));
        let _ = rustls::crypto::ring::default_provider().install_default();
        // A server that takes the connection and never answers.
        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let ws = WorkspaceSession { url: format!("http://{}/", silent.local_addr().unwrap()), ..account("T0000000001", "acme", "U0000000001") };
        let started = std::time::Instant::now();
        let h = health(&ws, Duration::from_millis(300)).await;
        assert!(matches!(&h, Health::Unknown(why) if why.starts_with("no answer in")), "{h:?}");
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    }

    #[test]
    fn the_guide_says_whether_the_profile_exists() {
        assert_eq!(opening_line("work", true), "Signing in profile work.");
        assert_eq!(opening_line("asd", false), "Profile asd has no sign-ins yet.");
    }

    #[test]
    fn sources_are_named() {
        let chrome = SessionSource::Browser { browser: "chrome".into(), profile: "/x".into(), label: "Chrome · Work".into() };
        assert_eq!(source_label(Some(&chrome)), "Chrome · Work");
        assert_eq!(source_label(Some(&SessionSource::Window)), "a browser window");
        assert_eq!(source_label(Some(&SessionSource::Code { email: "person@example.com".into() })), "code to person@example.com");
        assert_eq!(source_label(Some(&SessionSource::Cookie)), "cookie");
        assert_eq!(source_label(None), "unknown");
    }
}

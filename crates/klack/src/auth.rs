//! `klack auth login | logout | status`: signing a profile in to workspaces, forgetting a sign-in
//! on this machine and checking which still work. Sessions are kept per team + user, so one
//! profile may hold two accounts of one workspace.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use slack_core::session::{self, Profile, SessionSource, WorkspaceSession};

use crate::login_import::Group;
use crate::pick::{Item, Row};
use crate::{login_browser, login_import, pick};

/// How long `auth status` waits for each workspace's answer.
const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// The flags of `klack auth login`.
pub struct LoginFlags {
    pub workspace: Option<String>,
    pub cookie: Option<String>,
    pub no_browser: bool,
    pub from_browser: Option<login_import::Source>,
    pub browser_profile: Option<PathBuf>,
}

/// Sign `profile` in as `flags` say and save the session. Without flags, in a terminal, the guide
/// lists the sign-ins of the browsers ([`guide`]); `--workspace` alone lists the ways to sign in
/// to that workspace; `--from-browser`, `--no-browser` and `--cookie` / `$KLACK_COOKIE_D` pick the
/// way. Without a terminal nothing is asked: the workspace must be given, and the way is the
/// browser window, then the cookie (from the environment or standard input).
pub async fn login(profile: &str, flags: LoginFlags) -> Result<()> {
    let LoginFlags { workspace, cookie, no_browser, from_browser, browser_profile } = flags;
    let given = cookie.or_else(|| std::env::var("KLACK_COOKIE_D").ok());
    let asking = pick::interactive();
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
        let groups = looking("Looking for Slack in your browsers…", login_import::discover()).await?;
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

/// Shows `line` while `work` runs, then erases it.
async fn looking<T>(line: &str, work: impl std::future::Future<Output = T>) -> Result<T> {
    use std::io::Write;
    let mut out = std::io::stdout();
    write!(out, "{line}")?;
    out.flush()?;
    let done = work.await;
    crossterm::execute!(out, crossterm::cursor::MoveToColumn(0), crossterm::terminal::Clear(crossterm::terminal::ClearType::CurrentLine))?;
    Ok(done)
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
            Some(b) => window_session(b, url).await.or_else(failed),
            None => Err((host.clone(), login_browser::NoBrowser.to_string())),
        },
        Way::Cookie => match pick::ask_secret(COOKIE_PROMPT)? {
            Some(c) => cookie_session(url, &c).await.or_else(failed),
            None => return Ok(None),
        },
    }))
}

/// Saves the sessions that worked to `profile_name` and prints a line for each workspace:
/// `✓ Signed in …` or `✗ <workspace>: <why>`. Returns how many were saved.
fn finish(profile_name: &str, saved: &mut Profile, results: Vec<Result<WorkspaceSession, Failed>>) -> Result<usize> {
    let (good, bad): (Vec<_>, Vec<_>) = results.into_iter().partition(|r| r.is_ok());
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

/// The guided sign-in: lists the Slack sign-ins of every browser profile (Space marks several,
/// Enter signs in to them), or another workspace and how to sign in to it. Returns how many
/// sessions were saved; Esc leaves without saving (or creating) anything.
pub async fn guide(profile_name: &str) -> Result<usize> {
    let mut saved = Profile::read_or_new(&session::config_dir(profile_name)?.join("session.json"))?;
    if saved.workspaces.is_empty() {
        println!("Profile {profile_name} has no sign-ins yet.");
    } else {
        println!("Signing in profile {profile_name}.");
    }
    let groups = looking("Looking for Slack in your browsers…", login_import::discover()).await?;
    let (rows, choices) = rows(&groups, &saved);
    let items = item_rows(&rows);
    let pick::Outcome::Picked(at) = pick::choose("Sign in to:", rows, true)? else {
        nothing_signed_in()?;
        return Ok(0);
    };
    let mut results = Vec::new();
    let mut other = false;
    for choice in picked_items(&items, &choices, &at) {
        match choice {
            Choice::Candidate(g, c) => {
                if let Ok(s) = &groups[g].candidates[c].session {
                    results.push(Ok(s.clone()));
                }
            }
            Choice::Other => other = true,
        }
    }
    if other
        && let Some(url) = ask_workspace()?
        && let Some(r) = by_way(&groups, &url).await?
    {
        results.push(r);
    }
    finish(profile_name, &mut saved, results)
}

/// Signs `ws`'s account in again, offering where its session came from first: a browser profile
/// that is still signed in to that workspace as that account is a one-row question (Enter takes
/// it); otherwise, or on Esc, the full guide. Returns whether anything was saved.
// Used when a session has ended, at start (W8).
#[allow(dead_code)]
pub async fn sign_in_again(profile_name: &str, ws: &WorkspaceSession) -> Result<bool> {
    if let Some(SessionSource::Browser { profile, label, .. }) = &ws.source {
        let still = match login_import::browser_profiles().into_iter().find(|b| &b.dir == profile) {
            Some(b) => still_signed_in(&looking(&format!("Looking at {label}…"), login_import::discover_in(vec![b])).await?, ws),
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
    /// `another workspace, or sign in another way…`
    Other,
}

/// A way to sign in to one workspace (one per item row of [`ways`], in row order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Way {
    /// A browser profile signed in to it: group, candidate.
    Browser(usize, usize),
    Window,
    Cookie,
}

/// The guide's last row.
const OTHER: &str = "another workspace, or sign in another way…";
/// The ways to sign in to a workspace that is typed in.
const WINDOW: &str = "in a new browser window";
const WINDOW_NOTE: &str = "fresh — Google won't know you; may ask for a captcha";
const COOKIE: &str = "paste the d cookie by hand";

/// The address of the workspace typed as `input`: a name (`acme`), an address (`acme.slack.com`,
/// `acme.enterprise.slack.com`) or a link from the browser (`https://acme.slack.com/archives/…`).
pub fn workspace_url(input: &str) -> Result<String, String> {
    let text = input.trim().to_lowercase();
    let rest = text.strip_prefix("https://").or_else(|| text.strip_prefix("http://")).unwrap_or(&text);
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if host.is_empty() {
        return Err("type the workspace's name, like acme".into());
    }
    if ["slack.com", "app.slack.com", "www.slack.com"].contains(&host) {
        return Err("that link doesn't name the workspace — type its name".into());
    }
    let host = if host.contains('.') { host.to_string() } else { format!("{host}.slack.com") };
    let name_ok = host.split('.').all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    if !name_ok {
        return Err(format!("“{}” is not a workspace name — letters, digits and - only", input.trim()));
    }
    if !host.ends_with(".slack.com") && !host.ends_with(".slack-gov.com") {
        return Err(format!("{host} is not a Slack address — type the workspace's name, like acme"));
    }
    Ok(format!("https://{host}/"))
}

/// The host of a workspace's address: `acme.slack.com` of `https://acme.slack.com/`.
fn host_of(url: &str) -> String {
    url.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/').to_lowercase()
}

/// An item that can be picked when `enabled`.
fn item(text: &str, detail: &str, note: &str, enabled: bool) -> Row {
    Row::Item(Item { text: text.into(), detail: detail.into(), note: note.into(), enabled })
}

/// The guide's list: a heading per browser profile (a dim one with its problem when it could not be
/// looked at), a row per workspace found there, and the last row for anything else.
pub fn rows(groups: &[Group], profile: &Profile) -> (Vec<Row>, Vec<Choice>) {
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
    rows.push(item(OTHER, "", "", true));
    choices.push(Choice::Other);
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

/// The one account of workspace `team` (id or name) in `p`; refused when there is none, or when
/// the profile holds two accounts there (they are listed).
pub fn one_account<'a>(p: &'a Profile, profile: &str, team: &str) -> Result<&'a WorkspaceSession> {
    match p.matching(team).as_slice() {
        [] => bail!("profile {profile} has no workspace {team}"),
        [one] => Ok(one),
        several => {
            let who: Vec<String> = several.iter().map(|w| format!("{} ({})", w.user_name, w.user_id)).collect();
            bail!("profile {profile} has {} accounts in {team}: {} — klack cannot tell which one yet", several.len(), who.join(", "))
        }
    }
}

/// Forget sign-ins of `profile` on this machine (Slack is not signed out): the account of `team`,
/// or every one with `all`. The profile stays, empty if nothing is left.
pub async fn logout(profile: &str, team: Option<&str>, all: bool) -> Result<()> {
    let mut p = Profile::load(profile)?;
    let gone: Vec<(String, String)> = match (team, all) {
        (Some(_), true) => bail!("say --team or --all, not both"),
        (None, false) => bail!("say --team or --all"),
        (None, true) => p.workspaces.iter().map(|w| (w.team_id.clone(), w.user_id.clone())).collect(),
        (Some(t), false) => {
            let w = one_account(&p, profile, t)?;
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
pub fn source_label(source: Option<&SessionSource>) -> &str {
    match source {
        Some(SessionSource::Browser { label, .. }) => label,
        Some(SessionSource::Window) => "browser window",
        Some(SessionSource::Cookie) => "cookie",
        None => "unknown",
    }
}

/// Whether the session still works: `works`, `signed out`, or `can't check: <why>` (a network
/// failure says nothing about the session).
async fn check(ws: &WorkspaceSession) -> String {
    let answer = tokio::time::timeout(CHECK_TIMEOUT, async { slack_core::Client::new(ws.clone())?.auth_test().await }).await;
    match answer {
        Ok(Ok(_)) => "works".into(),
        Ok(Err(e)) => match e.downcast_ref::<slack_core::api::ApiError>() {
            Some(a) if ["invalid_auth", "not_authed", "token_revoked", "account_inactive"].contains(&a.error.as_str()) => "signed out".into(),
            _ => format!("can't check: {e:#}"),
        },
        Err(_) => format!("can't check: no answer in {} s", CHECK_TIMEOUT.as_secs()),
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
    let states = futures_util::future::join_all(p.workspaces.iter().map(check)).await;
    println!("profile {profile}:");
    for (w, state) in p.workspaces.iter().zip(states) {
        let mark = if start.as_ref().is_some_and(|(t, u)| *t == w.team_id && *u == w.user_id) { '★' } else { ' ' };
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
        assert!(e.contains("2 accounts") && e.contains("person-U0000000001 (U0000000001)") && e.contains("U0000000002"), "{e}");
        assert!(one_account(&p, "default", "nope").is_err());
    }

    use crate::login_import::{BrowserProfile, Candidate, Source};
    use slack_core::session::LoggedInTeam;

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
    fn workspace_url_takes_names_addresses_and_links() {
        for typed in ["acme", "acme.slack.com", "https://acme.slack.com/archives/C1", " Acme ", "ACME.slack.com/", "http://acme.slack.com"] {
            assert_eq!(workspace_url(typed).as_deref(), Ok("https://acme.slack.com/"), "{typed:?}");
        }
        assert_eq!(workspace_url("acme.enterprise.slack.com").as_deref(), Ok("https://acme.enterprise.slack.com/"));
        assert_eq!(workspace_url("my-team2").as_deref(), Ok("https://my-team2.slack.com/"));
    }

    #[test]
    fn workspace_url_refuses_what_names_no_workspace() {
        assert_eq!(workspace_url("https://app.slack.com/client/T1/C1"), Err("that link doesn't name the workspace — type its name".to_string()));
        assert!(workspace_url("app.slack.com").is_err());
        for bad in ["", "   ", "a b", "acme.example.com", "acme..slack.com", "ac_me"] {
            assert!(workspace_url(bad).is_err(), "{bad:?}");
        }
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
        let (rows, choices) = rows(&groups, &profile);
        assert_eq!(
            shown(&rows),
            [
                "# Chrome · Work | person@example.com",
                "- Acme | acme.slack.com | as person-U0000000001",
                "- Widgets | widgets.slack.com | signed out in Chrome · Work (dim)",
                "# Firefox · default | ",
                "- Widgets | widgets.slack.com | as person-U0000000001  already signed in",
                "#  | ",
                "- another workspace, or sign in another way… |  | ",
            ]
        );
        assert_eq!(choices, [Choice::Candidate(0, 0), Choice::Candidate(0, 1), Choice::Candidate(2, 0), Choice::Other]);
    }

    #[test]
    fn a_problem_group_is_a_dim_heading() {
        let groups = vec![group("Chrome · Work", Some("person@example.com"), Some("can't read Chrome's key: the keyring is locked"), vec![])];
        let (rows, choices) = rows(&groups, &Profile::default());
        assert_eq!(shown(&rows), ["#  | Chrome · Work  person@example.com — can't read Chrome's key: the keyring is locked", "#  | ", "- another workspace, or sign in another way… |  | "]);
        assert_eq!(choices, [Choice::Other]);
    }

    #[test]
    fn nothing_found_still_offers_another_way() {
        let (rows, choices) = rows(&[], &Profile::default());
        assert_eq!(shown(&rows), ["- another workspace, or sign in another way… |  | "]);
        assert_eq!(choices, [Choice::Other]);
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
    fn picked_rows_map_to_their_items() {
        let rows = vec![Row::Heading { title: "A".into(), detail: String::new() }, item("a", "", "", true), item("b", "", "", true), Row::Heading { title: String::new(), detail: String::new() }, item("c", "", "", true)];
        let items = item_rows(&rows);
        assert_eq!(picked_items(&items, &['a', 'b', 'c'], &[1, 4]), ['a', 'c']);
        assert_eq!(picked_items(&items, &['a', 'b', 'c'], &[0]), Vec::<char>::new());
    }

    #[test]
    fn sources_are_named() {
        let chrome = SessionSource::Browser { browser: "chrome".into(), profile: "/x".into(), label: "Chrome · Work".into() };
        assert_eq!(source_label(Some(&chrome)), "Chrome · Work");
        assert_eq!(source_label(Some(&SessionSource::Window)), "browser window");
        assert_eq!(source_label(Some(&SessionSource::Cookie)), "cookie");
        assert_eq!(source_label(None), "unknown");
    }
}

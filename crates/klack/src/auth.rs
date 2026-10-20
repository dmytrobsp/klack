//! `klack auth login | logout | status`: signing a profile in to workspaces, forgetting a sign-in
//! on this machine and checking which still work. Sessions are kept per team + user, so one
//! profile may hold two accounts of one workspace.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use slack_core::session::{self, Profile, SessionSource, WorkspaceSession};

use crate::{login_browser, login_import};

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

/// Sign `profile` in to the workspace of `flags` and save the session: from a browser (when
/// asked), in a browser window, or with the `d` cookie (flag, `$KLACK_COOKIE_D` or asked for).
pub async fn login(profile: &str, flags: LoginFlags) -> Result<()> {
    let LoginFlags { workspace, cookie, no_browser, from_browser, browser_profile } = flags;
    // Only a missing file is a new profile: one that cannot be read is not replaced.
    let mut saved = Profile::read_or_new(&session::config_dir(profile)?.join("session.json"))?;
    let url = workspace.ok_or_else(|| anyhow!("which workspace? add --workspace acme.slack.com"))?;
    let url = if url.starts_with("https://") { url } else { format!("https://{url}") };
    let given = cookie.or_else(|| std::env::var("KLACK_COOKIE_D").ok());
    // No cookie given: take it from a browser (when asked), sign in in a browser window,
    // or ask for it.
    let mut signed_in = None;
    if let Some(source) = from_browser {
        let found = login_import::find(source, browser_profile.as_deref()).await.with_context(|| format!("reading {}'s cookies", login_import::label(source)))?;
        if found.is_empty() {
            bail!("no Slack session in {} — sign in to Slack there first", login_import::label(source));
        }
        // Every profile that signs in to the workspace, one per account (freshest first).
        let mut working: Vec<(PathBuf, WorkspaceSession)> = Vec::new();
        for f in &found {
            match session::session_from_cookie(&url, &f.cookie).await {
                Ok(s) if !working.iter().any(|(_, w)| w.user_id == s.user_id) => working.push((f.profile.clone(), s)),
                Ok(_) => {}
                Err(e) => tracing::debug!("{}: {e:#}", f.profile.display()),
            }
        }
        if working.is_empty() {
            bail!("{} has Slack cookies, but none signs in to {url} (signed out, or another workspace)", login_import::label(source));
        }
        let pick = if working.len() > 1 && std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            eprintln!("{} profiles sign in to {url}:", working.len());
            for (n, (p, s)) in working.iter().enumerate() {
                eprintln!("  {}) {} — {}", n + 1, login_import::profile_at(source, p).label, s.user_name);
            }
            loop {
                eprint!("which one? [1-{}] ", working.len());
                let mut line = String::new();
                if std::io::stdin().read_line(&mut line)? == 0 {
                    bail!("no profile chosen");
                }
                match line.trim().parse::<usize>() {
                    Ok(n) if (1..=working.len()).contains(&n) => break n - 1,
                    _ => eprintln!("a number from 1 to {}, please", working.len()),
                }
            }
        } else {
            // Not asked: the freshest (the list starts with it).
            0
        };
        let (dir, mut s) = working.swap_remove(pick);
        let label = login_import::profile_at(source, &dir).label;
        eprintln!("took the session from {label}");
        s.source = Some(SessionSource::Browser { browser: login_import::label(source).to_lowercase(), profile: dir, label });
        signed_in = Some(s);
    } else if given.is_none() && !no_browser {
        match login_browser::find() {
            Some(browser) => {
                eprintln!("Opening {} to sign in to {url} — sign in there; the window closes by itself (Ctrl+C cancels).", browser.label);
                match login_browser::sign_in(&browser, &url).await {
                    Ok(s) => signed_in = Some(WorkspaceSession { source: Some(SessionSource::Window), ..s }),
                    Err(e) => eprintln!("browser sign-in did not finish: {e:#}"),
                }
            }
            None => eprintln!("{}", login_browser::NoBrowser),
        }
    }
    let ws = match signed_in {
        Some(s) => s,
        None => {
            let cookie = match given {
                Some(c) => c,
                None => {
                    eprint!("value of the `d` cookie (devtools → Application → Cookies → slack.com): ");
                    let mut line = String::new();
                    std::io::stdin().read_line(&mut line)?;
                    line
                }
            };
            let s = session::session_from_cookie(&url, cookie.trim().trim_start_matches("d=")).await.context("sign-in with the cookie failed")?;
            WorkspaceSession { source: Some(SessionSource::Cookie), ..s }
        }
    };
    println!("signed in to {} ({}) as {} ({})", ws.team_name, ws.team_id, ws.user_name, ws.user_id);
    saved.upsert(ws);
    let path = saved.save(profile)?;
    println!("session saved to {}", path.display());
    Ok(())
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

    #[test]
    fn sources_are_named() {
        let chrome = SessionSource::Browser { browser: "chrome".into(), profile: "/x".into(), label: "Chrome · Work".into() };
        assert_eq!(source_label(Some(&chrome)), "Chrome · Work");
        assert_eq!(source_label(Some(&SessionSource::Window)), "browser window");
        assert_eq!(source_label(Some(&SessionSource::Cookie)), "cookie");
        assert_eq!(source_label(None), "unknown");
    }
}

//! The way "in your browser": ttyak notes what every browser profile is signed in to, then watches the profiles'
//! cookie files while the person signs in to Slack in a browser of their own, and gives the new sign-in.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;

use crate::login_import::{self, BrowserProfile, Group, Keys};

/// How often the cookie files are looked at.
pub const TICK: Duration = Duration::from_secs(2);
/// How long ttyak watches before it gives up.
pub const GIVE_UP: Duration = Duration::from_secs(600);
/// How long after a problem on Slack's side (no answer, an error, its page changed) a profile is asked about again
/// when its `d` cookies have not changed meanwhile.
const RETRY_AFTER: Duration = Duration::from_secs(30);

/// A sign-in of a browser profile as ttyak saw it: the profile, the account, and a hash of its `d` cookie (never the
/// cookie itself), so a new session of the same account is told apart.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Seen {
    profile: PathBuf,
    team_id: String,
    user_id: String,
    cookie: u64,
}

impl Seen {
    fn of(profile: &BrowserProfile, s: &slack_core::session::WorkspaceSession) -> Seen {
        Seen { profile: profile.dir.clone(), team_id: s.team_id.clone(), user_id: s.user_id.clone(), cookie: login_import::cookie_hash(&s.cookie_d) }
    }
}

/// What the browser profiles were signed in to when ttyak started watching.
#[derive(Debug, Clone)]
pub struct Baseline(HashSet<Seen>);

/// The sign-ins of `groups`: each session that works, and each that did not (with no user and no cookie).
pub fn baseline(groups: &[Group]) -> Baseline {
    let mut seen = HashSet::new();
    for g in groups {
        for c in &g.candidates {
            seen.insert(match &c.session {
                Ok(s) => Seen::of(&g.profile, s),
                Err(_) => Seen { profile: g.profile.dir.clone(), team_id: c.team.team_id.clone(), user_id: String::new(), cookie: 0 },
            });
        }
    }
    Baseline(seen)
}

/// The sign-ins of `now` that work and were not there `before`, as (group, candidate).
pub fn new_ones(before: &Baseline, now: &[Group]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (g, group) in now.iter().enumerate() {
        for (c, found) in group.candidates.iter().enumerate() {
            if let Ok(s) = &found.session
                && !before.0.contains(&Seen::of(&group.profile, s))
            {
                out.push((g, c));
            }
        }
    }
    out
}

/// Calls `restore` once when it is dropped: the terminal is put back on every way out of the wait (Esc, an error,
/// a panic).
struct Restore<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> Restore<F> {
    fn new(restore: F) -> Restore<F> {
        Restore(Some(restore))
    }
}

impl<F: FnOnce()> Drop for Restore<F> {
    fn drop(&mut self) {
        if let Some(restore) = self.0.take() {
            restore();
        }
    }
}

/// Raw mode (keys are not echoed and come one at a time) until the guard is dropped.
fn raw_mode() -> Result<Restore<impl FnOnce()>> {
    crossterm::terminal::enable_raw_mode()?;
    Ok(Restore::new(|| {
        let _ = crossterm::terminal::disable_raw_mode();
    }))
}

/// Watches the browser profiles until one has a sign-in that is not in `before`, and gives the profiles looked at
/// then (each without a `problem`). `keys` are the Chrome-family browsers' keys, read once already. `None` on Esc or
/// Ctrl+C, or after [`GIVE_UP`]. The terminal is in raw mode meanwhile (what is typed is not echoed); a resize is
/// told to `resized(width, height)`, with the cursor at the start of the line under the screen.
pub async fn watch(before: Baseline, keys: &Keys, resized: &mut dyn FnMut(u16, u16) -> Result<()>) -> Result<Option<Vec<Group>>> {
    let _raw = raw_mode()?;
    let mut events = EventStream::new();
    let found = look_for(&before, keys);
    tokio::pin!(found);
    let give_up = tokio::time::sleep(GIVE_UP);
    tokio::pin!(give_up);
    let end = loop {
        tokio::select! {
            groups = &mut found => break Ok(Some(groups)),
            _ = &mut give_up => break Ok(None),
            ev = events.next() => match ev {
                Some(Ok(Event::Key(k))) if k.kind != KeyEventKind::Release && (k.code == KeyCode::Esc || (k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL))) => break Ok(None),
                Some(Ok(Event::Resize(w, h))) => {
                    if let Err(e) = resized(w, h) {
                        break Err(e);
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => break Err(e.into()),
                None => break Ok(None),
            },
        }
    };
    if let Err(e) = close(events).await {
        tracing::debug!("after the wait: {e:#}");
    }
    end
}

/// Drops `events` and takes the wake-up its drop leaves behind. crossterm's `EventStream` wakes the terminal's reader
/// when it is dropped; when nothing waits on the reader then (the stream's last key was taken, as with Esc), the
/// wake-up stays, and the next read — the picker asking where the cursor is — comes back empty at once and fails
/// ("The cursor position could not be read"). Once the stream's thread has ended (a moment), a poll takes it, with
/// the keys typed meanwhile.
async fn close(events: EventStream) -> Result<()> {
    drop(events);
    tokio::time::sleep(Duration::from_millis(50)).await;
    while crossterm::event::poll(Duration::ZERO)? {
        let _ = crossterm::event::read()?;
    }
    Ok(())
}

/// Whether Slack is asked about a profile whose `d` cookies (hashes) are `now` and were `before` (`None`: not read
/// before, a profile made during the wait): when one of them is new. A write of the cookie file that leaves them
/// as they were (another site's cookie, a visit), or a sign-out, asks nothing.
fn to_check(before: Option<&HashSet<u64>>, now: &HashSet<u64>) -> bool {
    match before {
        Some(before) => now.iter().any(|h| !before.contains(h)),
        None => !now.is_empty(),
    }
}

/// What ttyak knows of a watched profile: when its cookies were last written and read, its `d` cookies (hashes)
/// then, and when Slack may be asked about it again after a problem on Slack's side.
#[derive(Default)]
struct Watched {
    stamp: Option<SystemTime>,
    hashes: Option<HashSet<u64>>,
    retry: Option<tokio::time::Instant>,
}

/// Every [`TICK`] the profiles whose cookie files changed (or that are new since the start) have their `d` cookies
/// read, here only; Slack is asked about a profile when one of them is new ([`to_check`]), until one has a sign-in
/// that is not in `before`; the profiles asked about then are given. A cookie file that can't be read (being
/// written) is read again at the next tick; a profile Slack could not answer about is asked again after
/// [`RETRY_AFTER`], or sooner when a `d` cookie comes. A Chrome-family profile whose browser has no key is left out.
async fn look_for(before: &Baseline, keys: &Keys) -> Vec<Group> {
    let mut watched: HashMap<PathBuf, Watched> = HashMap::new();
    for p in login_import::browser_profiles().into_iter().filter(|p| keys.has(p)) {
        let stamp = login_import::stamp(&p);
        let hashes = login_import::d_hashes(&p, keys).await.ok();
        // Not read: read at the next tick.
        let stamp = if hashes.is_some() { stamp } else { None };
        watched.insert(p.dir.clone(), Watched { stamp, hashes, retry: None });
    }
    let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + TICK, TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let now = tokio::time::Instant::now();
        let mut ask = Vec::new();
        for p in login_import::browser_profiles().into_iter().filter(|p| keys.has(p)) {
            let stamp = login_import::stamp(&p);
            let w = watched.entry(p.dir.clone()).or_default();
            let mut asking = w.retry.is_some_and(|at| now >= at);
            if w.stamp != stamp {
                match login_import::d_hashes(&p, keys).await {
                    Ok(hashes) => {
                        asking |= to_check(w.hashes.as_ref(), &hashes);
                        (w.stamp, w.hashes) = (stamp, Some(hashes));
                    }
                    Err(e) => tracing::debug!("{}: cookies not read this time: {e:#}", p.label),
                }
            }
            if asking {
                w.retry = None;
                ask.push(p);
            }
        }
        if ask.is_empty() {
            continue;
        }
        for p in &ask {
            tracing::debug!("{}: a new d cookie — checking it with Slack", p.label);
        }
        let mut groups = Vec::new();
        for g in login_import::discover_with_keys(ask, keys).await {
            match &g.problem {
                Some(problem) => {
                    tracing::debug!("{}: {problem} — asking again in {} s", g.profile.label, RETRY_AFTER.as_secs());
                    if let Some(w) = watched.get_mut(&g.profile.dir) {
                        w.retry = Some(now + RETRY_AFTER);
                    }
                }
                None => groups.push(g),
            }
        }
        if !new_ones(before, &groups).is_empty() {
            return groups;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::login_import::{self, BrowserProfile, Candidate, Group, Source};
    use std::collections::HashSet;
    use slack_core::session::{LoggedInTeam, SessionSource, WorkspaceSession};

    /// A candidate of team `team` signed in as `user` with the cookie `cookie`, or signed out (`None`).
    fn candidate(team: &str, name: &str, signed_in: Option<(&str, &str)>) -> Candidate {
        let domain = name.to_lowercase();
        let t = LoggedInTeam { team_id: team.into(), team_name: name.into(), domain: domain.clone(), url: format!("https://{domain}.slack.com/"), email: "person@example.com".into() };
        let session = match signed_in {
            Some((user, cookie)) => Ok(WorkspaceSession {
                team_id: team.into(),
                team_name: name.into(),
                url: t.url.clone(),
                user_id: user.into(),
                user_name: format!("person-{user}"),
                cookie_d: cookie.into(),
                source: Some(SessionSource::Browser { browser: "firefox".into(), profile: "/p".into(), label: "Firefox · default".into() }),
                ..Default::default()
            }),
            None => Err("auth.test failed: invalid_auth".to_string()),
        };
        Candidate { team: t, session }
    }

    fn group(dir: &str, candidates: Vec<Candidate>) -> Group {
        Group { profile: BrowserProfile { source: Source::Firefox, dir: PathBuf::from(dir), label: format!("Firefox · {dir}"), account: None }, problem: None, candidates }
    }

    #[test]
    fn a_new_session_is_new() {
        let before = baseline(&[group("/p1", vec![candidate("T0000000001", "Acme", Some(("U0000000001", "xoxd-x")))])]);
        // Another workspace signed in to in the same profile.
        let now = [group("/p1", vec![candidate("T0000000001", "Acme", Some(("U0000000001", "xoxd-x"))), candidate("T0000000002", "Widgets", Some(("U0000000001", "xoxd-y")))])];
        assert_eq!(new_ones(&before, &now), [(0, 1)]);
        // Signed in again to the same account: a new cookie.
        let now = [group("/p1", vec![candidate("T0000000001", "Acme", Some(("U0000000001", "xoxd-z")))])];
        assert_eq!(new_ones(&before, &now), [(0, 0)]);
        // Signed out when ttyak looked first, signed in now.
        let before = baseline(&[group("/p1", vec![candidate("T0000000001", "Acme", None)])]);
        let now = [group("/p1", vec![candidate("T0000000001", "Acme", Some(("U0000000001", "xoxd-x")))])];
        assert_eq!(new_ones(&before, &now), [(0, 0)]);
    }

    #[test]
    fn a_rewrite_without_a_sign_in_offers_nothing() {
        // The browser wrote its cookies (another site, a visit) but the Slack sign-ins are the same.
        let groups = || vec![group("/p1", vec![candidate("T0000000001", "Acme", Some(("U0000000001", "xoxd-x"))), candidate("T0000000002", "Widgets", None)]), group("/p2", vec![candidate("T0000000001", "Acme", Some(("U0000000002", "xoxd-w")))])];
        let before = baseline(&groups());
        assert_eq!(new_ones(&before, &groups()), Vec::<(usize, usize)>::new());
        // Still signed out is nothing new either.
        assert_eq!(new_ones(&before, &[group("/p1", vec![candidate("T0000000002", "Widgets", None)])]), Vec::<(usize, usize)>::new());
    }

    #[test]
    fn a_profile_made_during_the_wait_is_watched() {
        let before = baseline(&[group("/p1", vec![candidate("T0000000001", "Acme", Some(("U0000000001", "xoxd-x")))])]);
        // The same account and cookie, in a profile made after ttyak looked: a sign-in of that profile.
        let now = [group("/new", vec![candidate("T0000000001", "Acme", Some(("U0000000001", "xoxd-x"))), candidate("T0000000002", "Widgets", None), candidate("T0000000003", "Gamma", Some(("U0000000001", "xoxd-g")))])];
        assert_eq!(new_ones(&before, &now), [(0, 0), (0, 2)]);
    }

    #[test]
    fn the_baseline_keeps_no_cookie() {
        let before = baseline(&[group("/p1", vec![candidate("T0000000001", "Acme", Some(("U0000000001", "xoxd-secret-value"))), candidate("T0000000002", "Widgets", None)])]);
        let shown = format!("{before:?}");
        assert!(!shown.contains("xoxd") && !shown.contains("secret"), "{shown}");
        assert!(shown.contains("T0000000001") && shown.contains("T0000000002"), "{shown}");
    }

    #[test]
    fn waiting_gives_back_the_terminal() {
        use std::cell::Cell;
        // Left the normal way.
        let restored = Cell::new(false);
        drop(Restore::new(|| restored.set(true)));
        assert!(restored.get());
        // Left by a panic (an error is the same path: the guard is dropped).
        let restored = std::sync::atomic::AtomicBool::new(false);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _raw = Restore::new(|| restored.store(true, std::sync::atomic::Ordering::SeqCst));
            panic!("while waiting");
        }));
        assert!(r.is_err());
        assert!(restored.load(std::sync::atomic::Ordering::SeqCst));
    }

    fn hashes(cookies: &[&str]) -> HashSet<u64> {
        cookies.iter().map(|c| login_import::cookie_hash(c)).collect()
    }

    #[test]
    fn slack_is_asked_only_when_a_d_cookie_comes() {
        let before = hashes(&["xoxd-a", "xoxd-b"]);
        // The file was written (another site's cookie, a visit), the d cookies are the same: nothing to ask.
        assert!(!to_check(Some(&before), &hashes(&["xoxd-b", "xoxd-a"])));
        // One went (signed out): nothing new to ask about either.
        assert!(!to_check(Some(&before), &hashes(&["xoxd-a"])));
        // A new one: asked.
        assert!(to_check(Some(&before), &hashes(&["xoxd-a", "xoxd-c"])));
        assert!(to_check(Some(&HashSet::new()), &hashes(&["xoxd-c"])));
        // A profile ttyak has not read before: asked when it has a d cookie, not when it has none.
        assert!(to_check(None, &hashes(&["xoxd-a"])));
        assert!(!to_check(None, &HashSet::new()));
    }

    #[test]
    fn a_slack_problem_is_retried_after_a_while_not_every_tick() {
        assert!(RETRY_AFTER >= Duration::from_secs(30));
        assert!(RETRY_AFTER > TICK * 10);
    }

    #[test]
    fn the_seen_cookie_is_the_watched_hash() {
        let g = group("/p1", vec![candidate("T0000000001", "Acme", Some(("U0000000001", "xoxd-x")))]);
        let seen: Vec<u64> = baseline(&[g]).0.into_iter().map(|s| s.cookie).collect();
        assert_eq!(seen, [login_import::cookie_hash("xoxd-x")]);
    }

    #[test]
    fn the_wait_is_short_ticks_for_ten_minutes() {
        assert_eq!((TICK, GIVE_UP), (std::time::Duration::from_secs(2), std::time::Duration::from_secs(600)));
    }
}

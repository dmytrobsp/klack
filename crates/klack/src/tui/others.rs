//! The profile's other workspaces (only the shown one has the UI): their unread state, polled
//! with `client.counts` and shown in the status bar, and their desktop notifications, from a
//! headless watcher per workspace (boot data + RTM, the same notification rules).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use slack_core::api::Client;
use slack_core::session::WorkspaceSession;

use super::event_loop::SignedOut;

/// How often the other workspaces are asked.
const POLL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Badge {
    /// Unread DMs, mentions in channels and in threads (Slack's `channel_badges`).
    pub attention: u64,
    /// Anything unread at all.
    pub unread: bool,
}

/// The badge of a `client.counts` answer.
pub fn badge(counts: &Value) -> Badge {
    let b = &counts["channel_badges"];
    let n = |k: &str| b[k].as_u64().unwrap_or(0);
    let any = ["channels", "mpims", "ims"].iter().flat_map(|k| counts[*k].as_array().into_iter().flatten()).any(|c| c["has_unreads"].as_bool() == Some(true));
    Badge { attention: n("dms") + n("channels") + n("thread_mentions"), unread: any || n("thread_unreads") > 0 }
}

/// Badges by workspace index, shared with the poller.
pub type Badges = Arc<Mutex<HashMap<usize, Badge>>>;

/// Workspace logos by index (only workspaces with a logo of their own), for the rail.
pub type Logos = Arc<Mutex<HashMap<usize, slack_core::types::File>>>;

/// A workspace's logo from `team.info`'s `icon`, as a pseudo file for the picture cache:
/// None for Slack's stand-in picture (`image_default`) or one not on Slack's CDN.
pub fn logo_file(team: &str, icon: &Value) -> Option<slack_core::types::File> {
    if icon["image_default"].as_bool() != Some(false) {
        return None;
    }
    let url = ["image_88", "image_102", "image_68", "image_132"].iter().find_map(|k| icon[*k].as_str())?;
    let parsed = slack_core::reqwest::Url::parse(url).ok()?;
    let slack_hosted = parsed.scheme() == "https" && parsed.host_str().is_some_and(|h| h == "slack-edge.com" || h.ends_with(".slack-edge.com"));
    // Fetched like a profile picture: without the session, cut to fill its cells.
    slack_hosted.then(|| slack_core::types::File { id: format!("{}team:{team}", super::media::AVATAR_PREFIX), mimetype: "image/png".into(), thumb_360: Some(url.to_string()), ..Default::default() })
}

/// Whether workspace `index` is signed out (its session ended): nothing is asked of it.
fn out(signed_out: &SignedOut, index: usize) -> bool {
    signed_out.lock().unwrap_or_else(|e| e.into_inner()).contains(&index)
}

/// Whether workspace `index` is signed out, first adding it to `signed_out` (the rail marks it)
/// when Slack answered `client` that its session has ended.
fn noticed(signed_out: &SignedOut, index: usize, client: &Client) -> bool {
    if client.signed_out() && signed_out.lock().unwrap_or_else(|e| e.into_inner()).insert(index) {
        tracing::warn!("workspace {} ({}): the session has ended", index + 1, client.session.team_name);
    }
    out(signed_out, index)
}

/// Drop the badges of the signed-out workspaces (nothing of theirs is up to date).
fn forget_signed_out(badges: &mut HashMap<usize, Badge>, signed_out: &std::collections::HashSet<usize>) {
    badges.retain(|i, _| !signed_out.contains(i));
}

/// Ask each workspace that is signed in for its logo once (nothing to do with a single workspace).
pub fn spawn_logos(sessions: Vec<WorkspaceSession>, logos: Logos, signed_out: SignedOut) {
    if sessions.len() < 2 {
        return;
    }
    tokio::spawn(async move {
        for (i, session) in sessions.into_iter().enumerate() {
            if out(&signed_out, i) {
                continue;
            }
            let team = session.team_id.clone();
            let Ok(client) = Client::new(session) else { continue };
            match client.call_raw("team.info", &[]).await {
                Ok(v) => {
                    if let Some(file) = logo_file(&team, &v["team"]["icon"]) {
                        logos.lock().unwrap_or_else(|e| e.into_inner()).insert(i, file);
                    }
                }
                Err(e) => {
                    tracing::debug!("team.info of workspace {}: {e}", i + 1);
                    noticed(&signed_out, i, &client);
                }
            }
        }
    });
}

/// Ask every workspace but the shown one (`shown`, kept current by the caller) and the signed-out
/// ones for its counts, now, every [`POLL`] and whenever `wake` is notified (a switch); nothing
/// to do with a single workspace.
pub fn spawn(sessions: Vec<WorkspaceSession>, shown: Arc<AtomicUsize>, badges: Badges, wake: Arc<tokio::sync::Notify>, signed_out: SignedOut) {
    if sessions.len() < 2 {
        return;
    }
    tokio::spawn(async move {
        let clients: Vec<Option<Client>> = sessions.into_iter().map(|s| Client::new(s).ok()).collect();
        loop {
            for (i, client) in clients.iter().enumerate() {
                let Some(client) = client.as_ref().filter(|_| i != shown.load(Ordering::Relaxed) && !out(&signed_out, i)) else { continue };
                match client.counts().await {
                    Ok(v) => {
                        badges.lock().unwrap_or_else(|e| e.into_inner()).insert(i, badge(&v));
                    }
                    Err(e) => {
                        tracing::debug!("counts of workspace {}: {e}", i + 1);
                        noticed(&signed_out, i, client);
                    }
                }
            }
            // Also those another part of klack found signed out since.
            let out = signed_out.lock().unwrap_or_else(|e| e.into_inner()).clone();
            forget_signed_out(&mut badges.lock().unwrap_or_else(|e| e.into_inner()), &out);
            tokio::select! {
                _ = tokio::time::sleep(POLL) => {}
                _ = wake.notified() => {}
            }
        }
    });
}

/// A click on a notification from workspace `.0`.
pub type Click = (usize, super::notify::Target);

/// Where a click from another workspace leads once that workspace has booted.
pub type OpenAfter = Arc<Mutex<Option<(usize, super::applink::Target)>>>;

/// How to join a huddle of workspace `index` from another one ("Alt+2, then Space h h,"), with
/// the keys bound now.
fn join_keys(keys: &super::keymap::Keymap, index: usize) -> String {
    use super::keymap::{Action::*, Context};
    let switch = [Workspace1, Workspace2, Workspace3, Workspace4, Workspace5, Workspace6, Workspace7, Workspace8, Workspace9].get(index).and_then(|a| keys.first_key(Context::Global, *a));
    let huddle = keys.first_key(Context::Global, Huddle).unwrap_or_default();
    match switch {
        Some(switch) => format!("{switch}, then {huddle},"),
        None => format!("{huddle} in its workspace"),
    }
}

/// Start a headless watcher for every workspace of a profile with several; each runs only
/// while its workspace is not the shown one (that one notifies itself), and stops for good once
/// its workspace is signed out.
pub fn spawn_watchers(sessions: Vec<WorkspaceSession>, labels: Vec<String>, shown: Arc<AtomicUsize>, clicks: tokio::sync::mpsc::UnboundedSender<Click>, signed_out: SignedOut) {
    if sessions.len() < 2 {
        return;
    }
    for (index, (session, label)) in sessions.into_iter().zip(labels).enumerate() {
        if out(&signed_out, index) {
            continue;
        }
        let (shown, clicks, signed_out) = (shown.clone(), clicks.clone(), signed_out.clone());
        tokio::spawn(async move {
            if let Err(e) = watch(index, session, label, shown, clicks, signed_out).await {
                tracing::warn!("watching workspace {}: {e}", index + 1);
            }
        });
    }
}

/// How often a watcher checks whether its workspace became the shown one (or stopped being it).
const SHOWN_CHECK: Duration = Duration::from_secs(1);

/// Watch workspace `index` while it is not the shown one; while it is shown the app has its
/// own connection, so the watcher holds none (no RTM, no copy of the users) and boots afresh
/// when the user switches away.
async fn watch(index: usize, session: WorkspaceSession, label: String, shown: Arc<AtomicUsize>, clicks: tokio::sync::mpsc::UnboundedSender<Click>, signed_out: SignedOut) -> anyhow::Result<()> {
    let client = Client::new(session)?;
    loop {
        while shown.load(Ordering::Relaxed) == index {
            tokio::time::sleep(SHOWN_CHECK).await;
        }
        if out(&signed_out, index) {
            tracing::debug!("workspace {} is signed out: its watcher stops", index + 1);
            return Ok(());
        }
        tracing::debug!("watching workspace {}", index + 1);
        match watch_hidden(index, &client, &label, &shown, &clicks, &signed_out).await {
            Ok(()) => tracing::debug!("workspace {} shown: its watcher pauses", index + 1),
            Err(_) if noticed(&signed_out, index, &client) => {}
            Err(e) => {
                tracing::warn!("watching workspace {}: {e}", index + 1);
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        }
    }
}

/// Boot workspace `index` into its own state and notify from its RTM events until it becomes
/// the shown one or is signed out (then the connection and the state are dropped).
async fn watch_hidden(index: usize, client: &Client, label: &str, shown: &AtomicUsize, clicks: &tokio::sync::mpsc::UnboundedSender<Click>, signed_out: &SignedOut) -> anyhow::Result<()> {
    use super::state::Workspace;
    let (boot, users) = tokio::join!(client.user_boot(), client.users_list());
    let mut ws = Workspace::default();
    for u in users? {
        ws.users.insert(u.id.clone(), u);
    }
    ws.apply_boot(&boot?);
    let mut rtm = slack_core::rtm::spawn(client.clone());
    let mut check = tokio::time::interval(SHOWN_CHECK);
    loop {
        let ev = tokio::select! {
            ev = rtm.events.recv() => ev.ok_or_else(|| anyhow::anyhow!("RTM stopped"))?,
            _ = check.tick() => {
                if shown.load(Ordering::Relaxed) == index || noticed(signed_out, index, client) {
                    return Ok(());
                }
                continue;
            }
        };
        let v = match ev {
            slack_core::rtm::RtmEvent::Event(v) => v,
            slack_core::rtm::RtmEvent::SignedOut => {
                noticed(signed_out, index, client);
                return Ok(());
            }
            _ => continue,
        };
        let fx = super::live::apply(&mut ws, &v, None);
        if shown.load(Ordering::Relaxed) == index {
            return Ok(());
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64();
        if ws.dnd.stale(now) {
            match client.call_raw("dnd.info", &[]).await {
                Ok(d) => ws.dnd.set_info(&d),
                Err(e) => tracing::debug!("dnd.info of workspace {}: {e}", index + 1),
            }
        }
        let note = match (&fx.incoming, &fx.huddle_started) {
            (Some((channel, m)), _) => super::notify::notification(&ws, channel, m, false, now).map(|n| (n, super::notify::Target::message(channel, m))),
            (None, Some((channel, user))) => super::notify::huddle_notification(&ws, channel, user, false, now, &join_keys(&super::keymap::load_user().0, index))
                .map(|n| (n, super::notify::Target { channel: channel.clone(), ts: None, thread_ts: None })),
            _ => None,
        };
        let Some(((title, body), target)) = note else {
            if let Some((channel, _)) = &fx.incoming {
                tracing::debug!(workspace = index + 1, channel, dnd = ws.dnd_active(now), "message from another workspace: no notification");
            }
            continue;
        };
        let (settings, _) = super::settings::load();
        tracing::debug!(workspace = index + 1, channel = target.channel, "notification from another workspace");
        if settings.notifications.desktop {
            let clicks = clicks.clone();
            super::notify::show(format!("{title} · {label}"), body, &settings, Some(Box::new(move || {
                let _ = clicks.send((index, target));
            })));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logos_only_of_their_own() {
        let stand_in = serde_json::json!({"image_88": "https://a.slack-edge.com/80588/img/avatars-teams/ava_0016-88.png", "image_default": true});
        assert!(logo_file("T1", &stand_in).is_none(), "Slack's stand-in picture: the initials instead");
        let own = serde_json::json!({"image_68": "https://avatars.slack-edge.com/2026-10-19/1_68.png", "image_88": "https://avatars.slack-edge.com/2026-10-19/1_88.png", "image_default": false});
        let file = logo_file("T1", &own).unwrap();
        assert_eq!((file.id.as_str(), file.thumb_360.as_deref()), ("avatar:team:T1", Some("https://avatars.slack-edge.com/2026-10-19/1_88.png")));
        let elsewhere = serde_json::json!({"image_88": "https://example.com/logo.png", "image_default": false});
        assert!(logo_file("T1", &elsewhere).is_none(), "nothing fetched from third parties");
        assert!(logo_file("T1", &serde_json::Value::Null).is_none());
    }

    #[test]
    fn joining_from_elsewhere_names_the_bound_keys() {
        let keys = crate::tui::keymap::Keymap::load("").0;
        assert_eq!(join_keys(&keys, 1), "Alt+2, then Space h h,");
        let rebound = crate::tui::keymap::Keymap::load("[global]\nhuddle = \"f3\"\nworkspace_2 = \"ctrl+2\"\n").0;
        assert_eq!(join_keys(&rebound, 1), "Ctrl+2, then F3,");
    }

    #[test]
    fn badges_from_counts() {
        // Trimmed from a real `client.counts` answer (B's account).
        let v = serde_json::json!({"ok":true,
            "channels":[{"id":"C0CA2V1L12A","has_unreads":false,"mention_count":0}],
            "ims":[{"id":"D0CA2V3DES2","has_unreads":true,"mention_count":0}], "mpims":[],
            "channel_badges":{"app_dms":0,"channels":0,"dms":7,"thread_mentions":0,"thread_unreads":2}});
        assert_eq!(badge(&v), Badge { attention: 7, unread: true });
        let quiet = serde_json::json!({"channels":[{"has_unreads":false}],"channel_badges":{"dms":0,"channels":0,"thread_mentions":0,"thread_unreads":0}});
        assert_eq!(badge(&quiet), Badge::default());
    }

    #[test]
    fn signed_out_workspaces_lose_their_badges() {
        let mut badges = HashMap::from([(1, Badge { attention: 3, unread: true }), (2, Badge { attention: 0, unread: true })]);
        forget_signed_out(&mut badges, &std::collections::HashSet::from([1]));
        assert_eq!(badges.keys().copied().collect::<Vec<_>>(), [2], "the stale badge of workspace 2 is gone");
    }
}

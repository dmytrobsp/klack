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

/// Ask every workspace but the shown one (`shown`, kept current by the caller) for its counts,
/// now, every [`POLL`] and whenever `wake` is notified (a switch); nothing to do with a single
/// workspace.
pub fn spawn(sessions: Vec<WorkspaceSession>, shown: Arc<AtomicUsize>, badges: Badges, wake: Arc<tokio::sync::Notify>) {
    if sessions.len() < 2 {
        return;
    }
    tokio::spawn(async move {
        let clients: Vec<Option<Client>> = sessions.into_iter().map(|s| Client::new(s).ok()).collect();
        loop {
            for (i, client) in clients.iter().enumerate() {
                let Some(client) = client.as_ref().filter(|_| i != shown.load(Ordering::Relaxed)) else { continue };
                match client.counts().await {
                    Ok(v) => {
                        badges.lock().unwrap_or_else(|e| e.into_inner()).insert(i, badge(&v));
                    }
                    Err(e) => tracing::debug!("counts of workspace {}: {e}", i + 1),
                }
            }
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
pub type OpenAfter = Arc<Mutex<Option<Click>>>;

/// Start a headless watcher for every workspace of a profile with several; each notifies
/// only while its workspace is not the shown one (that one notifies itself).
pub fn spawn_watchers(sessions: Vec<WorkspaceSession>, labels: Vec<String>, shown: Arc<AtomicUsize>, clicks: tokio::sync::mpsc::UnboundedSender<Click>) {
    if sessions.len() < 2 {
        return;
    }
    for (index, (session, label)) in sessions.into_iter().zip(labels).enumerate() {
        let (shown, clicks) = (shown.clone(), clicks.clone());
        tokio::spawn(async move {
            if let Err(e) = watch(index, session, label, shown, clicks).await {
                tracing::warn!("watching workspace {}: {e}", index + 1);
            }
        });
    }
}

async fn watch(index: usize, session: WorkspaceSession, label: String, shown: Arc<AtomicUsize>, clicks: tokio::sync::mpsc::UnboundedSender<Click>) -> anyhow::Result<()> {
    use super::state::{Dnd, Workspace};
    let client = Client::new(session)?;
    let (boot, users) = tokio::join!(client.user_boot(), client.users_list());
    let mut ws = Workspace::default();
    for u in users? {
        ws.users.insert(u.id.clone(), u);
    }
    ws.apply_boot(&boot?);
    let mut rtm = slack_core::rtm::spawn(client.clone());
    while let Some(ev) = rtm.events.recv().await {
        let slack_core::rtm::RtmEvent::Event(v) = ev else { continue };
        let fx = super::live::apply(&mut ws, &v, None);
        if shown.load(Ordering::Relaxed) == index {
            continue;
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64();
        if ws.dnd.stale(now) {
            match client.call_raw("dnd.info", &[]).await {
                Ok(d) => ws.dnd = Dnd::from_json(&d),
                Err(e) => tracing::debug!("dnd.info of workspace {}: {e}", index + 1),
            }
        }
        let note = match (&fx.incoming, &fx.huddle_started) {
            (Some((channel, m)), _) => super::notify::notification(&ws, channel, m, false, now).map(|n| (n, super::notify::Target::message(channel, m))),
            (None, Some((channel, user))) => super::notify::huddle_notification(&ws, channel, user, false, now, &format!("Alt+{}, then F3,", index + 1))
                .map(|n| (n, super::notify::Target { channel: channel.clone(), ts: None, thread_ts: None })),
            _ => None,
        };
        let Some(((title, body), target)) = note else { continue };
        let (settings, _) = super::settings::load();
        tracing::debug!(workspace = index + 1, channel = target.channel, "notification from another workspace");
        if settings.notifications.desktop {
            let clicks = clicks.clone();
            super::notify::show(format!("{title} · {label}"), body, &settings, Some(Box::new(move || {
                let _ = clicks.send((index, target));
            })));
        }
    }
    Ok(())
}

/// The status bar's note on the other workspaces: " · Alt+2 @3" for something needing
/// attention, " · Alt+2 •" for other unread messages; empty when all is read.
pub fn label(badges: &HashMap<usize, Badge>, shown: usize) -> String {
    let mut keys: Vec<&usize> = badges.keys().filter(|i| **i != shown).collect();
    keys.sort();
    keys.into_iter()
        .filter_map(|i| {
            let b = badges[i];
            match (b.attention, b.unread) {
                (0, false) => None,
                (0, true) => Some(format!(" · Alt+{} •", i + 1)),
                (n, _) => Some(format!(" · Alt+{} @{n}", i + 1)),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut all = HashMap::new();
        all.insert(1, Badge { attention: 3, unread: true });
        all.insert(2, Badge { attention: 0, unread: true });
        all.insert(3, Badge::default());
        all.insert(0, Badge { attention: 9, unread: true });
        assert_eq!(label(&all, 0), " · Alt+2 @3 · Alt+3 •", "the shown workspace and quiet ones are left out");
    }
}

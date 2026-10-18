//! Desktop notifications for incoming messages.

use slack_core::format;
use slack_core::types::Message;

use super::live::mentions_me;
use super::state::Workspace;

const BODY_MAX: usize = 300;

/// Title and body for a notification about `m` in `channel`, or None if it should stay
/// quiet. `viewing`: the user is looking at this conversation (or its thread) right now.
pub fn notification(ws: &Workspace, channel: &str, m: &Message, viewing: bool, now: f64) -> Option<(String, String)> {
    let from_me = m.user.as_deref() == Some(ws.self_id.as_str());
    let system = m.subtype.as_deref().is_some_and(|s| !matches!(s, "thread_broadcast" | "file_share" | "bot_message" | "me_message"));
    if from_me || system || m.hidden || viewing || ws.muted.contains(channel) || ws.dnd.active(now) {
        return None;
    }
    let conv = ws.conversations.get(channel);
    let is_im = conv.is_some_and(|c| c.is_im);
    let in_my_thread = m.is_reply()
        && m.thread_ts.as_deref().and_then(|t| ws.find_message(channel, t)).is_some_and(|p| p.user.as_deref() == Some(ws.self_id.as_str()) || p.reply_users.contains(&ws.self_id));
    let mentioned = mentions_me(ws, m) || ws.notify.keyword_in(&m.text).is_some();
    let wanted = match ws.notify.level(channel, conv.is_some_and(|c| c.is_mpim)) {
        "nothing" => false,
        // Every top-level message; replies only where we take part.
        "everything" => is_im || mentioned || in_my_thread || !m.is_reply(),
        _ => is_im || mentioned || in_my_thread || conv.is_some_and(|c| c.is_mpim),
    };
    if !wanted {
        return None;
    }
    let author = ws.author(m);
    let mut title = match conv {
        Some(c) if c.is_im => author,
        Some(c) if c.is_mpim => format!("{author} in {}", ws.conversation_label(c)),
        _ => format!("{author} in #{}", ws.channel_label(channel)),
    };
    if m.is_reply() {
        title.push_str(" (thread)");
    }
    let mut body = format::plain_text(&format::message_blocks(&m.text, &m.blocks), &|i| super::text::inline_plain(ws, i));
    if body.trim().is_empty() {
        if let Some(f) = m.files.first() {
            body = format!("sent a file: {}", if f.title.is_empty() { &f.name } else { &f.title });
        }
    }
    if body.chars().count() > BODY_MAX {
        body = body.chars().take(BODY_MAX).collect::<String>() + "…";
    }
    Some((title, body))
}

/// Notification for someone starting a huddle in a DM or group DM with us (Slack rings
/// there); None for channels, muted conversations, DND, or a huddle we are already in.
pub fn huddle_notification(ws: &Workspace, channel: &str, user: &str, in_it: bool, now: f64, join_key: &str) -> Option<(String, String)> {
    let conv = ws.conversations.get(channel)?;
    if in_it || user == ws.self_id || !conv.is_dm() || ws.muted.contains(channel) || ws.dnd.active(now) {
        return None;
    }
    let who = ws.user_name(user);
    let title = if conv.is_mpim { format!("{who} started a huddle in {}", ws.conversation_label(conv)) } else { format!("{who} started a huddle") };
    Some((title, format!("Press {join_key} in slacktui to join")))
}

/// Show a desktop notification without blocking the UI, with the sound from the settings.
pub fn show(title: String, body: String, settings: &super::settings::Settings) {
    let sound = settings.notifications.sound.clone();
    super::sound::play(&sound, super::settings::Settings::device(huddle::devices::OUTPUT_ENV, &settings.audio.output));
    tokio::task::spawn_blocking(move || {
        let mut n = notify_rust::Notification::new();
        n.appname("slacktui").summary(&title).body(&body);
        if sound == "system" {
            n.sound_name(super::sound::system_sound_name());
        }
        let result = n.show();
        if let Err(e) = result {
            tracing::warn!("notification failed: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use slack_core::types::{Conversation, User};

    fn ws() -> Workspace {
        let mut w = Workspace { self_id: "UME".into(), ..Default::default() };
        let mut u = User { id: "U2".into(), name: "bob".into(), ..Default::default() };
        u.profile.display_name = "Bob".into();
        w.users.insert("U2".into(), u);
        w.conversations.insert("D1".into(), Conversation { id: "D1".into(), is_im: true, user: Some("U2".into()), ..Default::default() });
        w.conversations.insert("C1".into(), Conversation { id: "C1".into(), name: "general".into(), is_channel: true, is_member: true, ..Default::default() });
        w
    }

    fn msg(user: &str, text: &str) -> Message {
        Message { ts: "10.0".into(), user: Some(user.into()), text: text.into(), ..Default::default() }
    }

    #[test]
    fn dms_and_mentions_notify() {
        let w = ws();
        assert_eq!(notification(&w, "D1", &msg("U2", "hi"), false, 0.0), Some(("Bob".into(), "hi".into())));
        assert_eq!(notification(&w, "C1", &msg("U2", "hey <@UME>"), false, 0.0).map(|n| n.0), Some("Bob in #general".into()));
        assert_eq!(notification(&w, "C1", &msg("U2", "<!here> lunch"), false, 0.0).map(|n| n.1), Some("@here lunch".into()));
        assert!(notification(&w, "C1", &msg("U2", "plain chatter"), false, 0.0).is_none());
    }

    #[test]
    fn quiet_cases() {
        let mut w = ws();
        assert!(notification(&w, "D1", &msg("UME", "mine"), false, 0.0).is_none());
        assert!(notification(&w, "D1", &msg("U2", "seen"), true, 0.0).is_none());
        let mut join = msg("U2", "joined");
        join.subtype = Some("channel_join".into());
        assert!(notification(&w, "D1", &join, false, 0.0).is_none());
        w.dnd.snooze_until = Some(100.0);
        assert!(notification(&w, "D1", &msg("U2", "dnd"), false, 50.0).is_none());
        assert!(notification(&w, "D1", &msg("U2", "after dnd"), false, 150.0).is_some());
        w.muted.insert("C1".into());
        assert!(notification(&w, "C1", &msg("U2", "<@UME>"), false, 150.0).is_none());
    }

    #[test]
    fn levels_and_keywords() {
        let mut w = ws();
        w.notify.keywords = vec!["deploy".into()];
        assert!(notification(&w, "C1", &msg("U2", "we deploy at 5"), false, 0.0).is_some(), "a keyword counts as a mention");
        w.notify.channels.insert("C1".into(), "everything".into());
        assert!(notification(&w, "C1", &msg("U2", "plain chatter"), false, 0.0).is_some());
        let mut reply = msg("U2", "a reply elsewhere");
        reply.thread_ts = Some("1.0".into());
        assert!(notification(&w, "C1", &reply, false, 0.0).is_none(), "everything: not every reply");
        w.notify.channels.insert("C1".into(), "nothing".into());
        assert!(notification(&w, "C1", &msg("U2", "hey <@UME>"), false, 0.0).is_none());
        w.notify.global = "nothing".into();
        assert!(notification(&w, "D1", &msg("U2", "hi"), false, 0.0).is_none(), "nothing silences DMs too");
    }

    #[test]
    fn huddle_starts_in_dms_only() {
        let w = ws();
        assert_eq!(huddle_notification(&w, "D1", "U2", false, 0.0, "F3").map(|n| n.0), Some("Bob started a huddle".into()));
        assert!(huddle_notification(&w, "C1", "U2", false, 0.0, "F3").is_none());
        assert!(huddle_notification(&w, "D1", "U2", true, 0.0, "F3").is_none());
        assert!(huddle_notification(&w, "D1", "UME", false, 0.0, "F3").is_none());
    }

    #[test]
    fn replies_in_my_threads() {
        let mut w = ws();
        w.timelines.entry("C1".into()).or_default().upsert(Message { ts: "5.0".into(), user: Some("UME".into()), ..Default::default() });
        let mut reply = msg("U2", "answer");
        reply.thread_ts = Some("5.0".into());
        assert_eq!(notification(&w, "C1", &reply, false, 0.0).map(|n| n.0), Some("Bob in #general (thread)".into()));
        reply.thread_ts = Some("6.0".into());
        assert!(notification(&w, "C1", &reply, false, 0.0).is_none());
    }
}

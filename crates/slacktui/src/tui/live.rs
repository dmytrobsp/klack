//! Apply real-time (RTM) events to the workspace state.

use serde_json::Value;
use slack_core::types::{Conversation, Message, Reaction, User};

use super::state::{Workspace, thread_key};

/// What the UI should do after an event.
#[derive(Debug, Default)]
pub struct Effects {
    pub rebuild_sidebar: bool,
    /// A new message from someone else arrived in this channel.
    pub incoming: Option<(String, Message)>,
    /// Someone else started a huddle: (channel, user).
    pub huddle_started: Option<(String, String)>,
}

fn parse_message(v: &Value, channel: &str) -> Option<Message> {
    let mut m: Message = serde_json::from_value(v.clone()).ok()?;
    if m.channel.is_none() {
        m.channel = Some(channel.to_string());
    }
    Some(m)
}

/// Does this message mention us (directly or via @here/@channel)?
pub fn mentions_me(ws: &Workspace, m: &Message) -> bool {
    let me = format!("<@{}", ws.self_id);
    m.text.contains(&me) || m.text.contains("<!here") || m.text.contains("<!channel") || m.text.contains("<!everyone")
}

pub fn apply(ws: &mut Workspace, ev: &Value, current: Option<&str>) -> Effects {
    let mut fx = Effects::default();
    let channel = ev["channel"].as_str().unwrap_or_default().to_string();
    match ev["type"].as_str().unwrap_or("") {
        "message" => match ev["subtype"].as_str() {
            Some("message_changed") => {
                if let Some(mut m) = parse_message(&ev["message"], &channel) {
                    // Keep what the event leaves out rather than wiping it.
                    if let Some(old) = ws.find_message(&channel, &m.ts) {
                        let missing = |key: &str| ev["message"].get(key).is_none();
                        if missing("reactions") {
                            m.reactions = old.reactions.clone();
                        }
                        if missing("reply_count") {
                            m.reply_count = old.reply_count;
                            m.reply_users = old.reply_users.clone();
                            m.latest_reply = old.latest_reply.clone();
                        }
                        if missing("files") {
                            m.files = old.files.clone();
                        }
                    }
                    update_everywhere(ws, &channel, m);
                }
            }
            Some("message_deleted") => {
                if let Some(ts) = ev["deleted_ts"].as_str() {
                    if let Some(t) = ws.timelines.get_mut(&channel) {
                        t.remove(ts);
                    }
                    for (key, t) in ws.threads.iter_mut() {
                        if key.starts_with(&format!("{channel}/")) {
                            t.remove(ts);
                        }
                    }
                }
            }
            Some("message_replied") => {
                // The parent arrives without reactions or files: take only the thread fields.
                let m = &ev["message"];
                let ts = m["ts"].as_str().unwrap_or_default();
                for p in ws.copies_mut(&channel, ts) {
                    p.reply_count = m["reply_count"].as_u64().unwrap_or(p.reply_count);
                    if let Some(latest) = m["latest_reply"].as_str() {
                        p.latest_reply = Some(latest.to_string());
                    }
                    if let Some(users) = m["reply_users"].as_array() {
                        p.reply_users = users.iter().filter_map(|u| u.as_str().map(String::from)).collect();
                    }
                    p.thread_ts = Some(ts.to_string());
                }
            }
            _ => {
                let Some(m) = parse_message(ev, &channel) else { return fx };
                let from_me = m.user.as_deref() == Some(ws.self_id.as_str());
                add_message(ws, &channel, &m);
                if !from_me {
                    // Someone stopped typing once their message lands.
                    if let (Some(list), Some(u)) = (ws.typing.get_mut(&channel), m.user.as_deref()) {
                        list.retain(|(x, _)| x != u);
                    }
                    if current != Some(channel.as_str()) && !m.is_reply() {
                        let is_dm = ws.conversations.get(&channel).is_some_and(|c| c.is_dm());
                        let mention = is_dm || mentions_me(ws, &m);
                        let u = ws.unread.entry(channel.clone()).or_default();
                        u.has_unreads = true;
                        if mention {
                            u.mentions += 1;
                        }
                    }
                    fx.incoming = Some((channel, m));
                }
            }
        },
        "reaction_added" | "reaction_removed" => {
            let added = ev["type"] == "reaction_added";
            let item_channel = ev["item"]["channel"].as_str().unwrap_or_default();
            let ts = ev["item"]["ts"].as_str().unwrap_or_default();
            let name = ev["reaction"].as_str().unwrap_or_default();
            let user = ev["user"].as_str().unwrap_or_default();
            let mut targets: Vec<&mut Message> = Vec::new();
            if let Some(t) = ws.timelines.get_mut(item_channel) {
                targets.extend(t.messages.iter_mut().filter(|m| m.ts == ts));
            }
            for (key, t) in ws.threads.iter_mut() {
                if key.starts_with(&format!("{item_channel}/")) {
                    targets.extend(t.messages.iter_mut().filter(|m| m.ts == ts));
                }
            }
            for m in targets {
                apply_reaction(m, name, user, added);
            }
        }
        "user_typing" => {
            if let Some(u) = ev["user"].as_str() {
                let list = ws.typing.entry(channel).or_default();
                list.retain(|(x, _)| x != u);
                list.push((u.to_string(), std::time::Instant::now()));
            }
        }
        "channel_marked" | "im_marked" | "group_marked" | "mpim_marked" => {
            let u = ws.unread.entry(channel.clone()).or_default();
            u.has_unreads = ev["unread_count_display"].as_u64().unwrap_or(0) > 0;
            u.mentions = ev["num_mentions_display"].as_u64().unwrap_or(0);
            if let Some(c) = ws.conversations.get_mut(&channel) {
                c.last_read = ev["ts"].as_str().map(String::from);
            }
        }
        "sh_room_join" | "sh_room_leave" | "sh_room_update" => {
            let room = &ev["room"];
            let id = room["id"].as_str().unwrap_or_default().to_string();
            let participants: Vec<String> = room["participants"].as_array().into_iter().flatten().filter_map(|u| u.as_str().map(String::from)).collect();
            let ended = room["has_ended"].as_bool().unwrap_or(false) || participants.is_empty();
            let user = ev["user"].as_str().unwrap_or_default();
            for c in room["channels"].as_array().into_iter().flatten().filter_map(|c| c.as_str()) {
                if ev["type"] == "sh_room_join" && !ended && user != ws.self_id && !ws.huddles.contains_key(c) {
                    fx.huddle_started = Some((c.to_string(), user.to_string()));
                }
                if ended {
                    ws.huddles.remove(c);
                } else {
                    ws.huddles.insert(c.to_string(), (id.clone(), participants.clone()));
                }
            }
            fx.rebuild_sidebar = true;
        }
        "pin_added" | "pin_removed" => {
            let added = ev["type"] == "pin_added";
            let item = &ev["item"];
            let channel = item["channel"].as_str().or(ev["channel_id"].as_str()).unwrap_or_default().to_string();
            let ts = item["message"]["ts"].as_str().unwrap_or_default().to_string();
            for m in ws.copies_mut(&channel, &ts) {
                m.pinned_to.retain(|c| *c != channel);
                if added {
                    m.pinned_to.push(channel.clone());
                }
            }
        }
        "dnd_updated" => ws.dnd = super::state::Dnd::from_json(&ev["dnd_status"]),
        "presence_change" => {
            let active = ev["presence"] == "active";
            let users: Vec<String> = match ev["users"].as_array() {
                Some(list) => list.iter().filter_map(|u| u.as_str().map(String::from)).collect(),
                None => ev["user"].as_str().map(|u| vec![u.to_string()]).unwrap_or_default(),
            };
            for u in users {
                ws.presence.insert(u, active);
            }
        }
        "channel_joined" | "group_joined" | "im_created" | "channel_created" | "mpim_joined" => {
            if let Ok(mut c) = serde_json::from_value::<Conversation>(ev["channel"].clone()) {
                if ev["type"] == "im_created" {
                    c.is_im = true;
                }
                if ev["type"] == "channel_created" {
                    // Only news if we did not know it: our own create may have been applied already.
                    ws.conversations.entry(c.id.clone()).or_insert(c);
                } else {
                    c.is_member = true;
                    ws.conversations.insert(c.id.clone(), c);
                }
                fx.rebuild_sidebar = true;
            }
        }
        "channel_left" | "group_left" | "channel_archive" | "channel_deleted" => {
            if let Some(c) = ws.conversations.get_mut(&channel) {
                c.is_member = false;
                fx.rebuild_sidebar = true;
            }
        }
        "channel_rename" | "group_rename" => {
            let id = ev["channel"]["id"].as_str().unwrap_or_default();
            if let (Some(c), Some(name)) = (ws.conversations.get_mut(id), ev["channel"]["name"].as_str()) {
                c.name = name.to_string();
                fx.rebuild_sidebar = true;
            }
        }
        "user_change" | "team_join" => {
            if let Ok(u) = serde_json::from_value::<User>(ev["user"].clone()) {
                ws.users.insert(u.id.clone(), u);
            }
        }
        _ => {}
    }
    fx
}

/// Put a new message where it belongs: a reply into its loaded thread (bumping the parent's
/// count once), a top-level message or thread broadcast into the loaded timeline.
pub fn add_message(ws: &mut Workspace, channel: &str, m: &Message) {
    if let Some(parent) = m.thread_ts.clone().filter(|_| m.is_reply()) {
        if let Some(t) = ws.threads.get_mut(&thread_key(channel, &parent)) {
            t.upsert(m.clone());
        }
        // `message_replied` may already have brought the server's count for this reply.
        let counted = |p: &Message| p.latest_reply.as_deref().and_then(|l| l.parse::<f64>().ok()).is_some_and(|l| l >= m.time());
        if let Some(p) = ws.timelines.get_mut(channel).and_then(|t| t.messages.iter_mut().find(|x| x.ts == parent)).filter(|p| !counted(p)) {
            p.reply_count += 1;
            p.latest_reply = Some(m.ts.clone());
            if let Some(u) = &m.user {
                if !p.reply_users.contains(u) {
                    p.reply_users.push(u.clone());
                }
            }
        }
        if m.subtype.as_deref() == Some("thread_broadcast") {
            if let Some(t) = ws.timelines.get_mut(channel).filter(|t| t.loaded) {
                t.upsert(m.clone());
            }
        }
    } else if let Some(t) = ws.timelines.get_mut(channel).filter(|t| t.loaded) {
        t.upsert(m.clone());
    }
}

fn update_everywhere(ws: &mut Workspace, channel: &str, m: Message) {
    if let Some(t) = ws.timelines.get_mut(channel) {
        if t.index_of(&m.ts).is_some() {
            t.upsert(m.clone());
        }
    }
    let thread = m.thread_ts.clone().unwrap_or_else(|| m.ts.clone());
    if let Some(t) = ws.threads.get_mut(&thread_key(channel, &thread)) {
        if t.index_of(&m.ts).is_some() {
            t.upsert(m);
        }
    }
}

fn apply_reaction(m: &mut Message, name: &str, user: &str, added: bool) {
    match m.reactions.iter_mut().position(|r| r.name == name) {
        Some(i) => {
            let r = &mut m.reactions[i];
            if added && !r.users.iter().any(|u| u == user) {
                r.users.push(user.to_string());
                r.count += 1;
            } else if !added && r.users.iter().any(|u| u == user) {
                r.users.retain(|u| u != user);
                r.count = r.count.saturating_sub(1);
                if r.count == 0 {
                    m.reactions.remove(i);
                }
            }
        }
        None if added => m.reactions.push(Reaction { name: name.to_string(), users: vec![user.to_string()], count: 1 }),
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::state::Timeline;
    use serde_json::json;

    fn ws() -> Workspace {
        let mut ws = Workspace { self_id: "UME".into(), ..Default::default() };
        ws.conversations.insert("C1".into(), Conversation { id: "C1".into(), name: "general".into(), is_member: true, ..Default::default() });
        ws.timelines.insert("C1".into(), Timeline { loaded: true, ..Default::default() });
        ws
    }

    #[test]
    fn new_message_lands_and_counts_unread() {
        let mut w = ws();
        let fx = apply(&mut w, &json!({"type":"message","channel":"C1","user":"U2","text":"hi <@UME>","ts":"10.0"}), None);
        assert_eq!(w.timelines["C1"].messages.len(), 1);
        assert!(fx.incoming.is_some());
        assert!(w.unread["C1"].has_unreads);
        assert_eq!(w.unread["C1"].mentions, 1);
        // Viewing the channel: no unread bump.
        apply(&mut w, &json!({"type":"message","channel":"C1","user":"U2","text":"again","ts":"11.0"}), Some("C1"));
        assert_eq!(w.unread["C1"].mentions, 1);
    }

    #[test]
    fn edits_deletes_and_reactions() {
        let mut w = ws();
        apply(&mut w, &json!({"type":"message","channel":"C1","user":"U2","text":"orig","ts":"10.0"}), Some("C1"));
        apply(&mut w, &json!({"type":"message","subtype":"message_changed","channel":"C1","message":{"type":"message","user":"U2","text":"edited","ts":"10.0","edited":{"user":"U2","ts":"12.0"}}}), Some("C1"));
        assert_eq!(w.timelines["C1"].messages[0].text, "edited");
        apply(&mut w, &json!({"type":"reaction_added","user":"UME","reaction":"tada","item":{"channel":"C1","ts":"10.0"}}), Some("C1"));
        apply(&mut w, &json!({"type":"reaction_added","user":"U2","reaction":"tada","item":{"channel":"C1","ts":"10.0"}}), Some("C1"));
        assert_eq!(w.timelines["C1"].messages[0].reactions[0].count, 2);
        apply(&mut w, &json!({"type":"reaction_removed","user":"UME","reaction":"tada","item":{"channel":"C1","ts":"10.0"}}), Some("C1"));
        assert_eq!(w.timelines["C1"].messages[0].reactions[0].count, 1);
        apply(&mut w, &json!({"type":"message","subtype":"message_deleted","channel":"C1","deleted_ts":"10.0"}), Some("C1"));
        assert!(w.timelines["C1"].messages.is_empty());
    }

    #[test]
    fn replies_bump_parent_not_timeline() {
        let mut w = ws();
        apply(&mut w, &json!({"type":"message","channel":"C1","user":"U2","text":"parent","ts":"10.0"}), Some("C1"));
        apply(&mut w, &json!({"type":"message","channel":"C1","user":"U3","text":"reply","ts":"11.0","thread_ts":"10.0"}), Some("C1"));
        let t = &w.timelines["C1"];
        assert_eq!(t.messages.len(), 1);
        assert_eq!(t.messages[0].reply_count, 1);
        assert_eq!(t.messages[0].latest_reply.as_deref(), Some("11.0"));
    }

    #[test]
    fn channel_membership_events() {
        let mut w = ws();
        w.conversations.insert("C9".into(), Conversation { id: "C9".into(), name: "mine".into(), is_member: true, ..Default::default() });
        apply(&mut w, &json!({"type":"channel_created","channel":{"id":"C9","name":"mine","creator":"UME"}}), None);
        assert!(w.conversations["C9"].is_member, "our own create, already applied, is not undone");
        apply(&mut w, &json!({"type":"channel_created","channel":{"id":"C8","name":"theirs"}}), None);
        assert!(!w.conversations["C8"].is_member);
        apply(&mut w, &json!({"type":"channel_joined","channel":{"id":"C8","name":"theirs"}}), None);
        assert!(w.conversations["C8"].is_member);
        let fx = apply(&mut w, &json!({"type":"channel_left","channel":"C8"}), None);
        assert!(fx.rebuild_sidebar && !w.conversations["C8"].is_member);
    }

    #[test]
    fn huddle_events_track_active_huddles() {
        let mut w = ws();
        apply(&mut w, &json!({"type":"sh_room_join","user":"U2","room":{"id":"R1","channels":["C1"],"participants":["U2"],"has_ended":false}}), None);
        assert_eq!(w.huddles["C1"], ("R1".to_string(), vec!["U2".to_string()]));
        let fx = apply(&mut w, &json!({"type":"sh_room_join","user":"U3","room":{"id":"R1","channels":["C1"],"participants":["U2","U3"],"has_ended":false}}), None);
        assert!(fx.huddle_started.is_none(), "joining a running huddle is not a start");
        apply(&mut w, &json!({"type":"sh_room_leave","user":"U2","room":{"id":"R1","channels":["C1"],"participants":[],"has_ended":false}}), None);
        assert!(w.huddles.is_empty());
    }

    #[test]
    fn pin_events_mark_messages() {
        let mut w = ws();
        apply(&mut w, &json!({"type":"message","channel":"C1","user":"U2","text":"pin me","ts":"10.0"}), Some("C1"));
        apply(&mut w, &json!({"type":"pin_added","user":"U2","channel_id":"C1","item":{"type":"message","channel":"C1","message":{"ts":"10.0"}}}), Some("C1"));
        assert_eq!(w.timelines["C1"].messages[0].pinned_to, vec!["C1".to_string()]);
        apply(&mut w, &json!({"type":"pin_removed","user":"U2","channel_id":"C1","item":{"type":"message","channel":"C1","message":{"ts":"10.0"}}}), Some("C1"));
        assert!(w.timelines["C1"].messages[0].pinned_to.is_empty());
    }

    #[test]
    fn message_replied_keeps_reactions() {
        let mut w = ws();
        apply(&mut w, &json!({"type":"message","channel":"C1","user":"U2","text":"parent","ts":"10.0"}), Some("C1"));
        apply(&mut w, &json!({"type":"reaction_added","user":"U3","reaction":"+1","item":{"type":"message","channel":"C1","ts":"10.0"}}), Some("C1"));
        apply(&mut w, &json!({"type":"message","subtype":"message_replied","channel":"C1","message":{"type":"message","user":"U2","text":"parent","ts":"10.0","thread_ts":"10.0","reply_count":2,"latest_reply":"12.0","reply_users":["U3"]}}), Some("C1"));
        apply(&mut w, &json!({"type":"message","subtype":"message_changed","channel":"C1","message":{"type":"message","user":"U2","text":"parent v2","ts":"10.0","edited":{"user":"U2","ts":"13.0"}}}), Some("C1"));
        let p = &w.timelines["C1"].messages[0];
        assert_eq!(p.reactions.len(), 1);
        assert_eq!(p.text, "parent v2");
        assert_eq!(p.reply_count, 2);
    }

    #[test]
    fn reply_counted_once_with_message_replied() {
        let parent = json!({"type":"message","channel":"C1","user":"U2","text":"parent","ts":"10.0"});
        let reply = json!({"type":"message","channel":"C1","user":"U3","text":"reply","ts":"11.0","thread_ts":"10.0"});
        let replied = json!({"type":"message","subtype":"message_replied","channel":"C1","message":{"type":"message","user":"U2","text":"parent","ts":"10.0","thread_ts":"10.0","reply_count":1,"latest_reply":"11.0"}});
        for order in [[&reply, &replied], [&replied, &reply]] {
            let mut w = ws();
            apply(&mut w, &parent, Some("C1"));
            for ev in order {
                apply(&mut w, ev, Some("C1"));
            }
            assert_eq!(w.timelines["C1"].messages[0].reply_count, 1);
        }
    }

    #[test]
    fn markers_and_typing() {
        let mut w = ws();
        apply(&mut w, &json!({"type":"message","channel":"C1","user":"U2","text":"x","ts":"10.0"}), None);
        apply(&mut w, &json!({"type":"channel_marked","channel":"C1","ts":"10.0","unread_count_display":0,"num_mentions_display":0}), None);
        assert!(!w.unread["C1"].has_unreads);
        apply(&mut w, &json!({"type":"user_typing","channel":"C1","user":"U2"}), None);
        assert_eq!(w.typing_in("C1"), vec!["U2".to_string()]);
    }
}

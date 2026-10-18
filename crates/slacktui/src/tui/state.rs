//! Workspace data held by the UI: conversations, users, message timelines, unread state.

use std::collections::{BTreeMap, HashMap};

use serde_json::Value;
use slack_core::types::{Conversation, Message, User};

#[derive(Debug, Clone, Default)]
pub struct Unread {
    pub has_unreads: bool,
    pub mentions: u64,
}

/// One conversation's loaded messages, oldest first.
#[derive(Debug, Clone, Default)]
pub struct Timeline {
    pub messages: Vec<Message>,
    pub loaded: bool,
    pub loading: bool,
    pub has_more: bool,
}

impl Timeline {
    pub fn index_of(&self, ts: &str) -> Option<usize> {
        self.messages.iter().position(|m| m.ts == ts)
    }

    /// Insert keeping ts order; replaces an existing message with the same ts.
    pub fn upsert(&mut self, msg: Message) {
        match self.messages.binary_search_by(|m| m.time().partial_cmp(&msg.time()).unwrap_or(std::cmp::Ordering::Equal)) {
            Ok(i) if self.messages[i].ts == msg.ts => self.messages[i] = msg,
            Ok(i) | Err(i) => {
                if let Some(j) = self.index_of(&msg.ts) {
                    self.messages[j] = msg;
                } else {
                    self.messages.insert(i, msg);
                }
            }
        }
    }

    pub fn remove(&mut self, ts: &str) {
        self.messages.retain(|m| m.ts != ts);
    }

    /// Prepend an older page (Slack returns newest first).
    pub fn prepend_page(&mut self, mut page: Vec<Message>, has_more: bool) {
        page.reverse();
        page.retain(|m| self.index_of(&m.ts).is_none());
        page.extend(std::mem::take(&mut self.messages));
        self.messages = page;
        self.has_more = has_more;
        self.loaded = true;
        self.loading = false;
    }
}

#[derive(Debug, Default)]
pub struct Workspace {
    pub team_name: String,
    pub self_id: String,
    pub users: HashMap<String, User>,
    pub conversations: BTreeMap<String, Conversation>,
    pub unread: HashMap<String, Unread>,
    pub timelines: HashMap<String, Timeline>,
    /// thread key `channel/ts` → replies (including parent first)
    pub threads: HashMap<String, Timeline>,
    pub presence: HashMap<String, bool>,
    /// channel → (user, when) of typing notifications
    pub typing: HashMap<String, Vec<(String, std::time::Instant)>>,
    pub custom_emoji: HashMap<String, String>,
    pub muted: std::collections::HashSet<String>,
    pub dnd: Dnd,
    /// Active huddles: channel → (room id, participant user ids).
    pub huddles: HashMap<String, (String, Vec<String>)>,
    /// A channel we are not in, shown in the sidebar while it is being looked at.
    pub preview: Option<String>,
}

/// Do Not Disturb, as `dnd.info` / `dnd_updated` report it (times in epoch seconds).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Dnd {
    /// Scheduled quiet hours as Slack reports them: the next start and the next end. They
    /// are not one window (after today's window, start is today's and end is tomorrow's),
    /// so they are read as a daily schedule.
    pub window: Option<(f64, f64)>,
    /// Manual snooze until this time.
    pub snooze_until: Option<f64>,
}

impl Dnd {
    pub fn from_json(v: &Value) -> Dnd {
        let num = |k: &str| v[k].as_f64().filter(|t| *t > 0.0);
        let window = match (v["dnd_enabled"].as_bool().unwrap_or(false), num("next_dnd_start_ts"), num("next_dnd_end_ts")) {
            (true, Some(start), Some(end)) => Some((start, end)),
            _ => None,
        };
        let snooze_until = num("snooze_endtime").filter(|_| v["snooze_enabled"].as_bool().unwrap_or(false));
        Dnd { window, snooze_until }
    }

    pub fn active(&self, now: f64) -> bool {
        const DAY: f64 = 86_400.0;
        let scheduled = self.window.is_some_and(|(start, end)| {
            let len = (end - start).rem_euclid(DAY);
            (now - start).rem_euclid(DAY) < len
        });
        self.snooze_until.is_some_and(|t| now < t) || scheduled
    }
}

pub fn thread_key(channel: &str, ts: &str) -> String {
    format!("{channel}/{ts}")
}

impl Workspace {
    /// Message `ts` of `channel`, from its timeline or a loaded thread.
    pub fn find_message(&self, channel: &str, ts: &str) -> Option<&Message> {
        let prefix = format!("{channel}/");
        let threads = self.threads.iter().filter(|(k, _)| k.starts_with(&prefix)).map(|(_, t)| t);
        self.timelines.get(channel).into_iter().chain(threads).flat_map(|t| t.messages.iter()).find(|m| m.ts == ts)
    }

    /// Every loaded copy of message `ts` of `channel` (a thread parent is in both its
    /// timeline and its thread).
    pub fn copies_mut(&mut self, channel: &str, ts: &str) -> Vec<&mut Message> {
        let prefix = format!("{channel}/");
        let mut out: Vec<&mut Message> = self.timelines.get_mut(channel).into_iter().flat_map(|t| t.messages.iter_mut()).filter(|m| m.ts == ts).collect();
        for (key, t) in self.threads.iter_mut() {
            if key.starts_with(&prefix) {
                out.extend(t.messages.iter_mut().filter(|m| m.ts == ts));
            }
        }
        out
    }

    /// Remove message `ts` of `channel` from its timeline and every loaded thread.
    pub fn remove_message(&mut self, channel: &str, ts: &str) {
        if let Some(t) = self.timelines.get_mut(channel) {
            t.remove(ts);
        }
        let prefix = format!("{channel}/");
        for (key, t) in self.threads.iter_mut() {
            if key.starts_with(&prefix) {
                t.remove(ts);
            }
        }
    }

    pub fn user_name(&self, id: &str) -> String {
        match self.users.get(id) {
            Some(u) => u.display().to_string(),
            None if id.starts_with('B') => "bot".into(),
            None => id.to_string(),
        }
    }

    /// Sidebar label for a conversation.
    pub fn conversation_label(&self, c: &Conversation) -> String {
        if c.is_im {
            c.user.as_deref().map(|u| self.user_name(u)).unwrap_or_else(|| c.id.clone())
        } else if c.is_mpim {
            // mpdm-alice--bob-1 → alice, bob
            let core = c.name.trim_start_matches("mpdm-").trim_end_matches("-1");
            core.split("--").collect::<Vec<_>>().join(", ")
        } else {
            c.name.clone()
        }
    }

    pub fn channel_label(&self, id: &str) -> String {
        self.conversations.get(id).map(|c| self.conversation_label(c)).unwrap_or_else(|| id.to_string())
    }

    /// Author name for a message (users, bots, integrations).
    pub fn author(&self, m: &Message) -> String {
        if let Some(u) = &m.user {
            return self.user_name(u);
        }
        if let Some(name) = m.bot_profile.as_ref().and_then(|b| b["name"].as_str()) {
            return name.to_string();
        }
        m.username.clone().unwrap_or_else(|| "bot".into())
    }

    /// Sidebar order: channels (alphabetical), then DMs (open ones), Slackbot last.
    pub fn sidebar(&self) -> (Vec<String>, Vec<String>) {
        let shown = |c: &Conversation| c.is_member || self.preview.as_deref() == Some(c.id.as_str());
        let mut channels: Vec<&Conversation> = self.conversations.values().filter(|c| !c.is_dm() && shown(c) && !c.is_archived).collect();
        channels.sort_by_key(|c| c.name.to_lowercase());
        let mut dms: Vec<&Conversation> = self
            .conversations
            .values()
            .filter(|c| c.is_dm() && !c.is_archived)
            .filter(|c| c.user.as_deref().is_none_or(|u| self.users.get(u).is_none_or(|u| !u.deleted)))
            .collect();
        dms.sort_by_key(|c| (c.user.as_deref() == Some("USLACKBOT"), self.conversation_label(c).to_lowercase()));
        (channels.into_iter().map(|c| c.id.clone()).collect(), dms.into_iter().map(|c| c.id.clone()).collect())
    }

    /// Apply `client.userBoot` (channels, IMs, DND).
    pub fn apply_boot(&mut self, boot: &Value) {
        for key in ["channels", "ims"] {
            for c in boot[key].as_array().into_iter().flatten() {
                if let Ok(mut conv) = serde_json::from_value::<Conversation>(c.clone()) {
                    if key == "ims" {
                        conv.is_im = true;
                    }
                    if key == "channels" && c.get("is_member").is_none() {
                        conv.is_member = true;
                    }
                    self.conversations.insert(conv.id.clone(), conv);
                }
            }
        }
        self.dnd = Dnd::from_json(&boot["dnd"]);
        if let Some(prefs) = boot["prefs"]["muted_channels"].as_str() {
            self.muted = prefs.split(',').filter(|s| !s.is_empty()).map(String::from).collect();
        }
    }

    /// Add channels from `conversations.list` that boot did not report (public channels we
    /// are not in); known ones keep their boot data.
    pub fn add_channels(&mut self, list: Vec<Conversation>) {
        for c in list.into_iter().filter(|c| !c.is_dm()) {
            match self.conversations.get_mut(&c.id) {
                Some(known) => known.num_members = known.num_members.or(c.num_members),
                None => {
                    self.conversations.insert(c.id.clone(), c);
                }
            }
        }
    }

    /// A public channel we are not a member of.
    pub fn is_outside(&self, id: &str) -> bool {
        self.conversations.get(id).is_some_and(|c| !c.is_dm() && !c.is_member)
    }

    /// Apply `client.counts` unread state.
    pub fn apply_counts(&mut self, counts: &Value) {
        for key in ["channels", "mpims", "ims"] {
            for c in counts[key].as_array().into_iter().flatten() {
                let Some(id) = c["id"].as_str() else { continue };
                self.unread.insert(
                    id.to_string(),
                    Unread { has_unreads: c["has_unreads"].as_bool().unwrap_or(false), mentions: c["mention_count"].as_u64().unwrap_or(0) },
                );
            }
        }
    }

    pub fn total_unread(&self) -> (usize, u64) {
        let n = self.unread.values().filter(|u| u.has_unreads).count();
        let m = self.unread.values().map(|u| u.mentions).sum();
        (n, m)
    }

    /// Typing users (active within 6 s) for a channel.
    pub fn typing_in(&self, channel: &str) -> Vec<String> {
        self.typing
            .get(channel)
            .map(|v| v.iter().filter(|(_, t)| t.elapsed().as_secs() < 6).map(|(u, _)| self.user_name(u)).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(ts: &str) -> Message {
        Message { ts: ts.into(), text: ts.into(), ..Default::default() }
    }

    #[test]
    fn timeline_ordering() {
        let mut t = Timeline::default();
        t.upsert(msg("3.0"));
        t.upsert(msg("1.0"));
        t.upsert(msg("2.0"));
        let mut edited = msg("2.0");
        edited.text = "edited".into();
        t.upsert(edited);
        assert_eq!(t.messages.iter().map(|m| m.ts.as_str()).collect::<Vec<_>>(), vec!["1.0", "2.0", "3.0"]);
        assert_eq!(t.messages[1].text, "edited");
        t.prepend_page(vec![msg("0.5"), msg("0.2"), msg("1.0")], true);
        assert_eq!(t.messages.first().unwrap().ts, "0.2");
        assert_eq!(t.messages.len(), 5);
        t.remove("3.0");
        assert_eq!(t.messages.len(), 4);
    }

    #[test]
    fn copies_span_timeline_and_thread() {
        let mut ws = Workspace::default();
        ws.timelines.entry("C1".into()).or_default().upsert(msg("1.0"));
        let thread = ws.threads.entry(thread_key("C1", "1.0")).or_default();
        thread.upsert(msg("1.0"));
        thread.upsert(msg("2.0"));
        ws.threads.entry(thread_key("C2", "1.0")).or_default().upsert(msg("1.0"));
        assert_eq!(ws.copies_mut("C1", "1.0").len(), 2);
        assert_eq!(ws.find_message("C1", "2.0").map(|m| m.ts.as_str()), Some("2.0"));
        assert!(ws.find_message("C2", "2.0").is_none());
        ws.remove_message("C1", "1.0");
        assert!(ws.find_message("C1", "1.0").is_none());
        assert!(ws.find_message("C2", "1.0").is_some());
    }

    #[test]
    fn dnd_windows() {
        let d = Dnd::from_json(&serde_json::json!({"dnd_enabled":true,"next_dnd_start_ts":100,"next_dnd_end_ts":200,"snooze_enabled":true,"snooze_endtime":50}));
        assert!(d.active(10.0) && !d.active(60.0) && d.active(150.0) && !d.active(200.0));
        assert!(!Dnd::from_json(&serde_json::json!({"dnd_enabled":false,"next_dnd_start_ts":100,"next_dnd_end_ts":200})).active(150.0));
        // Real dnd.info for a daily 00:00–09:00 schedule seen at 15:20: start today, end tomorrow.
        let daily = Dnd::from_json(&serde_json::json!({"dnd_enabled":true,"next_dnd_start_ts":1792270800,"next_dnd_end_ts":1792389600}));
        assert!(!daily.active(1792326040.0), "15:20 is outside 00:00–09:00");
        assert!(daily.active(1792270800.0 + 86_400.0 + 3.0 * 3600.0), "03:00 the next night is inside");
    }

    #[test]
    fn previewed_channel_is_in_the_sidebar() {
        let mut ws = Workspace::default();
        ws.conversations.insert("C1".into(), Conversation { id: "C1".into(), name: "mine".into(), is_channel: true, is_member: true, ..Default::default() });
        ws.add_channels(vec![
            Conversation { id: "C1".into(), name: "mine".into(), is_channel: true, num_members: Some(3), ..Default::default() },
            Conversation { id: "C2".into(), name: "other".into(), is_channel: true, ..Default::default() },
        ]);
        assert!(ws.conversations["C1"].is_member, "boot data is kept");
        assert_eq!(ws.conversations["C1"].num_members, Some(3));
        assert!(ws.is_outside("C2") && !ws.is_outside("C1"));
        assert_eq!(ws.sidebar().0, vec!["C1"]);
        ws.preview = Some("C2".into());
        assert_eq!(ws.sidebar().0, vec!["C1", "C2"]);
    }

    #[test]
    fn mpim_label() {
        let ws = Workspace::default();
        let c = Conversation { is_mpim: true, name: "mpdm-alice--bob--carol-1".into(), ..Default::default() };
        assert_eq!(ws.conversation_label(&c), "alice, bob, carol");
    }
}

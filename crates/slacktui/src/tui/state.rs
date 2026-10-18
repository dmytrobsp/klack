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
    pub team_id: String,
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
    pub dnd: bool,
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
        let mut channels: Vec<&Conversation> = self.conversations.values().filter(|c| !c.is_dm() && c.is_member && !c.is_archived).collect();
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
        self.dnd = boot["dnd"]["dnd_enabled"].as_bool().unwrap_or(false) && boot["dnd"]["snooze_enabled"].as_bool().unwrap_or(false);
        if let Some(prefs) = boot["prefs"]["muted_channels"].as_str() {
            self.muted = prefs.split(',').filter(|s| !s.is_empty()).map(String::from).collect();
        }
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
    }

    #[test]
    fn mpim_label() {
        let ws = Workspace::default();
        let c = Conversation { is_mpim: true, name: "mpdm-alice--bob--carol-1".into(), ..Default::default() };
        assert_eq!(ws.conversation_label(&c), "alice, bob, carol");
    }
}

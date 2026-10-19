//! Workspace data held by the UI: conversations, users, message timelines, unread state.

use std::collections::{BTreeMap, HashMap};

use serde_json::Value;
use slack_core::types::{Conversation, Message, User};

#[derive(Debug, Clone, Default)]
pub struct Unread {
    pub has_unreads: bool,
    pub mentions: u64,
}

/// Now in epoch seconds (debug builds: `KLACK_DAYS_LATER=N` pretends it is N days later, to try
/// what hides after a quiet month).
pub fn now_secs() -> f64 {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    #[cfg(debug_assertions)]
    if let Some(days) = std::env::var("KLACK_DAYS_LATER").ok().and_then(|d| d.parse::<f64>().ok()) {
        return now + days * 86_400.0;
    }
    now
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

    /// The newest page fetched again (a previewed channel, which gets no live events): its
    /// messages replace the copies held (edits, reactions, replies), and top-level messages in its
    /// time span that it no longer has were deleted. Older pages stay as they are.
    pub fn refresh_newest(&mut self, page: Vec<Message>) {
        let oldest = page.iter().map(Message::time).fold(f64::INFINITY, f64::min);
        let fresh: std::collections::HashSet<String> = page.iter().map(|m| m.ts.clone()).collect();
        self.messages.retain(|m| m.time() < oldest || fresh.contains(&m.ts) || m.thread_ts.as_deref().is_some_and(|t| t != m.ts));
        for m in page {
            self.upsert(m);
        }
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
    /// Custom emoji pictures are drawn in the message pane (set while drawing: the terminal
    /// draws pictures and the settings allow it).
    pub emoji_pictures: bool,
    /// Custom emoji names numbered for the placeholders that stand for their pictures in
    /// built message lines (`text::emoji_cell`); only grows, so numbers stay valid.
    pub emoji_ids: Vec<String>,
    pub emoji_index: HashMap<String, u32>,
    pub team_name: String,
    /// The workspace's id (`T…`).
    pub team_id: String,
    pub self_id: String,
    pub users: HashMap<String, User>,
    pub conversations: BTreeMap<String, Conversation>,
    pub unread: HashMap<String, Unread>,
    /// When each conversation last had a message (epoch seconds), for ordering direct messages
    /// and hiding quiet conversations.
    pub latest: HashMap<String, f64>,
    pub timelines: HashMap<String, Timeline>,
    /// thread key `channel/ts` → replies (including parent first)
    pub threads: HashMap<String, Timeline>,
    pub presence: HashMap<String, bool>,
    /// channel → (user, when) of typing notifications
    pub typing: HashMap<String, Vec<(String, std::time::Instant)>>,
    pub custom_emoji: HashMap<String, String>,
    /// Slack's own and apps' slash commands (`commands.list`), for suggestions.
    pub slack_commands: Vec<slack_core::types::SlashCommand>,
    pub muted: std::collections::HashSet<String>,
    pub dnd: Dnd,
    /// Active huddles by channel.
    pub huddles: HashMap<String, Huddle>,
    /// A channel we are not in, shown in the sidebar while it is being looked at.
    pub preview: Option<String>,
    /// The conversation open (or just jumped to): in the sidebar however quiet it is.
    pub shown: Option<String>,
    /// Starred conversations.
    pub starred: std::collections::HashSet<String>,
    /// Sidebar sections in display order (`users.channelSections.list`).
    pub sections: Vec<Section>,
    /// Local preferences (settings.toml).
    pub settings: super::settings::Settings,
    /// Slack-side notification preferences.
    pub notify: NotifyPrefs,
    /// People marked VIP.
    pub vip: std::collections::HashSet<String>,
}

/// A huddle going on in a conversation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Huddle {
    pub room: String,
    pub participants: Vec<String>,
    /// Its thread (the "A huddle started" message), once Slack has posted it.
    pub thread: Option<String>,
}

/// Desktop notification levels: `everything`, `mentions_dms` ("just mentions") or `nothing`.
#[derive(Debug, Clone, PartialEq)]
pub struct NotifyPrefs {
    /// Default for channels.
    pub global: String,
    /// Default for group DMs.
    pub group_dms: String,
    /// Per-conversation overrides.
    pub channels: HashMap<String, String>,
    /// Words that notify like a mention.
    pub keywords: Vec<String>,
}

impl Default for NotifyPrefs {
    fn default() -> Self {
        NotifyPrefs { global: "mentions_dms".into(), group_dms: "everything".into(), channels: HashMap::new(), keywords: vec![] }
    }
}

pub const LEVELS: [&str; 3] = ["everything", "mentions_dms", "nothing"];

pub fn level_label(level: &str) -> &'static str {
    match level {
        "everything" => "all new messages",
        "nothing" => "nothing",
        _ => "mentions and DMs",
    }
}

impl NotifyPrefs {
    /// From `all_notifications_prefs`; also returns the muted conversations.
    pub fn from_json(v: &Value) -> (NotifyPrefs, std::collections::HashSet<String>) {
        let g = &v["global"];
        let text = |k: &str, default: &str| g[k].as_str().filter(|s| !s.is_empty()).unwrap_or(default).to_string();
        let mut channels = HashMap::new();
        let mut muted = std::collections::HashSet::new();
        for (id, c) in v["channels"].as_object().into_iter().flatten() {
            if let Some(level) = c["desktop"].as_str().filter(|l| !l.is_empty() && *l != "default") {
                channels.insert(id.clone(), level.to_string());
            }
            if c["muted"].as_bool() == Some(true) {
                muted.insert(id.clone());
            }
        }
        let keywords = g["global_keywords"].as_str().unwrap_or_default().split(',').map(str::trim).filter(|k| !k.is_empty()).map(String::from).collect();
        (NotifyPrefs { global: text("global_desktop", "mentions_dms"), group_dms: text("global_mpdm_desktop", "everything"), channels, keywords }, muted)
    }

    /// The level in effect for a conversation (DMs always count as direct).
    pub fn level(&self, channel: &str, is_mpim: bool) -> &str {
        self.channels.get(channel).map(String::as_str).unwrap_or(if is_mpim { &self.group_dms } else { &self.global })
    }

    /// The first keyword in `text` (whole words, any case).
    pub fn keyword_in(&self, text: &str) -> Option<&str> {
        let words: Vec<String> = text.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-').filter(|w| !w.is_empty()).map(str::to_lowercase).collect();
        self.keywords.iter().map(String::as_str).find(|k| {
            let k = k.to_lowercase();
            if k.contains(' ') { text.to_lowercase().contains(&k) } else { words.contains(&k) }
        })
    }
}

/// A sidebar section. `kind` is Slack's type: `standard` (made by the user), `stars`,
/// `channels`, `direct_messages`, and others klack does not show.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Section {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub emoji: String,
    pub channels: Vec<String>,
    /// The section after this one (Slack keeps the order as a linked list).
    pub next: Option<String>,
}

impl Section {
    fn from_json(v: &Value) -> Option<Section> {
        let str_of = |k: &str| v[k].as_str().unwrap_or_default().to_string();
        let ids: Vec<String> = v["channel_ids_page"]["channel_ids"].as_array().into_iter().flatten().filter_map(|c| c.as_str()).map(String::from).collect();
        // The page is the whole list as far as seen (its `cursor` is just the last id, and the
        // web client never pages); say so if Slack ever counts more than it sent.
        if let Some(count) = v["channel_ids_page"]["count"].as_u64().filter(|c| *c as usize > ids.len()) {
            tracing::warn!("section {} has {count} conversations but only {} were sent", str_of("name"), ids.len());
        }
        Some(Section {
            id: v["channel_section_id"].as_str()?.to_string(),
            kind: v["channel_section_type"].as_str().or(v["type"].as_str()).unwrap_or("standard").to_string(),
            name: str_of("name"),
            emoji: str_of("emoji"),
            channels: ids,
            next: v["next_channel_section_id"].as_str().map(String::from),
        })
    }
}

/// Do Not Disturb, as `dnd.info` / `dnd_updated` report it (times in epoch seconds).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Dnd {
    /// Scheduled quiet hours as Slack reports them: the next start and the next end. They
    /// are not always one window (after today's window, start is today's and end is
    /// tomorrow's), and the schedule may skip days (weekdays only), so only the window that
    /// ends at `end` is known; `dnd.info` is asked again once it has passed.
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

    /// The known scheduled window has ended, so Slack should be asked for the next one.
    pub fn stale(&self, now: f64) -> bool {
        self.window.is_some_and(|(_, end)| now >= end)
    }

    pub fn active(&self, now: f64) -> bool {
        const DAY: f64 = 86_400.0;
        // The window ending at `end` starts at `start`'s time of day (local time, so not an hour
        // off across a DST change).
        let scheduled = self.window.is_some_and(|(start, end)| {
            let begins = if end - start <= DAY { start } else { super::clock::window_start(start as i64, end as i64).map_or(end - (end - start).rem_euclid(DAY), |t| t as f64) };
            begins <= now && now < end
        });
        self.snooze_until.is_some_and(|t| now < t) || scheduled
    }
}

/// Sections in linked-list order (the head is the one no other points to); anything off the
/// chain keeps its listed order at the end.
fn order_sections(mut list: Vec<Section>) -> Vec<Section> {
    let pointed: std::collections::HashSet<String> = list.iter().filter_map(|s| s.next.clone()).collect();
    let mut out = Vec::new();
    let mut cur = list.iter().position(|s| !pointed.contains(&s.id));
    while let Some(i) = cur {
        let s = list.remove(i);
        cur = s.next.as_ref().and_then(|n| list.iter().position(|x| x.id == *n));
        out.push(s);
    }
    out.extend(list);
    out
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

    /// A person's name for lists, with " (You)" for yourself.
    pub fn person_label(&self, id: &str) -> String {
        let name = self.user_name(id);
        if id == self.self_id { format!("{name} (You)") } else { name }
    }

    /// Sidebar label for a conversation.
    pub fn conversation_label(&self, c: &Conversation) -> String {
        if c.is_im {
            c.user.as_deref().map(|u| self.person_label(u)).unwrap_or_else(|| c.id.clone())
        } else if c.is_mpim {
            // mpdm-me--alice--bob-1 → Alice, Bob (without ourselves, display names where known)
            let me = self.users.get(&self.self_id).map(|u| u.name.as_str());
            let core = c.name.trim_start_matches("mpdm-").trim_end_matches("-1");
            let names: Vec<String> = core
                .split("--")
                .filter(|h| Some(*h) != me)
                .map(|h| self.users.values().find(|u| u.name == h).map(|u| u.display().to_string()).unwrap_or_else(|| h.to_string()))
                .collect();
            names.join(", ")
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

    /// Replace the sections with a `users.channelSections.list` result, in linked-list order.
    pub fn apply_sections(&mut self, v: &Value) {
        let list: Vec<Section> = v["channel_sections"].as_array().into_iter().flatten().filter_map(Section::from_json).collect();
        self.sections = order_sections(list);
    }

    /// `channel_section_upserted`: a new or renamed/moved section.
    pub fn upsert_section(&mut self, v: &Value) {
        let Some(mut s) = Section::from_json(v) else { return };
        if let Some(pos) = self.sections.iter().position(|x| x.id == s.id) {
            s.channels = self.sections.remove(pos).channels;
        }
        let at = s.next.as_ref().and_then(|n| self.sections.iter().position(|x| x.id == *n)).unwrap_or(self.sections.len());
        self.sections.insert(at, s);
    }

    pub fn remove_section(&mut self, id: &str) {
        self.sections.retain(|s| s.id != id);
    }

    /// Put conversations into section `id` (a conversation is in at most one section).
    pub fn place_in_section(&mut self, id: &str, channels: &[String]) {
        for s in &mut self.sections {
            if s.id == id {
                for c in channels {
                    if !s.channels.contains(c) {
                        s.channels.push(c.clone());
                    }
                }
            } else {
                s.channels.retain(|c| !channels.contains(c));
            }
        }
    }

    pub fn remove_from_section(&mut self, id: &str, channels: &[String]) {
        if let Some(s) = self.sections.iter_mut().find(|s| s.id == id) {
            s.channels.retain(|c| !channels.contains(c));
        }
    }

    /// The user-made section holding conversation `id`.
    pub fn section_of(&self, id: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.kind == "standard" && s.channels.iter().any(|c| c == id))
    }

    /// The sidebar as (header, conversations): Starred, then the sections in Slack's order,
    /// with Channels and Direct messages holding whatever no other section shows.
    pub fn sidebar_groups(&self) -> Vec<(String, Vec<String>)> {
        let (channels, dms) = self.sidebar();
        let shown = |id: &String| channels.contains(id) || dms.contains(id);
        let label = |id: &String| self.conversations.get(id).map(|c| self.conversation_label(c).to_lowercase()).unwrap_or_default();
        let starred: Vec<String> = channels.iter().chain(&dms).filter(|id| self.starred.contains(*id)).cloned().collect();
        let mut placed: std::collections::HashSet<&String> = starred.iter().collect();
        let mut custom: HashMap<&str, Vec<String>> = HashMap::new();
        for s in self.sections.iter().filter(|s| s.kind == "standard") {
            let mut ids: Vec<String> = s.channels.iter().filter(|id| shown(id) && !placed.contains(id)).cloned().collect();
            ids.sort_by_key(|id| label(id));
            placed.extend(s.channels.iter().filter(|id| shown(id)));
            custom.insert(&s.id, ids);
        }
        let rest = |list: &Vec<String>| list.iter().filter(|id| !placed.contains(id)).cloned().collect::<Vec<_>>();
        let (rest_channels, rest_dms) = (rest(&channels), rest(&dms));
        let mut out = Vec::new();
        let push = |name: &str, ids: Vec<String>, out: &mut Vec<(String, Vec<String>)>| out.push((name.to_string(), ids));
        if !self.sections.iter().any(|s| s.kind == "stars") && !starred.is_empty() {
            push("Starred", starred.clone(), &mut out);
        }
        let mut seen_channels = false;
        let mut seen_dms = false;
        for s in &self.sections {
            match s.kind.as_str() {
                "stars" if !starred.is_empty() => push("Starred", starred.clone(), &mut out),
                "standard" => push(&s.name, custom.remove(s.id.as_str()).unwrap_or_default(), &mut out),
                "channels" if !seen_channels => {
                    seen_channels = true;
                    push("Channels", rest_channels.clone(), &mut out);
                }
                "direct_messages" if !seen_dms => {
                    seen_dms = true;
                    push("Direct messages", rest_dms.clone(), &mut out);
                }
                _ => {}
            }
        }
        if !seen_channels {
            push("Channels", rest_channels, &mut out);
        }
        if !seen_dms {
            push("Direct messages", rest_dms, &mut out);
        }
        out
    }

    /// Sidebar order: channels (alphabetical), then DMs (open ones), Slackbot last.
    pub fn sidebar(&self) -> (Vec<String>, Vec<String>) {
        self.conversation_lists(true)
    }

    /// [`Self::sidebar`]'s lists with the quiet conversations too (Ctrl+K finds those).
    pub fn listed(&self) -> (Vec<String>, Vec<String>) {
        self.conversation_lists(false)
    }

    fn conversation_lists(&self, hide_quiet: bool) -> (Vec<String>, Vec<String>) {
        let shown = |c: &Conversation| c.is_member || self.preview.as_deref() == Some(c.id.as_str());
        let previewed = |c: &Conversation| self.preview.as_deref() == Some(c.id.as_str());
        let now = now_secs();
        let lively = |c: &&Conversation| !hide_quiet || self.lively(c, now);
        let mut channels: Vec<&Conversation> = self.conversations.values().filter(|c| !c.is_dm() && shown(c) && (!c.is_archived || previewed(c))).filter(lively).collect();
        channels.sort_by_key(|c| c.name.to_lowercase());
        let mut dms: Vec<&Conversation> = self
            .conversations
            .values()
            .filter(|c| c.is_dm() && !c.is_archived)
            .filter(|c| c.user.as_deref().is_none_or(|u| self.users.get(u).is_none_or(|u| !u.deleted)))
            .filter(lively)
            .collect();
        // The most recently active first, as Slack does; never-active ones by name at the end.
        dms.sort_by(|a, b| self.activity(&b.id).total_cmp(&self.activity(&a.id)).then_with(|| self.conversation_label(a).to_lowercase().cmp(&self.conversation_label(b).to_lowercase())));
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
        self.starred = boot["starred"].as_array().into_iter().flatten().filter_map(|s| s.as_str()).map(String::from).collect();
        self.apply_prefs(&boot["prefs"]);
    }

    /// Number custom emoji not numbered yet (after the list changed).
    pub fn number_emoji(&mut self) {
        for name in self.custom_emoji.keys() {
            if !self.emoji_index.contains_key(name) {
                self.emoji_index.insert(name.clone(), self.emoji_ids.len() as u32);
                self.emoji_ids.push(name.clone());
            }
        }
    }

    /// Apply user prefs (boot, or a `pref_change`): notification levels, keywords, mutes, VIPs.
    pub fn apply_prefs(&mut self, prefs: &Value) {
        let all = match &prefs["all_notifications_prefs"] {
            Value::String(s) => serde_json::from_str(s).unwrap_or(Value::Null),
            v => v.clone(),
        };
        if !all.is_null() {
            let (notify, muted) = NotifyPrefs::from_json(&all);
            self.notify = notify;
            self.muted = muted;
        }
        // Older workspaces keep mutes here.
        if let Some(list) = prefs["muted_channels"].as_str() {
            self.muted.extend(list.split(',').filter(|s| !s.is_empty()).map(String::from));
        }
        if let Some(list) = prefs["vip_users"].as_str() {
            self.vip = list.split(',').filter(|s| !s.is_empty()).map(String::from).collect();
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

    /// People we have a DM with who are not in the user list (`users.list` leaves out some
    /// system users, such as Slack's own `USLACK`).
    pub fn unknown_dm_users(&self) -> Vec<String> {
        let mut out: Vec<String> = self.conversations.values().filter(|c| c.is_im).filter_map(|c| c.user.clone()).filter(|u| !self.users.contains_key(u)).collect();
        out.sort();
        out.dedup();
        out
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
                // `latest`: the newest real message (zeros when there is none).
                if let Some(t) = c["latest"].as_str().and_then(|t| t.parse::<f64>().ok()).filter(|t| *t > 0.0) {
                    self.note_activity(id, t);
                }
                if let (Some(conv), Some(ts)) = (self.conversations.get_mut(id), c["last_read"].as_str()) {
                    conv.last_read = Some(ts.to_string());
                }
                self.unread.insert(
                    id.to_string(),
                    Unread { has_unreads: c["has_unreads"].as_bool().unwrap_or(false), mentions: c["mention_count"].as_u64().unwrap_or(0) },
                );
            }
        }
    }

    /// Whether conversation `c` stays in the sidebar at `now`: it had a message (or was created)
    /// within `hide_inactive_days`, or is starred, unread, open, previewed or has a huddle.
    pub fn lively(&self, c: &Conversation, now: f64) -> bool {
        let days = self.settings.appearance.hide_inactive_days;
        if days == 0 || self.starred.contains(&c.id) || self.shown.as_deref() == Some(c.id.as_str()) || self.preview.as_deref() == Some(c.id.as_str()) || self.huddles.contains_key(&c.id) {
            return true;
        }
        if self.unread.get(&c.id).is_some_and(|u| u.has_unreads || u.mentions > 0) {
            return true;
        }
        let last = self.activity(&c.id).max(c.created.unwrap_or(0.0));
        last >= now - days as f64 * 86_400.0
    }

    /// A message at `t` in conversation `id`; true if that is newer than what was known.
    pub fn note_activity(&mut self, id: &str, t: f64) -> bool {
        let known = self.latest.entry(id.to_string()).or_insert(0.0);
        if t > *known {
            *known = t;
            true
        } else {
            false
        }
    }

    /// When conversation `id` last had a message (0: never, as far as known).
    pub fn activity(&self, id: &str) -> f64 {
        self.latest.get(id).copied().unwrap_or(0.0)
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
        // Real dnd.info for 17:00–09:00 on weekdays, seen on Sunday 17:24: next window Mon 17:00 → Tue 09:00.
        let weekdays = Dnd::from_json(&serde_json::json!({"dnd_enabled":true,"next_dnd_start_ts":1792418400,"next_dnd_end_ts":1792476000,"snooze_enabled":false}));
        assert!(!weekdays.active(1792333440.0), "Sunday evening is not in a weekday schedule");
        assert!(weekdays.active(1792418400.0 + 3.0 * 3600.0), "Monday 20:00 is");
        assert!(!weekdays.active(1792476000.0 + 60.0), "after the known window: unknown until dnd.info is asked again");
    }

    #[test]
    fn previewed_channel_is_in_the_sidebar() {
        let mut ws = Workspace::default();
        ws.settings.appearance.hide_inactive_days = 0;
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
        // An archived channel stays while it is previewed (it was open when archived).
        ws.conversations.get_mut("C2").unwrap().is_archived = true;
        assert_eq!(ws.sidebar().0, vec!["C1", "C2"]);
        ws.preview = None;
        assert_eq!(ws.sidebar().0, vec!["C1"]);
    }

    fn sections_ws() -> Workspace {
        let mut ws = Workspace::default();
        ws.settings.appearance.hide_inactive_days = 0;
        for (id, name) in [("C1", "alpha"), ("C2", "beta"), ("C3", "gamma")] {
            ws.conversations.insert(id.into(), Conversation { id: id.into(), name: name.into(), is_channel: true, is_member: true, ..Default::default() });
        }
        ws.conversations.insert("D1".into(), Conversation { id: "D1".into(), is_im: true, user: Some("U2".into()), ..Default::default() });
        // Shaped like the real users.channelSections.list, out of order on purpose.
        ws.apply_sections(&serde_json::json!({"channel_sections": [
            {"channel_section_id":"L3","type":"direct_messages","name":"Direct Messages","next_channel_section_id":"L5","channel_ids_page":{"channel_ids":[]}},
            {"channel_section_id":"L1","type":"stars","name":"","next_channel_section_id":"L2","channel_ids_page":{"channel_ids":[]}},
            {"channel_section_id":"L5","type":"standard","name":"Work","next_channel_section_id":null,"channel_ids_page":{"channel_ids":["C3","D1","C9"]}},
            {"channel_section_id":"L2","type":"channels","name":"Channels","next_channel_section_id":"L3","channel_ids_page":{"channel_ids":[]}}
        ]}));
        ws
    }

    #[test]
    fn sections_follow_the_linked_list() {
        let mut ws = sections_ws();
        assert_eq!(ws.sections.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), vec!["L1", "L2", "L3", "L5"]);
        ws.starred.insert("C2".into());
        let groups = ws.sidebar_groups();
        let names: Vec<(&str, Vec<&str>)> = groups.iter().map(|(n, ids)| (n.as_str(), ids.iter().map(String::as_str).collect())).collect();
        assert_eq!(names, vec![("Starred", vec!["C2"]), ("Channels", vec!["C1"]), ("Direct messages", vec![]), ("Work", vec!["C3", "D1"])]);
    }

    #[test]
    fn section_events() {
        let mut ws = sections_ws();
        ws.upsert_section(&serde_json::json!({"channel_section_id":"L6","channel_section_type":"standard","name":"New","next_channel_section_id":"L2"}));
        assert_eq!(ws.sections.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), vec!["L1", "L6", "L2", "L3", "L5"]);
        ws.place_in_section("L6", &["C3".into()]);
        assert_eq!(ws.section_of("C3").map(|s| s.name.as_str()), Some("New"), "moved out of Work");
        ws.upsert_section(&serde_json::json!({"channel_section_id":"L6","channel_section_type":"standard","name":"Renamed","next_channel_section_id":null}));
        assert_eq!((ws.sections.last().unwrap().name.as_str(), ws.sections.last().unwrap().channels.len()), ("Renamed", 1), "rename keeps channels");
        ws.remove_from_section("L6", &["C3".into()]);
        ws.remove_section("L5");
        assert!(ws.section_of("C3").is_none() && ws.section_of("D1").is_none());
    }

    #[test]
    fn dm_partners_missing_from_the_user_list() {
        let mut ws = Workspace::default();
        ws.settings.appearance.hide_inactive_days = 0;
        ws.users.insert("U1".into(), User { id: "U1".into(), ..Default::default() });
        for (id, user) in [("D1", "U1"), ("D2", "USLACK"), ("D3", "USLACK")] {
            ws.conversations.insert(id.into(), Conversation { id: id.into(), is_im: true, user: Some(user.into()), ..Default::default() });
        }
        assert_eq!(ws.unknown_dm_users(), vec!["USLACK"]);
    }

    #[test]
    fn notification_prefs_from_slack() {
        // Real all_notifications_prefs after setting #social to mentions and muting C2.
        let v = serde_json::json!({"channels":{"C1":{"desktop":"mentions_dms"},"C2":{"muted":true,"temp_mute_expiration_ts":0}},
            "global":{"global_desktop":"everything","global_keywords":"deploy, release notes","global_mpdm_desktop":"everything"}});
        let mut ws = Workspace::default();
        ws.apply_prefs(&serde_json::json!({"all_notifications_prefs": v.to_string(), "vip_users": "U2,U3"}));
        assert_eq!(ws.notify.level("C1", false), "mentions_dms");
        assert_eq!(ws.notify.level("C9", false), "everything");
        assert!(ws.muted.contains("C2") && !ws.muted.contains("C1"));
        assert_eq!(ws.notify.keyword_in("Time to DEPLOY!"), Some("deploy"));
        assert_eq!(ws.notify.keyword_in("redeployment"), None, "whole words only");
        assert_eq!(ws.notify.keyword_in("see the Release Notes"), Some("release notes"));
        assert_eq!(ws.vip.len(), 2);
    }

    #[test]
    fn mpim_label() {
        let mut ws = Workspace { self_id: "UME".into(), ..Default::default() };
        let c = Conversation { is_mpim: true, name: "mpdm-alice--bob--carol-1".into(), ..Default::default() };
        assert_eq!(ws.conversation_label(&c), "alice, bob, carol");
        ws.users.insert("UME".into(), User { id: "UME".into(), name: "alice".into(), ..Default::default() });
        let mut bob = User { id: "U2".into(), name: "bob".into(), ..Default::default() };
        bob.profile.display_name = "Bob B".into();
        ws.users.insert("U2".into(), bob);
        assert_eq!(ws.conversation_label(&c), "Bob B, carol", "without ourselves, display names where known");
        let own = Conversation { is_im: true, user: Some("UME".into()), ..Default::default() };
        assert_eq!(ws.conversation_label(&own), "alice (You)");
        assert_eq!(ws.person_label("U2"), "Bob B");
    }

    #[test]
    fn counts_carry_last_read() {
        let mut ws = Workspace::default();
        ws.conversations.insert("D1".into(), Conversation { id: "D1".into(), is_im: true, ..Default::default() });
        ws.apply_counts(&serde_json::json!({"ims": [{"id": "D1", "has_unreads": true, "mention_count": 0, "last_read": "1792357735.815229"}]}));
        assert!(ws.unread["D1"].has_unreads);
        assert_eq!(ws.conversations["D1"].last_read.as_deref(), Some("1792357735.815229"));
    }

    #[test]
    fn a_refreshed_newest_page_brings_edits_and_deletions() {
        let msg = |ts: &str, text: &str| Message { ts: ts.into(), text: text.into(), ..Default::default() };
        let mut t = Timeline { loaded: true, has_more: false, ..Default::default() };
        for (ts, text) in [("1.0", "old page"), ("5.0", "kept"), ("6.0", "to be edited"), ("7.0", "to be deleted")] {
            t.upsert(msg(ts, text));
        }
        // Slack's newest page now: 5 unchanged, 6 edited, 7 gone, 8 new (newest first).
        t.refresh_newest(vec![msg("8.0", "new"), msg("6.0", "edited"), msg("5.0", "kept")]);
        let texts: Vec<&str> = t.messages.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(texts, ["old page", "kept", "edited", "new"], "older than the page: untouched");
        assert!(!t.has_more, "paging state unchanged");
    }

    #[test]
    fn direct_messages_by_latest_activity() {
        let mut ws = Workspace::default();
        ws.settings.appearance.hide_inactive_days = 0;
        for (id, user, name) in [("D1", "U1", "bob"), ("D2", "U2", "carol"), ("D3", "U3", "alice")] {
            ws.users.insert(user.into(), User { id: user.into(), name: name.into(), ..Default::default() });
            ws.conversations.insert(id.into(), Conversation { id: id.into(), is_im: true, user: Some(user.into()), ..Default::default() });
        }
        // client.counts: zeros mean no message.
        ws.apply_counts(&serde_json::json!({"ims": [
            {"id": "D1", "latest": "1792356849.633269", "has_unreads": false},
            {"id": "D2", "latest": "1792360000.000100", "has_unreads": false},
            {"id": "D3", "latest": "0000000000.000000", "has_unreads": false}
        ]}));
        assert_eq!(ws.sidebar().1, ["D2", "D1", "D3"], "newest first, never-active last");
        assert!(ws.note_activity("D1", 1792370000.0), "a new message moves it");
        assert!(!ws.note_activity("D1", 1792000000.0), "an older one does not");
        assert_eq!(ws.sidebar().1, ["D1", "D2", "D3"]);
    }

    #[test]
    fn quiet_conversations_leave_the_sidebar() {
        let now = 1_800_000_000.0;
        let day = 86_400.0;
        let mut ws = Workspace::default();
        for (id, name, created) in [("C1", "busy", 0.0), ("C2", "quiet", 0.0), ("C3", "starred", 0.0), ("C4", "unread", 0.0), ("C5", "new", now - day), ("C6", "open", 0.0)] {
            ws.conversations.insert(id.into(), Conversation { id: id.into(), name: name.into(), is_channel: true, is_member: true, created: Some(created).filter(|c| *c > 0.0), ..Default::default() });
        }
        ws.note_activity("C1", now - 2.0 * day);
        ws.note_activity("C2", now - 40.0 * day);
        ws.note_activity("C3", now - 400.0 * day);
        ws.starred.insert("C3".into());
        ws.unread.insert("C4".into(), Unread { has_unreads: true, mentions: 0 });
        ws.shown = Some("C6".into());
        let lively: Vec<&str> = ["C1", "C2", "C3", "C4", "C5", "C6"].into_iter().filter(|id| ws.lively(&ws.conversations[*id], now)).collect();
        assert_eq!(lively, ["C1", "C3", "C4", "C5", "C6"], "only the 40-day quiet one goes (30 days)");
        ws.settings.appearance.hide_inactive_days = 60;
        assert!(ws.lively(&ws.conversations["C2"], now));
        ws.settings.appearance.hide_inactive_days = 0;
        let nothing_known = Conversation { id: "C9".into(), ..Default::default() };
        assert!(ws.lively(&nothing_known, now), "0 days: never hidden");
        // The full list (Ctrl+K) has them all whatever the setting (`sidebar` uses the clock).
        ws.settings.appearance.hide_inactive_days = 1;
        let real = now_secs();
        ws.latest.insert("C1".into(), real - 3600.0);
        ws.latest.insert("C2".into(), real - 40.0 * day);
        assert!(ws.listed().0.len() > ws.sidebar().0.len());
        assert_eq!(ws.listed().0.len(), 6);
    }
}

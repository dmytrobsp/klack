//! Workspace data held by the UI: conversations, users, message timelines, unread state.

use std::collections::{BTreeMap, HashMap};

use serde_json::Value;
use slack_core::dnd::Schedule;
use slack_core::types::{Conversation, Message, User};

#[derive(Debug, Clone, Default)]
pub struct Unread {
    pub has_unreads: bool,
    pub mentions: u64,
}

/// Threads you follow, together: replies you have not read (and mentions among them).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThreadUnread {
    pub has_unreads: bool,
    pub mentions: u64,
    /// Unread replies, over all threads.
    pub replies: u64,
}

impl ThreadUnread {
    /// From `client.counts`' `threads`, or an `update_global_thread_state` event (same fields).
    pub fn from_json(v: &Value) -> ThreadUnread {
        let replies = v["unread_count_by_channel"].as_object().map_or(0, |by| by.values().filter_map(Value::as_u64).sum());
        ThreadUnread { has_unreads: v["has_unreads"].as_bool().unwrap_or(false), mentions: v["mention_count"].as_u64().unwrap_or(0), replies }
    }
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
    /// Replies in threads you follow not read yet (the sidebar's Threads row).
    pub thread_unread: ThreadUnread,
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
    /// The `dnd_*` prefs seen so far (boot, then one per `pref_change`), for `dnd.schedule`.
    pub dnd_prefs: serde_json::Map<String, Value>,
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
    /// The notification schedule (from the prefs), to tell a quiet period longer than a day
    /// from Slack's "next start" of a daily one.
    pub schedule: Option<Schedule>,
    /// The account's time zone from the boot (None: not given; see `Workspace::account_zone`).
    pub zone: Option<chrono_tz::Tz>,
}

impl Dnd {
    pub fn from_json(v: &Value) -> Dnd {
        let num = |k: &str| v[k].as_f64().filter(|t| *t > 0.0);
        let window = match (v["dnd_enabled"].as_bool().unwrap_or(false), num("next_dnd_start_ts"), num("next_dnd_end_ts")) {
            (true, Some(start), Some(end)) => Some((start, end)),
            _ => None,
        };
        let snooze_until = num("snooze_endtime").filter(|_| v["snooze_enabled"].as_bool().unwrap_or(false));
        Dnd { window, snooze_until, ..Dnd::default() }
    }

    /// A new `dnd.info` / `dnd_status`, keeping the schedule and the zone.
    pub fn set_info(&mut self, v: &Value) {
        *self = Dnd { schedule: self.schedule.take(), zone: self.zone, ..Dnd::from_json(v) };
    }

    /// The known scheduled window has ended, so Slack should be asked for the next one.
    pub fn stale(&self, now: f64) -> bool {
        self.window.is_some_and(|(_, end)| now >= end)
    }

    /// When notifications come back if they are paused now: the end of the snooze or of the
    /// scheduled quiet period, whichever runs longer when one runs into the other.
    pub fn until(&self, now: f64) -> Option<f64> {
        self.until_in(now, self.zone)
    }

    /// `until` with the schedule in `zone` (None: klack's).
    pub fn until_in(&self, now: f64, zone: Option<chrono_tz::Tz>) -> Option<f64> {
        const DAY: f64 = 86_400.0;
        let zone = zone.or_else(super::clock::zone);
        // A window longer than a day is one quiet period when the schedule is quiet all through it
        // (a custom day paused all day); otherwise it is Slack's report of a daily schedule, and
        // the window ending at `end` starts at `start`'s time of day (local time, so not an hour
        // off across a DST change).
        let window = self.window.map(|(start, end)| {
            let whole = end - start <= DAY || self.schedule.as_ref().is_some_and(|s| quiet_throughout(s, zone, start as i64, end as i64));
            let begins = if whole { start } else { super::clock::window_start_in(zone, start as i64, end as i64).map_or(end - (end - start).rem_euclid(DAY), |t| t as f64) };
            (begins, end)
        });
        match (self.snooze_until.filter(|t| now < *t), window) {
            (Some(snooze), Some((begins, end))) if begins <= snooze && now < end => Some(snooze.max(end)),
            (Some(snooze), _) => Some(snooze),
            (None, Some((begins, end))) if begins <= now && now < end => Some(end),
            _ => None,
        }
    }

    pub fn active(&self, now: f64) -> bool {
        self.until(now).is_some()
    }

    /// What resuming notifications has to end now. With nothing known to be on (klack may be
    /// behind Slack), both: ending what isn't on does nothing.
    pub fn resume(&self, now: f64) -> Resume {
        let snooze = self.snooze_until.is_some_and(|t| now < t);
        // While snoozed, Slack reports the snooze itself as the next window: that is not a period.
        let own = |w: &(f64, f64)| self.snooze_until == Some(w.1);
        let period = Dnd { snooze_until: None, window: self.window.filter(|w| !own(w)), ..self.clone() }.active(now);
        if snooze || period { Resume { snooze, period } } else { Resume { snooze: true, period: true } }
    }
}

/// The Slack calls that resume notifications: `dnd.endSnooze` for a snooze, `dnd.endDnd` for
/// the current scheduled quiet period (it leaves the schedule alone).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resume {
    pub snooze: bool,
    pub period: bool,
}

/// Whether `schedule` pauses notifications all the way from `start` to `end`: at `start` and at
/// each time of day in between where that can change, in `zone` (None: the system's).
fn quiet_throughout(schedule: &Schedule, zone: Option<chrono_tz::Tz>, start: i64, end: i64) -> bool {
    use chrono::{Datelike, Timelike};
    let quiet = |t: i64| super::clock::in_zone(zone, t).is_some_and(|d| schedule.quiet_at(d.weekday().num_days_from_monday() as usize, d.hour() * 60 + d.minute()));
    let (Some(first), Some(last)) = (super::clock::in_zone(zone, start), super::clock::in_zone(zone, end)) else { return false };
    let marks = schedule.boundaries();
    quiet(start) && first.date_naive().iter_days().take_while(|d| *d <= last.date_naive()).all(|date| marks.iter().filter_map(|m| super::clock::local_minute(zone, date, *m)).filter(|t| start < *t && *t < end).all(quiet))
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

    /// Whether conversation `c` is listed in the sidebar at `now` (with `hide_quiet`, quiet ones
    /// are not): channels you are in (or preview) that are not archived, DMs with people who
    /// are not deactivated.
    fn is_listed(&self, c: &Conversation, now: f64, hide_quiet: bool) -> bool {
        let previewed = self.preview.as_deref() == Some(c.id.as_str());
        let kept = if c.is_dm() {
            !c.is_archived && c.user.as_deref().is_none_or(|u| self.users.get(u).is_none_or(|u| !u.deleted))
        } else {
            (c.is_member || previewed) && (!c.is_archived || previewed)
        };
        kept && (!hide_quiet || self.lively(c, now))
    }

    fn conversation_lists(&self, hide_quiet: bool) -> (Vec<String>, Vec<String>) {
        let now = now_secs();
        let mut channels: Vec<&Conversation> = self.conversations.values().filter(|c| !c.is_dm() && self.is_listed(c, now, hide_quiet)).collect();
        channels.sort_by_key(|c| c.name.to_lowercase());
        let mut dms: Vec<&Conversation> = self.conversations.values().filter(|c| c.is_dm() && self.is_listed(c, now, hide_quiet)).collect();
        // The most recently active first, as Slack does; never-active ones by name at the end.
        dms.sort_by(|a, b| self.activity(&b.id).total_cmp(&self.activity(&a.id)).then_with(|| self.conversation_label(a).to_lowercase().cmp(&self.conversation_label(b).to_lowercase())));
        (channels.into_iter().map(|c| c.id.clone()).collect(), dms.into_iter().map(|c| c.id.clone()).collect())
    }

    /// The account's time zone, which the schedule is in: the boot's (kept up to date by your
    /// own `user_change`), else your profile's (None: klack's).
    pub fn account_zone(&self) -> Option<chrono_tz::Tz> {
        self.dnd.zone.or_else(|| self.users.get(&self.self_id)?.tz.as_deref()?.parse().ok())
    }

    /// `Dnd::until` in the account's zone.
    pub fn dnd_until(&self, now: f64) -> Option<f64> {
        self.dnd.until_in(now, self.account_zone())
    }

    /// Notifications are paused now (in the account's zone).
    pub fn dnd_active(&self, now: f64) -> bool {
        self.dnd_until(now).is_some()
    }

    /// `Dnd::resume` in the account's zone.
    pub fn dnd_resume(&self, now: f64) -> Resume {
        Dnd { zone: self.account_zone(), ..self.dnd.clone() }.resume(now)
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
        self.dnd.set_info(&boot["dnd"]);
        self.dnd.zone = boot["self"]["tz"].as_str().and_then(|z| z.parse().ok()).or(self.dnd.zone);
        self.starred = boot["starred"].as_array().into_iter().flatten().filter_map(|s| s.as_str()).map(String::from).collect();
        self.apply_prefs(&boot["prefs"]);
    }

    /// "Sent using @App" for a message a person sent through an app (`app_id` beside their own
    /// `user`, no `bot_id`): the app's bot user by name, as Slack names it; "an app" when that
    /// bot user is not known.
    pub fn sent_using(&self, m: &Message) -> Option<String> {
        let app = m.app_id.as_deref().filter(|_| m.bot_id.is_none())?;
        let person = m.user.as_deref().and_then(|u| self.users.get(u));
        if person.is_some_and(|u| u.is_bot) || m.user.is_none() {
            return None;
        }
        let bot = self.users.values().find(|u| u.is_bot && !u.deleted && u.profile.api_app_id.as_deref() == Some(app));
        Some(bot.map_or_else(|| "Sent using an app".to_string(), |b| format!("Sent using @{}", b.display())))
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

    /// Apply user prefs (boot, or a `pref_change`): notification levels, keywords, mutes, VIPs,
    /// the notification schedule.
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
        let dnd: Vec<(&String, &Value)> = prefs.as_object().into_iter().flatten().filter(|(k, _)| k.starts_with("dnd_")).collect();
        if !dnd.is_empty() {
            self.dnd_prefs.extend(dnd.into_iter().map(|(k, v)| (k.clone(), v.clone())));
            self.dnd.schedule = Some(Schedule::from_prefs(&Value::Object(self.dnd_prefs.clone())));
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
        if counts["threads"].is_object() {
            self.thread_unread = ThreadUnread::from_json(&counts["threads"]);
        }
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
        // As the sidebar shows them: listed conversations only (not archived ones, or ones Slack
        // counts that are not in the list); a muted one is not unread, but its mentions count.
        let now = now_secs();
        let (mut n, mut m) = (0, 0);
        for (id, u) in &self.unread {
            if !self.conversations.get(id).is_some_and(|c| self.is_listed(c, now, true)) {
                continue;
            }
            if u.has_unreads && !self.muted.contains(id) {
                n += 1;
            }
            m += u.mentions;
        }
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

    #[test]
    fn the_unread_count_is_what_the_sidebar_shows() {
        let mut ws = Workspace::default();
        for (id, archived) in [("C1", false), ("C2", false), ("C3", true)] {
            ws.conversations.insert(id.into(), Conversation { id: id.into(), name: id.to_lowercase(), is_channel: true, is_member: true, is_archived: archived, ..Default::default() });
        }
        ws.conversations.insert("D1".into(), Conversation { id: "D1".into(), is_im: true, user: Some("U2".into()), ..Default::default() });
        let unread = |mentions| Unread { has_unreads: true, mentions };
        ws.unread.insert("C1".into(), unread(0));
        ws.unread.insert("C2".into(), unread(1));
        ws.unread.insert("C3".into(), unread(0));
        ws.unread.insert("C9".into(), unread(0));
        ws.unread.insert("D1".into(), unread(2));
        ws.muted.insert("C2".into());
        // C1 and D1 are bold in the sidebar; C2 is muted (dim, its mention badged), C3 archived
        // and C9 unknown are not listed.
        assert_eq!(ws.total_unread(), (2, 3));
    }

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
    fn dnd_until() {
        // Real dnd.info answers (test account, Tue 2026-10-20 00:58 Asia/Jerusalem, now = 1792447100).
        let now = 1792447100.0;
        let neither = Dnd::from_json(&serde_json::json!({"dnd_enabled":false,"next_dnd_end_ts":1,"next_dnd_start_ts":1,"ok":true,"snooze_enabled":false}));
        assert_eq!(neither.until(now), None);
        // Snoozed 30 min with no schedule: Slack reports the snooze as the next window too.
        let snooze = Dnd::from_json(&serde_json::json!({"dnd_enabled":true,"next_dnd_end_ts":1792449014,"next_dnd_start_ts":1792447214,"ok":true,"snooze_enabled":true,"snooze_endtime":1792449014,"snooze_remaining":1799}));
        assert_eq!(snooze.until(1792447220.0), Some(1792449014.0));
        assert_eq!(snooze.until(1792449014.0), None);
        // Weekdays 17:00–9:00, after midnight: today's part of the period, 00:00–09:00.
        let scheduled = Dnd::from_json(&serde_json::json!({"dnd_enabled":true,"next_dnd_end_ts":1792476000,"next_dnd_start_ts":1792443600,"ok":true,"snooze_enabled":false}));
        assert_eq!(scheduled.until(now), Some(1792476000.0));
        assert_eq!(scheduled.until(1792476000.0), None);
        // Snoozed 30 min inside a scheduled period ending 08:00: paused until 08:00.
        let both = Dnd::from_json(&serde_json::json!({"dnd_enabled":true,"next_dnd_end_ts":1792472400,"next_dnd_start_ts":1792443600,"ok":true,"snooze_enabled":true,"snooze_endtime":1792448903,"snooze_remaining":1800}));
        assert_eq!(both.until(now), Some(1792472400.0));
        // A snooze running into the next period lasts to its end; one ending before it doesn't.
        let ahead = |snooze: f64| Dnd { window: Some((now + 3600.0, now + 7200.0)), snooze_until: Some(snooze), ..Dnd::default() };
        assert_eq!(ahead(now + 3600.0).until(now), Some(now + 7200.0));
        assert_eq!(ahead(now + 3000.0).until(now), Some(now + 3000.0));
        // A snooze outlasting the period.
        assert_eq!(Dnd { window: Some((now - 60.0, now + 60.0)), snooze_until: Some(now + 600.0), ..Dnd::default() }.until(now), Some(now + 600.0));
        for (d, t) in [(&neither, now), (&snooze, 1792447220.0), (&scheduled, now), (&both, now), (&scheduled, 1792476001.0)] {
            assert_eq!(d.active(t), d.until(t).is_some());
        }
    }

    #[test]
    fn resume_ends_what_pauses() {
        // The real dnd.info answers of `dnd_until`.
        let now = 1792447100.0;
        let ends = |snooze, period| Resume { snooze, period };
        let snooze = Dnd::from_json(&serde_json::json!({"dnd_enabled":true,"next_dnd_end_ts":1792449014,"next_dnd_start_ts":1792447214,"ok":true,"snooze_enabled":true,"snooze_endtime":1792449014,"snooze_remaining":1799}));
        assert_eq!(snooze.resume(1792447220.0), ends(true, false), "a snooze is ended; its own window (Slack reports it as next_dnd) is not a scheduled period");
        let scheduled = Dnd::from_json(&serde_json::json!({"dnd_enabled":true,"next_dnd_end_ts":1792476000,"next_dnd_start_ts":1792443600,"ok":true,"snooze_enabled":false}));
        assert_eq!(scheduled.resume(now), ends(false, true), "in a scheduled period: endDnd only");
        let both = Dnd::from_json(&serde_json::json!({"dnd_enabled":true,"next_dnd_end_ts":1792472400,"next_dnd_start_ts":1792443600,"ok":true,"snooze_enabled":true,"snooze_endtime":1792448903,"snooze_remaining":1800}));
        assert_eq!(both.resume(now), ends(true, true), "a snooze inside a period: both");
        // A snooze before the next period: the snooze only.
        let ahead = Dnd { window: Some((now + 3600.0, now + 7200.0)), snooze_until: Some(now + 600.0), ..Dnd::default() };
        assert_eq!(ahead.resume(now), ends(true, false));
        // Nothing known to be on (klack may be behind Slack): both, ending nothing costs nothing.
        let neither = Dnd::from_json(&serde_json::json!({"dnd_enabled":false,"next_dnd_end_ts":1,"next_dnd_start_ts":1,"ok":true,"snooze_enabled":false}));
        assert_eq!(neither.resume(now), ends(true, true));
        assert_eq!(scheduled.resume(1792476000.0), ends(true, true), "the period is over");
    }

    #[test]
    fn dnd_periods_longer_than_a_day() {
        use slack_core::dnd::{DayHours, Schedule, ScheduleDays};
        let between = || DayHours::Between("8:00".into(), "22:00".into());
        let custom = |days: &[(usize, DayHours)]| {
            let mut per_day: [DayHours; 7] = std::array::from_fn(|_| between());
            for (i, d) in days {
                per_day[*i] = d.clone();
            }
            Schedule { days: ScheduleDays::Custom, range: ("9:00".into(), "17:00".into()), per_day, weekdays_off_allday: false }
        };
        let jerusalem: chrono_tz::Tz = "Asia/Jerusalem".parse().unwrap();
        let dnd = |info: serde_json::Value, schedule: Schedule| Dnd { schedule: Some(schedule), zone: Some(jerusalem), ..Dnd::from_json(&info) };
        // Real dnd.info for a custom schedule with Tuesday paused all day: Tue 00:00 → Wed 08:00.
        let all_day = dnd(serde_json::json!({"dnd_enabled":true,"next_dnd_start_ts":1792443600,"next_dnd_end_ts":1792558800,"snooze_enabled":false}), custom(&[(1, DayHours::Never)]));
        assert_eq!(all_day.until(1792447200.0), Some(1792558800.0), "Tue 01:00 is inside");
        assert_eq!(all_day.until(1792486800.0 + 11.0 * 3600.0), Some(1792558800.0), "Tue 23:00 too");
        assert_eq!(all_day.until(1792558800.0), None);
        // Real: Tuesday's notifications 0:30–1:30 → quiet Tue 01:30 → Wed 08:00, seen at 00:57.
        let evening = dnd(serde_json::json!({"dnd_enabled":true,"next_dnd_start_ts":1792449000,"next_dnd_end_ts":1792558800,"snooze_enabled":false}), custom(&[(1, DayHours::Between("0:30".into(), "1:30".into()))]));
        assert_eq!(evening.until(1792447200.0), None, "01:00 is before it");
        assert_eq!(evening.until(1792486800.0), Some(1792558800.0), "Tue 12:00 is inside");
        // A weekend paused all day: Fri 22:00 → Mon 08:00 (across the end of summer time).
        let weekend = dnd(serde_json::json!({"dnd_enabled":true,"next_dnd_start_ts":1792782000,"next_dnd_end_ts":1792994400,"snooze_enabled":false}), custom(&[(5, DayHours::Never), (6, DayHours::Never)]));
        assert_eq!(weekend.until(1792832400.0), Some(1792994400.0), "Saturday noon");
        assert_eq!(weekend.until(1792990800.0), Some(1792994400.0), "Monday 07:00");
        // Slack's daily report (00:00–09:00 seen at 15:20: start today, end tomorrow) stays daily
        // with the schedule known: the afternoon in between is not quiet.
        let daily = Schedule { days: ScheduleDays::EveryDay, range: ("9:00".into(), "0:00".into()), per_day: std::array::from_fn(|_| between()), weekdays_off_allday: false };
        let kyiv = Dnd { zone: Some("Europe/Kyiv".parse().unwrap()), ..dnd(serde_json::json!({"dnd_enabled":true,"next_dnd_start_ts":1792270800,"next_dnd_end_ts":1792389600}), daily) };
        assert_eq!(kyiv.until(1792326040.0), None, "15:20");
        assert_eq!(kyiv.until(1792270800.0 + 86_400.0 + 3.0 * 3600.0), Some(1792389600.0), "03:00 the next night");
        // A custom period that isn't quiet all through (a day with notifications in it) is daily too.
        let gap = dnd(serde_json::json!({"dnd_enabled":true,"next_dnd_start_ts":1792443600,"next_dnd_end_ts":1792558800}), custom(&[]));
        assert_eq!(gap.until(1792486800.0), None, "Tue 12:00 gets notifications");
        // dnd.info keeps the schedule and the zone.
        let mut kept = all_day.clone();
        kept.set_info(&serde_json::json!({"dnd_enabled":false}));
        assert_eq!((kept.window, kept.schedule.is_some(), kept.zone), (None, true, Some(jerusalem)));
    }

    #[test]
    fn the_schedule_follows_the_prefs() {
        let mut ws = Workspace::default();
        let info = serde_json::json!({"dnd_enabled":true,"next_dnd_start_ts":1792443600,"next_dnd_end_ts":1792558800,"snooze_enabled":false});
        ws.apply_boot(&serde_json::json!({"self": {"tz": "Asia/Jerusalem"}, "dnd": info, "prefs": {"dnd_enabled": true, "dnd_days": "custom", "dnd_enabled_tuesday": "partial", "dnd_before_tuesday": "8:00", "dnd_after_tuesday": "22:00"}}));
        assert_eq!(ws.dnd.zone, "Asia/Jerusalem".parse().ok());
        assert_eq!(ws.dnd.until(1792486800.0), None, "Tue 12:00 gets notifications");
        // A `pref_change` brings one pref: Tuesday paused all day.
        ws.apply_prefs(&serde_json::json!({"dnd_enabled_tuesday": "all_day"}));
        assert_eq!(ws.dnd.schedule.as_ref().map(|s| s.days), Some(slack_core::dnd::ScheduleDays::Custom));
        ws.dnd.set_info(&info);
        assert_eq!(ws.dnd.until(1792486800.0), Some(1792558800.0));
    }

    #[test]
    fn the_account_zone_falls_back_to_your_profile() {
        use slack_core::types::User;
        // Tuesday paused all day in a custom schedule: quiet at Tue 12:00 in Jerusalem, but read
        // in New York the period starts on a Monday afternoon with notifications, so it isn't one.
        let info = serde_json::json!({"dnd_enabled":true,"next_dnd_start_ts":1792443600,"next_dnd_end_ts":1792558800,"snooze_enabled":false});
        let prefs = serde_json::json!({"dnd_enabled": true, "dnd_days": "custom", "dnd_enabled_tuesday": "all_day", "dnd_start_hour": "17:00", "dnd_end_hour": "9:00"});
        let me = |tz: &str| User { id: "UME".into(), tz: Some(tz.into()), ..Default::default() };
        let (jerusalem, new_york): (chrono_tz::Tz, chrono_tz::Tz) = ("Asia/Jerusalem".parse().unwrap(), "America/New_York".parse().unwrap());
        let mut ws = Workspace { self_id: "UME".into(), ..Default::default() };
        ws.users.insert("UME".into(), me("Asia/Jerusalem"));
        ws.apply_boot(&serde_json::json!({"self": {}, "dnd": info, "prefs": prefs}));
        assert_eq!(ws.account_zone(), Some(jerusalem), "no zone in the boot: your profile's");
        assert_eq!(ws.dnd_until(1792486800.0), Some(1792558800.0));
        assert_eq!(crate::tui::prefs::zone_note(ws.account_zone(), Some(new_york), 1_800_000_000), " (Asia/Jerusalem)");
        ws.users.insert("UME".into(), me("America/New_York"));
        assert_eq!(ws.account_zone(), Some(new_york));
        assert_eq!(ws.dnd_until(1792486800.0), None, "read in your profile's zone");
        // The boot's zone, when it has one, is used.
        ws.apply_boot(&serde_json::json!({"self": {"tz": "Asia/Jerusalem"}, "dnd": info, "prefs": prefs}));
        assert_eq!(ws.account_zone(), Some(jerusalem));
        assert_eq!(ws.dnd_until(1792486800.0), Some(1792558800.0));
        assert!(ws.dnd_active(1792486800.0));
        // Your own zone changed during the session (`user_change`): followed.
        crate::tui::live::apply(&mut ws, &serde_json::json!({"type": "user_change", "user": {"id": "UME", "name": "me", "tz": "America/New_York"}}), None);
        assert_eq!(ws.account_zone(), Some(new_york));
        assert_eq!(ws.dnd_until(1792486800.0), None);
        // Someone else's change is theirs.
        crate::tui::live::apply(&mut ws, &serde_json::json!({"type": "user_change", "user": {"id": "U2", "name": "b", "tz": "Asia/Jerusalem"}}), None);
        assert_eq!(ws.account_zone(), Some(new_york));
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

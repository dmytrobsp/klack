//! Activity (Ctrl+A), a view in the main pane: mentions, reactions to your messages, thread
//! replies and direct messages from `activity.feed`, newest first under date headers, in tabs (All ·
//! DMs · Mentions · Threads; a key keeps the unread ones only). Each item shows who, when, the
//! message itself (a few lines, as a conversation draws it) and where it happened; Enter goes there.
//! Message texts are fetched per item as they are shown.

use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use serde_json::Value;
use slack_core::types::{File, Message};

use super::applink::{AppLink, Here, Target};
use super::i18n::tr;
use super::media::Media;
use super::messages::{self, Avatars, ImageSlot};
use super::state::Workspace;
use super::text;

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// `at_user`, `at_channel`, … : a message mentioning you (or everyone).
    Mention(String),
    Reaction { name: String, user: String },
    /// New replies in a thread you follow: how many are unread, the newest and the first unread one.
    Thread { unread: u64, latest: String, first_unread: String },
    /// New messages in a direct or group message (`dm`, `mpdm`): the newest of them.
    Dm { latest_ts: String },
    /// New messages in a channel (`channel`): how many; the item is the newest of them.
    Channel { unread: u64 },
    /// An app's messages to you (`bot_dm_bundle`): how many are unread; the thread the newest is in.
    App { unread: u64, thread: Option<String> },
    /// You were added to a channel (`internal_channel_invite`): the item is its join message.
    Invite,
    /// Something with no message of its own (`quietly_added_to_channel`, `prejoin_dm_welcome_party_alert`, the Slack
    /// Connect invitations, the list items), by type; the item's channel is the conversation it is about, if any.
    Notice(String),
    Other(String),
}

impl Default for Kind {
    fn default() -> Kind {
        Kind::Other(String::new())
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Item {
    pub key: String,
    /// Slack's item type (`at_user`, `thread_v2`, `dm`, …).
    pub ty: String,
    /// When it came (or last changed) as Slack writes it (`when` is the same as a number).
    pub feed_ts: String,
    pub kind: Kind,
    pub channel: String,
    /// The message the item is about (for threads: the parent).
    pub ts: String,
    pub author: Option<String>,
    pub unread: bool,
    pub when: f64,
}

/// Mark as read the items opened in this run since they came (`seen`: see
/// `threads::Seen`; a thread's own key counts for its replies).
pub fn apply_seen(items: &mut [Item], seen: &super::threads::Seen) {
    for it in items.iter_mut().filter(|it| it.unread) {
        let opened = |key: &str, t: f64| seen.get(key).is_some_and(|at| *at >= t);
        it.unread = match &it.kind {
            Kind::Thread { latest, .. } => !opened(&super::state::thread_key(&it.channel, &it.ts), latest.parse().unwrap_or(f64::MAX)) && !opened(&it.key, it.when),
            _ => !opened(&it.key, it.when),
        };
    }
}

impl Item {
    fn from_json(v: &Value) -> Option<Item> {
        let it = &v["item"];
        let ty = it["type"].as_str()?;
        let thread = &it["bundle_info"]["payload"]["thread_entry"];
        let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
        let feed_ts = v["feed_ts"].as_str().unwrap_or_default();
        // (Shapes as Slack's web client reads them; those with no message of their own get no ts.)
        fn notice<'a>(ty: &str, channel: &'a Value) -> (Kind, &'a str, &'a str) {
            (Kind::Notice(ty.to_string()), channel.as_str().unwrap_or_default(), "")
        }
        let (kind, channel, ts) = match ty {
            "channel" => {
                let entry = &it["bundle_info"]["payload"]["channel_entry"];
                (Kind::Channel { unread: entry["unread_msg_count"].as_u64().unwrap_or(0) }, entry["latest_message"]["channel"].as_str()?, entry["latest_message"]["ts"].as_str().unwrap_or(feed_ts))
            }
            "bot_dm_bundle" => {
                let m = &it["bundle_info"]["payload"]["message"];
                let ts = m["ts"].as_str().unwrap_or(feed_ts);
                (Kind::App { unread: it["bundle_info"]["unread_count"].as_u64().unwrap_or(0), thread: m["thread_ts"].as_str().filter(|t| *t != ts).map(String::from) }, m["channel"].as_str()?, ts)
            }
            "internal_channel_invite" => (Kind::Invite, it["message"]["channel"].as_str()?, it["message"]["ts"].as_str()?),
            "unjoined_channel_mention" => {
                let p = &it["unjoined_channel_mention_payload"];
                (Kind::Mention(ty.to_string()), p["channel_id"].as_str()?, p["message_ts"].as_str().unwrap_or(feed_ts))
            }
            "quietly_added_to_channel" => notice(ty, &it["quietly_added_to_channel_payload"]["channel_id"]),
            "prejoin_dm_welcome_party_alert" => notice(ty, &it["prejoin_dm_welcome_party_payload"]["channel_id"]),
            "external_channel_invite" | "external_dm_invite" | "shared_workspace_invite" | "list_record_assigned" | "list_record_edited" | "list_user_mentioned" => notice(ty, &Value::Null),
            "thread_v2" => (
                Kind::Thread { unread: thread["unread_msg_count"].as_u64().unwrap_or(0), latest: text(&thread["latest_ts"]), first_unread: text(&thread["min_unread_ts"]) },
                thread["channel_id"].as_str()?,
                thread["thread_ts"].as_str()?,
            ),
            "dm" | "mpdm" => {
                let latest = &it["bundle_info"]["payload"]["dm_entry"]["latest_message"];
                (Kind::Dm { latest_ts: text(&latest["ts"]) }, latest["channel"].as_str()?, latest["ts"].as_str()?)
            }
            "message_reaction" => (
                Kind::Reaction { name: text(&it["reaction"]["name"]), user: text(&it["reaction"]["user"]) },
                it["message"]["channel"].as_str()?,
                it["message"]["ts"].as_str()?,
            ),
            t if t.starts_with("at_") || t == "keyword" => (Kind::Mention(t.to_string()), it["message"]["channel"].as_str()?, it["message"]["ts"].as_str()?),
            t => (Kind::Other(t.to_string()), it["message"]["channel"].as_str().unwrap_or_default(), it["message"]["ts"].as_str().unwrap_or_default()),
        };
        Some(Item {
            key: v["key"].as_str().unwrap_or_default().to_string(),
            ty: ty.to_string(),
            feed_ts: feed_ts.to_string(),
            kind,
            channel: channel.to_string(),
            ts: ts.to_string(),
            // Who: the author; who assigned, edited or mentioned you in a list item; who is new.
            author: [&it["message"]["author_user_id"], &it["list_payload"]["assigned_by"], &it["list_edit_payload"]["edited_by"], &it["list_user_mention_payload"]["mentioned_by"], &it["prejoin_dm_welcome_party_payload"]["invitee_user_id"]]
                .into_iter()
                .find_map(Value::as_str)
                .map(String::from),
            unread: v["is_unread"].as_bool().unwrap_or(false),
            when: feed_ts.parse().unwrap_or(0.0),
        })
    }

    /// The item of a pushed `activity` event (`activity_updated`: its `entry` is shaped as a listed item).
    pub fn from_event(ev: &Value) -> Option<Item> {
        (ev["subtype"] == "activity_updated").then(|| Item::from_json(&ev["entry"])).flatten()
    }

    /// The message the item shows (channel, ts): a thread's newest reply, else its own message.
    pub fn message_key(&self) -> (String, String) {
        match &self.kind {
            Kind::Thread { latest, .. } if !latest.is_empty() => (self.channel.clone(), latest.clone()),
            _ => (self.channel.clone(), self.ts.clone()),
        }
    }

    /// A thread's parent (channel, ts), for who is in it.
    pub fn parent_key(&self) -> Option<(String, String)> {
        matches!(self.kind, Kind::Thread { .. }).then(|| (self.channel.clone(), self.ts.clone()))
    }
}

pub fn parse(v: &Value) -> Vec<Item> {
    v["items"].as_array().into_iter().flatten().filter_map(Item::from_json).collect()
}

/// The view's tabs, each a set of `activity.feed` item types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Tab {
    #[default]
    All,
    Dms,
    Mentions,
    Threads,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::All, Tab::Dms, Tab::Mentions, Tab::Threads];

    /// The item types it asks for (`bot_dm` would fail the whole request: never).
    pub fn types(self) -> &'static str {
        match self {
            Tab::All => ALL_TYPES,
            Tab::Dms => "dm,mpdm,bot_dm_bundle",
            Tab::Mentions => "at_user,at_user_group,at_channel,at_everyone,keyword,unjoined_channel_mention",
            Tab::Threads => "thread_v2",
        }
    }

    /// Its name in the tab bar (through `tr`).
    pub fn name(self) -> &'static str {
        match self {
            Tab::All => "All",
            Tab::Dms => "DMs",
            Tab::Mentions => "Mentions",
            Tab::Threads => "Threads",
        }
    }

    /// The tab after it (after the last: the first).
    pub fn next(self) -> Tab {
        Tab::ALL[(self.index() + 1) % Tab::ALL.len()]
    }

    /// The tab before it (before the first: the last).
    pub fn prev(self) -> Tab {
        Tab::ALL[(self.index() + Tab::ALL.len() - 1) % Tab::ALL.len()]
    }

    fn index(self) -> usize {
        Tab::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    /// Whether its list has items of type `ty`.
    pub fn lists(self, ty: &str) -> bool {
        self.types().split(',').any(|t| t == ty)
    }
}

/// The feed's item types on the All tab (`activity.feed`; the sidebar row's count when Slack does not give its badge):
/// every type the badge counts ([`BADGE_TYPES`]), and reactions and group DMs (the feed takes each of them).
pub const ALL_TYPES: &str = "at_user,at_user_group,at_channel,at_everyone,keyword,message_reaction,thread_v2,dm,mpdm,channel,bot_dm_bundle,internal_channel_invite,quietly_added_to_channel,unjoined_channel_mention,prejoin_dm_welcome_party_alert,external_channel_invite,external_dm_invite,shared_workspace_invite,list_record_assigned,list_record_edited,list_user_mentioned";

/// How many of `items` are unread (the sidebar's Activity row when Slack does not say).
pub fn unread_count(items: &[Item]) -> usize {
    items.iter().filter(|it| it.unread).count()
}

/// The item types Slack's own Activity badge adds up (its `activity_v2` counts; reactions, group DMs and the
/// types behind its feature checks other than DMs never count).
const BADGE_TYPES: &[&str] = &[
    "thread_v2", "channel", "internal_channel_invite", "list_record_edited", "bot_dm_bundle", "at_user", "at_user_group", "at_channel", "at_everyone", "keyword", "list_record_assigned", "list_user_mentioned",
    "external_channel_invite", "external_dm_invite", "shared_workspace_invite", "unjoined_channel_mention", "quietly_added_to_channel", "prejoin_dm_welcome_party_alert", "dm",
];

/// Whether an unread item of type `ty` is in the Activity badge.
pub fn counts_in_badge(ty: &str) -> bool {
    BADGE_TYPES.contains(&ty)
}

/// The Activity badge from Slack's unread counts by item type (`activity_v2` of `client.counts` and
/// `badge_counts_updated`); none when they are not given.
pub fn badge_total(activity_v2: &Value) -> Option<usize> {
    let counts = activity_v2.as_object()?;
    Some(counts.iter().filter(|(ty, _)| counts_in_badge(ty)).map(|(_, n)| n.as_u64().unwrap_or(0) as usize).sum())
}

/// The item types `activity.markRead` is also sent `ts` (the item's `feed_ts`) for, as Slack's client does: DMs, the
/// invitations and the list items.
const MARKED_WITH_TS: &[&str] = &["dm", "internal_channel_invite", "quietly_added_to_channel", "external_channel_invite", "external_dm_invite", "shared_workspace_invite", "list_record_assigned", "list_record_edited", "list_user_mentioned"];

/// What `activity.markRead` is sent when the unread item `it` is opened, as Slack's client does (`type`, `feed_ts`,
/// `key`; a DM, an invitation or a list item: also its `feed_ts` as `ts`); none for a reaction (never marked) or an
/// item read already.
pub fn mark_read_params(it: &Item) -> Option<Vec<(&'static str, String)>> {
    if !it.unread || it.ty == "message_reaction" || it.ty.is_empty() || it.key.is_empty() || it.feed_ts.is_empty() {
        return None;
    }
    let mut params = vec![("type", it.ty.clone()), ("feed_ts", it.feed_ts.clone()), ("key", it.key.clone())];
    if MARKED_WITH_TS.contains(&it.ty.as_str()) {
        params.push(("ts", it.feed_ts.clone()));
    }
    Some(params)
}

/// Lines of a message an item shows; a longer one is cut with "…" (Enter shows it all).
pub const EXCERPT_LINES: usize = 3;

const TODAY: &str = "Today";
const YESTERDAY: &str = "Yesterday";
const MISSING: &str = "message not available";
const LOADING: &str = "Loading…";
const LOADING_MORE: &str = "Loading more…";
const MENTION: &str = "Mention in {where}";
const GROUP_MENTION: &str = "Group mention in {where}";
const CHANNEL_MENTION: &str = "@channel in {where}";
const EVERYONE_MENTION: &str = "@everyone in {where}";
const KEYWORD: &str = "Keyword in {where}";
const IN_THREAD: &str = "{where} thread";
const THREAD: &str = "Thread in {where}";
const NEW_REPLY: &str = "1 new reply";
const NEW_REPLIES: &str = "{n} new replies";
const DIRECT: &str = "Direct message";
const GROUP_DM: &str = "Group message with {where}";
const REACTED: &str = "Reacted {emoji} to:";
const YOUR_MESSAGE: &str = "Your message in {where}";
const OTHER: &str = "{what} in {where}";
const CHANNEL_NEW: &str = "New messages in {where}";
const APP: &str = "App message in {where}";
const NEW: &str = "{n} new";
const ADDED: &str = "You were added to {where}";
const UNJOINED: &str = "Mention in {where}, a channel you are not in";
const WELCOME: &str = "Someone new to welcome in {where}";
const CONNECT_CHANNEL: &str = "An invitation to a Slack Connect channel (accept it in Slack)";
const CONNECT_DM: &str = "An invitation to a Slack Connect DM (accept it in Slack)";
const SHARED_WORKSPACE: &str = "An invitation to a shared workspace (accept it in Slack)";
const LIST_ASSIGNED: &str = "A list item assigned to you (open it in Slack)";
const LIST_EDITED: &str = "A list item you follow was edited (open it in Slack)";
const LIST_MENTION: &str = "A mention in a list (open it in Slack)";
/// Said when Enter is pressed on an item klack has nowhere to show (a list item, a Slack Connect invitation).
pub const NOT_HERE: &str = "klack can't show this one: open it in Slack";
const UNREAD_ONLY: &str = "unread only";
const EMPTY: &str = "Nothing here yet: mentions, reactions to your messages, thread replies and direct messages show up here.";
const NONE_UNREAD: &str = "Nothing unread here.";
const FAILED: &str = "The activity could not be loaded";

/// The view's words, for the locale template.
pub const WORDS: &[&str] = &[
    TODAY, YESTERDAY, MISSING, LOADING, LOADING_MORE, MENTION, GROUP_MENTION, CHANNEL_MENTION, EVERYONE_MENTION, KEYWORD, IN_THREAD, THREAD, NEW_REPLY, NEW_REPLIES, DIRECT, GROUP_DM, REACTED, YOUR_MESSAGE, OTHER, CHANNEL_NEW, APP, NEW, ADDED, UNJOINED, WELCOME, CONNECT_CHANNEL, CONNECT_DM, SHARED_WORKSPACE, LIST_ASSIGNED, LIST_EDITED, LIST_MENTION, NOT_HERE, UNREAD_ONLY, EMPTY, NONE_UNREAD, FAILED, "All", "DMs", "Mentions", "Threads",
];

/// A tab's list while the view is open, when another tab is shown.
#[derive(Default)]
struct Kept {
    items: Option<Vec<Item>>,
    selected: usize,
    scroll: usize,
    cursor: Option<String>,
    loading: bool,
}

#[derive(Default)]
pub struct Activity {
    /// The tab shown; its list is `items`.
    pub tab: Tab,
    pub items: Option<Vec<Item>>,
    /// The selected item (an index into `items`; always one that is shown).
    pub selected: usize,
    /// Why the feed could not be loaded (said in the view).
    pub failed: Option<String>,
    /// Only the unread items are shown (kept for the run: each opening of the view keeps it).
    pub unread_only: bool,
    /// Where the next page of the list starts (none: it is all there), and whether it is being loaded.
    pub cursor: Option<String>,
    pub loading: bool,
    /// Why the next page could not be loaded (said at the list's end): not asked for again until you move in the list,
    /// filter it, show another tab or open the view again.
    page_error: Option<String>,
    /// The end of the list was on screen at the last draw.
    at_end: bool,
    /// The other tabs' lists, kept while the view is open.
    kept: HashMap<Tab, Kept>,
    /// Messages fetched for items, by (channel, ts); None when it is gone.
    pub messages: HashMap<(String, String), Option<Message>>,
    /// Messages asked for (loaded or not).
    pub requested: HashSet<(String, String)>,
    /// Lines of the list above the view.
    pub scroll: usize,
    /// Bring the selection into view at the next draw (it moved by the keys).
    follow: bool,
    /// Last drawn area (empty while the view is not shown), for the mouse.
    pub area: Rect,
    /// The list's rows (under the tab bar) at the last draw, and the item each row belongs to.
    pub list: Rect,
    rows: Vec<Option<usize>>,
    /// Where each tab was drawn in the tab bar.
    tab_spots: Vec<(Rect, Tab)>,
    /// The items on screen at the last draw.
    shown: Vec<usize>,
    /// The item the pointer is over (not the selection), at the last `Moved`.
    pub hover: Option<usize>,
    /// Where links were drawn at the last draw: as in the message panes.
    links: Vec<(Rect, String, Option<AppLink>, bool)>,
    /// Pictures on screen but not loaded at the last draw.
    pub pictures: Vec<(File, (u16, u16))>,
    /// How many times the list was built (laid out) to be drawn.
    pub builds: u64,
    /// The list changed since it was last built.
    dirty: bool,
    /// The list as last built.
    cache: Option<Cache>,
}

/// The list as last built for drawing, and what from: the tab, the filter, how many items and messages (each change
/// to them also marks it `dirty`) and how it looks; the items shown (indices into `items`), the lines, the item each
/// line belongs to, and where its links are.
struct Cache {
    of: (Tab, bool, usize, usize),
    look: messages::Look,
    visible: Vec<usize>,
    built: Built,
    owners: Vec<Option<usize>>,
    links: Vec<messages::LinkSpot>,
}

impl Activity {
    /// The items shown (all, or the unread ones), as indices into `items`.
    pub fn visible(&self) -> Vec<usize> {
        let Some(items) = &self.items else { return Vec::new() };
        (0..items.len()).filter(|&n| !self.unread_only || items[n].unread).collect()
    }

    /// The message of an item came (None: it is gone).
    pub fn add_message(&mut self, key: (String, String), message: Option<Message>) {
        self.messages.insert(key, message);
        self.dirty = true;
    }

    /// Mark the items opened in this run read (see [`apply_seen`]).
    pub fn apply_seen(&mut self, seen: &super::threads::Seen) {
        if let Some(items) = self.items.as_mut() {
            apply_seen(items, seen);
            self.dirty = true;
        }
    }

    /// Move the selection by `delta` shown items.
    pub fn move_by(&mut self, delta: isize) {
        self.page_error = None;
        let shown = self.visible();
        if shown.is_empty() {
            return;
        }
        let at = shown.iter().position(|&n| n == self.selected).unwrap_or(0) as isize;
        self.selected = shown[(at + delta).clamp(0, shown.len() as isize - 1) as usize];
        self.follow = true;
    }

    /// Items a page moves (those on screen at the last draw).
    pub fn page(&self) -> isize {
        self.shown.len().max(1) as isize
    }

    /// Show only the unread items, or all again; the selection stays on a shown item (the next one, else the one before).
    pub fn toggle_unread(&mut self) {
        self.dirty = true;
        self.unread_only = !self.unread_only;
        self.page_error = None;
        self.keep_selection_shown();
    }

    fn keep_selection_shown(&mut self) {
        let shown = self.visible();
        if !shown.contains(&self.selected) {
            self.selected = shown.iter().copied().find(|&n| n > self.selected).or(shown.iter().copied().rev().find(|&n| n < self.selected)).unwrap_or(0);
        }
        self.follow = true;
    }

    /// Show tab `tab`: its list as it was while the view has been open (none yet: to be loaded).
    pub fn set_tab(&mut self, tab: Tab) {
        if tab == self.tab {
            return;
        }
        self.dirty = true;
        let here = Kept { items: self.items.take(), selected: self.selected, scroll: self.scroll, cursor: self.cursor.take(), loading: self.loading };
        self.kept.insert(self.tab, here);
        let there = self.kept.remove(&tab).unwrap_or_default();
        (self.tab, self.items, self.selected, self.scroll, self.cursor, self.loading) = (tab, there.items, there.selected, there.scroll, there.cursor, there.loading);
        (self.failed, self.page_error, self.hover, self.at_end) = (None, None, None, false);
        self.keep_selection_shown();
    }

    /// Tab `tab`'s first page came (`cursor`: where the next one starts): shown if it is the tab shown (the selected
    /// item stays selected), else kept for it.
    pub fn set_items(&mut self, tab: Tab, items: Vec<Item>, cursor: Option<String>) {
        self.dirty = true;
        if tab != self.tab {
            let kept = self.kept.entry(tab).or_default();
            (kept.items, kept.cursor, kept.loading) = (Some(items), cursor, false);
            return;
        }
        let key = self.selected_item().map(|it| it.key.clone());
        self.selected = key.and_then(|k| items.iter().position(|it| it.key == k)).unwrap_or(0);
        (self.items, self.failed, self.page_error, self.cursor, self.loading) = (Some(items), None, None, cursor, false);
        self.keep_selection_shown();
    }

    /// The next page of tab `tab` came: its items not listed yet go at the end (none asked for — the list was loaded
    /// afresh meanwhile —: left out).
    pub fn add_page(&mut self, tab: Tab, page: Vec<Item>, cursor: Option<String>) {
        self.dirty = true;
        let (items, at, loading) = if tab == self.tab {
            (&mut self.items, &mut self.cursor, &mut self.loading)
        } else {
            let kept = self.kept.entry(tab).or_default();
            (&mut kept.items, &mut kept.cursor, &mut kept.loading)
        };
        if !*loading {
            return;
        }
        let items = items.get_or_insert_with(Vec::new);
        for it in page {
            if !items.iter().any(|o| o.key == it.key) {
                items.push(it);
            }
        }
        (*at, *loading) = (cursor, false);
        // (Filtered with none shown before: the selection onto the first one shown now.)
        if tab == self.tab && !self.visible().contains(&self.selected) {
            self.keep_selection_shown();
        }
    }

    /// An item Slack pushed (new, or changed: read, a newer reply): into each loaded list it belongs to, in its place
    /// by time (newest first); the selected item stays selected.
    pub fn upsert(&mut self, it: Item) {
        self.dirty = true;
        let key = self.selected_item().map(|i| i.key.clone());
        let lists = std::iter::once((self.tab, &mut self.items)).chain(self.kept.iter_mut().map(|(t, k)| (*t, &mut k.items)));
        for (_, items) in lists.filter(|(tab, _)| tab.lists(&it.ty)) {
            let Some(items) = items.as_mut() else { continue };
            items.retain(|o| o.key != it.key);
            let at = items.iter().position(|o| o.when < it.when).unwrap_or(items.len());
            items.insert(at, it.clone());
        }
        self.reselect(key);
    }

    /// The item `key` is gone (Slack deleted it): out of every list.
    pub fn remove_key(&mut self, key: &str) {
        self.dirty = true;
        let selected = self.selected_item().map(|i| i.key.clone()).filter(|k| k != key);
        for items in std::iter::once(&mut self.items).chain(self.kept.values_mut().map(|k| &mut k.items)).flatten() {
            items.retain(|o| o.key != key);
        }
        self.reselect(selected);
    }

    /// The selection back on item `key` after the list changed (gone: the one now at its place).
    fn reselect(&mut self, key: Option<String>) {
        let len = self.items.as_ref().map_or(0, Vec::len);
        let at = key.and_then(|k| self.items.as_ref()?.iter().position(|i| i.key == k));
        self.selected = at.unwrap_or(self.selected.min(len.saturating_sub(1)));
        self.keep_selection_shown();
    }

    /// A page could not be loaded: what is listed stays (the next one is asked for again when the end comes).
    pub fn page_failed(&mut self, tab: Tab) {
        if tab == self.tab {
            self.loading = false;
        } else if let Some(kept) = self.kept.get_mut(&tab) {
            kept.loading = false;
        }
    }

    /// Where the next page starts, when the end of the list was on screen and there is more (once: it is loading then).
    pub fn next_page(&mut self) -> Option<String> {
        let cursor = self.cursor.clone().filter(|_| self.at_end && !self.loading && self.page_error.is_none() && self.items.is_some())?;
        self.loading = true;
        Some(cursor)
    }

    /// Tab `tab`'s list (`more`: its next page) could not be loaded: said there, in place of the list when it has none
    /// to show; a page's failure at the list's end, and it is not asked for again by itself.
    pub fn fail(&mut self, tab: Tab, more: bool, error: String) {
        if tab == self.tab && self.items.is_none() {
            self.failed = Some(error);
        } else if tab == self.tab && more {
            self.page_error = Some(error);
        }
        self.page_failed(tab);
    }

    /// The item drawn at a terminal position (none on its date header or the gap after it).
    pub fn row_at(&self, column: u16, row: u16) -> Option<usize> {
        self.list.contains((column, row).into()).then(|| self.rows.get((row - self.list.y) as usize).copied().flatten()).flatten()
    }

    /// The tab drawn at a terminal position (in the tab bar).
    pub fn tab_at(&self, column: u16, row: u16) -> Option<Tab> {
        self.tab_spots.iter().find(|(at, _)| at.contains((column, row).into())).map(|(_, t)| *t)
    }

    /// The pointer moved to a position: hover the item there (none off the list).
    pub fn hover_at(&mut self, column: u16, row: u16) {
        self.hover = self.row_at(column, row);
    }

    pub fn selected_item(&self) -> Option<&Item> {
        self.items.as_ref()?.get(self.selected)
    }

    /// Where Enter on the selected item goes: a thread at its first unread reply (none unread: its newest); an app's
    /// messages in its DM (in the thread the newest is in); something with no message of its own in its conversation;
    /// anything else at its message (a reply: in its thread). None when it is about no conversation (a list item, a
    /// Slack Connect invitation).
    pub fn target(&self) -> Option<Target> {
        let it = self.selected_item().filter(|it| !it.channel.is_empty())?;
        let message = |ts: &str, thread_ts: Option<String>| Target::Message { channel: it.channel.clone(), ts: ts.to_string(), thread_ts };
        Some(match &it.kind {
            Kind::Thread { unread, latest, first_unread } => {
                let at = if *unread > 0 && !first_unread.is_empty() { first_unread } else if !latest.is_empty() { latest } else { &it.ts };
                message(at, Some(it.ts.clone()))
            }
            Kind::App { thread, .. } => message(&it.ts, thread.clone()),
            Kind::Notice(_) => Target::Conversation { channel: it.channel.clone() },
            _ => {
                let parent = self.messages.get(&it.message_key()).and_then(|m| m.as_ref()).filter(|m| m.is_reply()).and_then(|m| m.thread_ts.clone());
                message(&it.ts, parent)
            }
        })
    }

    /// The messages of the items on screen and around them (and the selection) not asked for yet.
    pub fn wanted(&self) -> Vec<(String, String)> {
        let Some(items) = &self.items else { return vec![] };
        let shown = self.visible();
        // (`shown` is in order: found by halving.)
        let positions: Vec<usize> = self.shown.iter().chain(std::iter::once(&self.selected)).filter_map(|n| shown.binary_search(n).ok()).collect();
        let (low, high) = (positions.iter().min().copied().unwrap_or(0), positions.iter().max().copied().unwrap_or(0));
        let mut out: Vec<(String, String)> = Vec::new();
        for &n in &shown[low.saturating_sub(5)..(high + 15).min(shown.len())] {
            for k in std::iter::once(items[n].message_key()).chain(items[n].parent_key()) {
                if !self.requested.contains(&k) && !out.contains(&k) && !k.1.is_empty() {
                    out.push(k);
                }
            }
        }
        out
    }

    /// The links to messages drawn now whose author is worth asking for.
    pub fn authors_wanted(&self) -> impl Iterator<Item = &AppLink> {
        self.links.iter().filter(|l| l.3).filter_map(|(_, _, app, _)| app.as_ref())
    }

    /// Any link drawn at a terminal position: its address and where it goes in klack.
    pub fn link_under(&self, column: u16, row: u16) -> Option<(String, Option<AppLink>)> {
        if !self.area.contains((column, row).into()) {
            return None;
        }
        self.links.iter().find(|(at, ..)| at.contains((column, row).into())).map(|(_, url, app, _)| (url.clone(), app.clone()))
    }

    /// Not drawn this time: nothing of it is under the mouse.
    pub fn hide(&mut self) {
        (self.area, self.list, self.hover) = (Rect::default(), Rect::default(), None);
        self.rows.clear();
        self.tab_spots.clear();
        self.shown.clear();
        self.links.clear();
    }

    /// Draw the view into `area` (the main pane), its frame lit while it has the focus: the tab bar, then the items.
    pub fn draw(&mut self, f: &mut Frame, area: Rect, here: &Here, focused: bool, media: &Media) {
        let p = super::theme::p();
        let ws = here.ws;
        self.area = area;
        self.rows.clear();
        self.tab_spots.clear();
        self.shown.clear();
        self.links.clear();
        self.pictures.clear();
        self.at_end = false;
        let border = Style::default().fg(if focused { p.accent } else { p.dim });
        let block = Block::default().borders(Borders::ALL).border_style(border).title(format!(" {} ", tr("Activity")));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let [bar, list] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
        self.draw_tabs(f, bar);
        self.list = list;
        let avatars = if !ws.settings.messages.avatars {
            Avatars::Off
        } else if media.avatars {
            Avatars::Pictures
        } else {
            Avatars::Initials
        };
        if self.items.is_some() {
            self.refresh(here, list.width, avatars);
        }
        let none_shown = self.cache.as_ref().is_none_or(|c| c.visible.is_empty());
        // (None shown with more to load — every item loaded is read —: the next page, as when the end is on screen.)
        self.at_end = none_shown && self.items.is_some() && self.cursor.is_some();
        let note = match &self.items {
            None if self.failed.is_some() => Some(Line::styled(format!("{}: {}", tr(FAILED), self.failed.as_deref().unwrap_or_default()), Style::default().fg(p.danger))),
            None => Some(Line::styled(tr(LOADING).into_owned(), Style::default().fg(p.dim))),
            Some(_) if none_shown && self.page_error.is_some() => Some(Line::styled(format!("{}: {}", tr(FAILED), self.page_error.as_deref().unwrap_or_default()), Style::default().fg(p.danger))),
            Some(_) if self.at_end => Some(Line::styled(tr(LOADING_MORE).into_owned(), Style::default().fg(p.dim))),
            Some(items) if none_shown => Some(Line::styled(tr(if items.is_empty() { EMPTY } else { NONE_UNREAD }).into_owned(), Style::default().fg(p.dim))),
            Some(_) => None,
        };
        // (A note in its place keeps the list as built.)
        if let Some(note) = note {
            f.render_widget(Paragraph::new(note).wrap(ratatui::widgets::Wrap { trim: true }), list);
            return;
        }
        let Some(cache) = self.cache.take() else { return };
        let (built, owners, shown) = (&cache.built, &cache.owners, &cache.visible);
        let height = list.height as usize;
        // Bring a selection moved by the keys into view (with its date header, when it is the first of its day).
        if std::mem::take(&mut self.follow) {
            if let (Some(first), Some(last)) = (owners.iter().position(|o| *o == Some(self.selected)), owners.iter().rposition(|o| *o == Some(self.selected))) {
                let first = if built.headers.contains(&first.saturating_sub(1)) { first - 1 } else { first };
                self.scroll = super::threads::bring_into_view(self.scroll, height, first, last);
            }
        }
        let more = usize::from(self.loading || self.page_error.is_some());
        let total = built.lines.len() + more;
        self.scroll = self.scroll.min(total.saturating_sub(height));
        // (The last item on screen: the line under the list too — loading, or why the next page failed.)
        if more > 0 && owners.iter().rposition(Option::is_some).is_some_and(|last| last < self.scroll + height) {
            self.scroll = total.saturating_sub(height);
        }
        // (No room at all leaves `total` one past the lines: start at most at their end.)
        let start = self.scroll.min(built.lines.len());
        let end = (start + height).min(built.lines.len());
        let mut lines: Vec<Line<'static>> = built.lines[start..end].to_vec();
        let mut rows: Vec<Option<usize>> = owners[start..end].to_vec();
        if self.loading && lines.len() < height {
            lines.push(Line::styled(tr(LOADING_MORE).into_owned(), Style::default().fg(p.dim)));
            rows.push(None);
        } else if let Some(e) = self.page_error.as_deref().filter(|_| lines.len() < height) {
            lines.push(Line::styled(format!("{}: {e}", tr(FAILED)), Style::default().fg(p.danger)));
            rows.push(None);
        }
        // (The last item's rows, or the selection on the last item: time for the next page.)
        self.at_end = (height > 0 && end == built.lines.len()) || shown.last() == Some(&self.selected);
        let selected = Some(&self.selected).filter(|_| focused);
        if let Some(sel) = selected {
            let highlight = Style::default().bg(p.selection);
            for (line, owner) in lines.iter_mut().zip(&rows) {
                if owner.as_ref() == Some(sel) {
                    *line = std::mem::take(line).patch_style(highlight);
                }
            }
        }
        let slots: Vec<ImageSlot> = built.slots.iter().filter(|s| s.line + s.size.1 as usize > start && s.line < end).cloned().collect();
        messages::fill_rows(f, list, &rows, &[selected], self.hover.as_ref(), &slots, start, 0, media);
        f.render_widget(Paragraph::new(lines), list);
        for spot in cache.links.iter().filter(|s| (start..end).contains(&s.line)) {
            let (x, y) = (list.x + spot.col, list.y + (spot.line - start) as u16);
            let width = spot.width.min(list.right().saturating_sub(x));
            messages::hyperlink(f.buffer_mut(), x, y, width, &spot.url);
            self.links.push((Rect { x, y, width, height: 1 }, spot.url.clone(), spot.app.clone(), spot.wants_author));
        }
        super::scroll::indicator_beside(f, area, list, start, end - start, total, border);
        self.pictures = messages::draw_pictures(f, list, slots, start, 0, media);
        let mut on_screen = HashSet::new();
        self.shown = rows.iter().flatten().copied().filter(|n| on_screen.insert(*n)).collect();
        self.rows = rows;
        self.cache = Some(cache);
    }

    /// Build the list again for drawing `width` wide if anything in it changed since it was last built: its items or
    /// their messages, the tab or the filter, or what it looks like ([`messages::Look`]: people, the settings, …).
    fn refresh(&mut self, here: &Here, width: u16, avatars: Avatars) {
        let of = (self.tab, self.unread_only, self.items.as_ref().map_or(0, Vec::len), self.messages.len());
        if !self.dirty && self.cache.as_ref().is_some_and(|c| c.of == of && c.look.holds(here, width, avatars)) {
            return;
        }
        let visible = self.visible();
        let all = self.items.as_deref().unwrap_or_default();
        let items: Vec<&Item> = visible.iter().map(|&n| &all[n]).collect();
        let built = build(here, &items, &self.messages, width, super::clock::now().date_naive(), avatars);
        let owners = built.owners.iter().map(|o| o.map(|n| visible[n])).collect();
        let links = messages::link_spots(here, &built.messages, &built.lines, &built.message_rows);
        // Whose statuses it shows: who reacted, the authors.
        let reacted = items.iter().filter_map(|it| match &it.kind {
            Kind::Reaction { user, .. } => Some(user.as_str()),
            _ => None,
        });
        let authors = items.iter().flat_map(|it| [it.author.as_deref(), self.messages.get(&it.message_key()).and_then(Option::as_ref).and_then(|m| m.user.as_deref())]).flatten();
        let look = messages::Look::new(here, width, avatars, reacted.chain(authors));
        self.cache = Some(Cache { of, look, visible, built, owners, links });
        self.dirty = false;
        self.builds += 1;
    }

    /// The tab bar: each tab's name (the one shown lit), then whether only the unread items are shown; cut when narrow.
    fn draw_tabs(&mut self, f: &mut Frame, bar: Rect) {
        let p = super::theme::p();
        let mut spans = Vec::new();
        let mut x = 0u16;
        for tab in Tab::ALL {
            let label = format!(" {} ", tr(tab.name()));
            let width = unicode_width::UnicodeWidthStr::width(label.as_str()) as u16;
            let style = if tab == self.tab { Style::default().bg(p.selection).fg(p.strong).add_modifier(Modifier::BOLD) } else { Style::default().fg(p.dim) };
            if x < bar.width {
                self.tab_spots.push((Rect { x: bar.x + x, y: bar.y, width: width.min(bar.width - x), height: 1 }, tab));
            }
            spans.push(Span::styled(label, style));
            spans.push(Span::raw(" "));
            x = x.saturating_add(width + 1);
        }
        if self.unread_only {
            spans.push(Span::styled(format!(" · {}", tr(UNREAD_ONLY)), Style::default().fg(p.accent)));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), bar);
    }
}

/// The list built: its lines, the item (an index into the items given) each line belongs to, the date headers' lines,
/// where pictures go, and for the links the messages shown and the message (ts) each line shows.
#[derive(Debug, Default)]
pub struct Built {
    pub lines: Vec<Line<'static>>,
    pub owners: Vec<Option<usize>>,
    pub headers: Vec<usize>,
    pub slots: Vec<ImageSlot>,
    pub messages: Vec<Message>,
    pub message_rows: Vec<Option<String>>,
}

/// The list of `items` (newest first) `width` columns wide: its lines and the item each line belongs to (none: date
/// headers and the blank rows between items). [`build`] with pictures is what the view draws.
#[cfg(test)]
pub fn layout(ws: &Workspace, items: &[Item], messages: &HashMap<(String, String), Option<Message>>, width: u16, today: NaiveDate) -> (Vec<Line<'static>>, Vec<Option<usize>>) {
    let built = build(&Here::alone(ws), &items.iter().collect::<Vec<_>>(), messages, width, today, Avatars::Off);
    (built.lines, built.owners)
}

/// [`layout`] with the avatars, the custom emoji pictures and the links (`today`: the day "Today" is).
pub fn build(here: &Here, items: &[&Item], messages: &HashMap<(String, String), Option<Message>>, width: u16, today: NaiveDate, avatars: Avatars) -> Built {
    let mut out = Built::default();
    let dim = Style::default().fg(super::theme::p().dim);
    // As wide as a conversation's text in a pane this wide (the gutter goes before it).
    let body = (width as usize).saturating_sub(1 + messages::GUTTER as usize);
    let mut last_day = None;
    for (n, it) in items.iter().enumerate() {
        let day = super::clock::at(it.when as i64).map(|d| d.date_naive());
        // A date header above the first item of each day (after the blank row that ends the one before).
        if day != last_day {
            last_day = day;
            let label = format!(" {} ", day_name(here.ws, it.when, today));
            let side = (width as usize).saturating_sub(label.chars().count()) / 2;
            out.headers.push(out.lines.len());
            out.lines.push(Line::styled(format!("{}{label}{}", "─".repeat(side), "─".repeat(side)), dim));
            out.owners.push(None);
            out.message_rows.push(None);
        }
        let start = out.lines.len();
        let parent = it.parent_key().and_then(|k| messages.get(&k)).and_then(Option::as_ref);
        let (lines, slots, rows, message) = item_lines(here, it, messages.get(&it.message_key()), parent, body, avatars);
        out.slots.extend(slots.into_iter().map(|mut s| {
            s.line += start;
            s
        }));
        out.owners.extend(std::iter::repeat_n(Some(n), lines.len()));
        out.lines.extend(lines);
        out.message_rows.extend(rows);
        out.messages.extend(message);
        // A blank row between items.
        out.lines.push(Line::raw(""));
        out.owners.push(None);
        out.message_rows.push(None);
    }
    out
}

/// "Today", "Yesterday" or the day as the preferences write it ("Monday, October 19").
fn day_name(ws: &Workspace, t: f64, today: NaiveDate) -> String {
    match super::clock::at(t as i64).map(|d| d.date_naive()) {
        Some(d) if d == today => tr(TODAY).into_owned(),
        Some(d) if Some(d) == today.pred_opt() => tr(YESTERDAY).into_owned(),
        _ => messages::day_label(ws, t),
    }
}

/// Nothing of the message is there to show (deleted, or Slack left it out).
fn missing(m: &Message) -> bool {
    m.text.trim().is_empty() && m.blocks.is_empty() && m.files.is_empty() && m.attachments.is_empty()
}

/// One item's lines (`width`: the text's), with the gutter (the unread bar on its first): who (in a thread, "+N" others
/// who replied in it besides you, from its `parent`), the status emoji and when; the message (up to
/// [`EXCERPT_LINES`], then "…"); where. Also its pictures, the message (ts) each line shows and that message, for the
/// links.
fn item_lines(here: &Here, it: &Item, msg: Option<&Option<Message>>, parent: Option<&Message>, width: usize, avatars: Avatars) -> (Vec<Line<'static>>, Vec<ImageSlot>, Vec<Option<String>>, Option<Message>) {
    let ws = here.ws;
    let p = super::theme::p();
    let dim = Style::default().fg(p.dim);
    let no_thumbs = |_: &File, _: u16| None;
    let message = msg.and_then(Option::as_ref).filter(|m| !missing(m));
    let large = avatars != Avatars::Off && ws.settings.messages.large_avatars;
    let (mut lines, mut slots, mut rows): (Vec<Line<'static>>, Vec<ImageSlot>, Vec<Option<String>>) = (Vec::new(), Vec::new(), Vec::new());
    // Who: the one who reacted; else the message's author (known from the item before the message is loaded).
    let reaction = match &it.kind {
        Kind::Reaction { name, user } => Some((name.as_str(), user.as_str())),
        _ => None,
    };
    let header_of = match (reaction, message) {
        (None, Some(m)) => Some(m.clone()),
        (Some((_, user)), _) if !user.is_empty() => Some(Message { user: Some(user.to_string()), ts: format!("{:.6}", it.when), ..Default::default() }),
        // (Nothing of its own to show: when it came.)
        (None, None) => it.author.clone().map(|u| Message { user: Some(u), ts: Some(it.message_key().1).filter(|t| !t.is_empty()).unwrap_or_else(|| format!("{:.6}", it.when)), ..Default::default() }),
        _ => None,
    };
    let mut body_from = 0;
    if let Some(h) = &header_of {
        let (built, _, built_slots) = messages::build_shown(here, &[h], 0, 1, width, true, None, &no_thumbs, avatars, None);
        // Its own message (not a reaction's): the header and the message as a conversation draws them.
        let own = reaction.is_none() && message.is_some();
        let keep = if own { built.len() } else if large { 2.min(built.len()) } else { 1.min(built.len()) };
        lines.extend(built.into_iter().take(keep));
        slots.extend(built_slots.into_iter().filter(|s| s.line < keep));
        rows.extend((0..lines.len()).map(|n| (own && n > 0).then(|| h.ts.clone())));
        body_from = 1;
        // Others in a thread besides the one shown (and you).
        let others = parent.map_or(0, |p| p.reply_users.iter().filter(|u| Some(u.as_str()) != h.user.as_deref() && **u != ws.self_id).count());
        if let Some(first) = lines.first_mut().filter(|_| others > 0) {
            let name = ws.author(h);
            if let Some(at) = first.spans.iter().position(|s| s.content == name) {
                first.spans.insert(at + 1, Span::styled(format!(" +{others}"), dim));
            }
        }
    }
    if let Some((name, _)) = reaction {
        // "Reacted (:x: 2) to:", then the message reacted to.
        let count = message.and_then(|m| m.reactions.iter().find(|r| r.name == name)).map_or(1, |r| r.count);
        let words = tr(REACTED);
        let (before, after) = words.split_once("{emoji}").unwrap_or((&words, ""));
        let mut lead = [Line::from(vec![Span::styled(before.to_string(), dim), messages::reaction_pill(&text::emoji_cell(ws, name, None), count, false), Span::styled(after.to_string(), dim)])];
        for (_, col, emoji) in text::emoji_cells(ws, &mut lead) {
            if let Some(file) = super::media::emoji_file(ws, &emoji) {
                slots.push(ImageSlot { line: lines.len(), col, file, size: super::media::AVATAR_SIZE });
            }
        }
        lines.extend(lead);
        rows.push(None);
        body_from = lines.len();
        if let Some(m) = message {
            // (Its reactions are the line above.)
            let bare = Message { reactions: Vec::new(), ..m.clone() };
            let (body, body_slots) = messages::message_body(here, &bare, width, false, &no_thumbs);
            let base = lines.len();
            slots.extend(body_slots.into_iter().map(|mut s| {
                s.line += base;
                s
            }));
            rows.extend(std::iter::repeat_n(Some(m.ts.clone()), body.len()));
            lines.extend(body);
        }
    }
    // (Waited for unless the item has no message of its own.)
    if message.is_none() && !it.message_key().1.is_empty() {
        let note = if msg.is_some() { MISSING } else { LOADING };
        lines.push(Line::from(Span::styled(tr(note).into_owned(), dim.add_modifier(Modifier::ITALIC))));
        rows.push(None);
    }
    // A longer message is cut.
    if lines.len() > body_from + EXCERPT_LINES {
        let end = body_from + EXCERPT_LINES;
        lines.truncate(end);
        rows.truncate(end);
        slots.retain(|s| s.line + s.size.1 as usize <= end);
        lines.push(Line::from(Span::styled("…", dim)));
        rows.push(None);
    }
    lines.push(Line::from(Span::styled(where_line(ws, it, message), dim)));
    rows.push(None);
    // The gutter: the unread bar beside the first line.
    for (n, line) in lines.iter_mut().enumerate() {
        let gutter = if n == 0 { vec![super::sidebar::unread_mark(it.unread), Span::raw(" ")] } else { vec![Span::raw("  ")] };
        line.spans.splice(0..0, gutter);
    }
    (lines, slots, rows, message.cloned())
}

/// Where the item happened: "Mention in #general thread", "Thread in #general · 2 new replies", "Direct message", ….
fn where_line(ws: &Workspace, it: &Item, message: Option<&Message>) -> String {
    let place = super::feed::place(ws, &it.channel);
    let in_thread = if message.is_some_and(Message::is_reply) { tr(IN_THREAD).replace("{where}", &place) } else { place.clone() };
    match &it.kind {
        Kind::Mention(t) => {
            let words = match t.as_str() {
                "at_user_group" => GROUP_MENTION,
                "at_channel" => CHANNEL_MENTION,
                "at_everyone" => EVERYONE_MENTION,
                "keyword" => KEYWORD,
                "unjoined_channel_mention" => UNJOINED,
                _ => MENTION,
            };
            tr(words).replace("{where}", &in_thread)
        }
        Kind::Reaction { .. } => tr(YOUR_MESSAGE).replace("{where}", &in_thread),
        Kind::Thread { unread, .. } => {
            let mut out = tr(THREAD).replace("{where}", &place);
            if *unread > 0 {
                out += " · ";
                out += &if *unread == 1 { tr(NEW_REPLY).into_owned() } else { tr(NEW_REPLIES).replace("{n}", &unread.to_string()) };
            }
            out
        }
        Kind::Dm { .. } if ws.conversations.get(&it.channel).is_some_and(|c| c.is_mpim) => tr(GROUP_DM).replace("{where}", &place),
        Kind::Dm { .. } => tr(DIRECT).into_owned(),
        Kind::Channel { unread } | Kind::App { unread, .. } => {
            let mut out = tr(if matches!(it.kind, Kind::Channel { .. }) { CHANNEL_NEW } else { APP }).replace("{where}", &place);
            if *unread > 0 {
                out += " · ";
                out += &tr(NEW).replace("{n}", &unread.to_string());
            }
            out
        }
        Kind::Invite => tr(ADDED).replace("{where}", &place),
        Kind::Notice(t) => {
            let words = match t.as_str() {
                "quietly_added_to_channel" => ADDED,
                "prejoin_dm_welcome_party_alert" => WELCOME,
                "external_channel_invite" => CONNECT_CHANNEL,
                "external_dm_invite" => CONNECT_DM,
                "shared_workspace_invite" => SHARED_WORKSPACE,
                "list_record_assigned" => LIST_ASSIGNED,
                "list_record_edited" => LIST_EDITED,
                "list_user_mentioned" => LIST_MENTION,
                _ => OTHER,
            };
            tr(words).replace("{what}", t).replace("{where}", &place)
        }
        Kind::Other(t) => tr(OTHER).replace("{what}", t).replace("{where}", &in_thread),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::applink::Here;

    fn item(key: &str, kind: Kind, channel: &str, ts: &str, author: Option<&str>, unread: bool, when: f64) -> Item {
        Item { key: key.into(), kind, channel: channel.into(), ts: ts.into(), author: author.map(String::from), unread, when, ..Default::default() }
    }

    fn plain(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.to_string()).collect()
    }

    fn ws() -> Workspace {
        let mut ws = Workspace::default();
        ws.conversations.insert("C1".into(), slack_core::types::Conversation { id: "C1".into(), name: "c".into(), is_channel: true, ..Default::default() });
        ws.conversations.insert("D1".into(), slack_core::types::Conversation { id: "D1".into(), is_im: true, user: Some("U2".into()), ..Default::default() });
        let mut ann = slack_core::types::User { id: "U2".into(), name: "ann".into(), ..Default::default() };
        ann.profile.status_emoji = ":palm_tree:".into();
        ws.users.insert("U2".into(), ann);
        ws.users.insert("U3".into(), slack_core::types::User { id: "U3".into(), name: "bob".into(), ..Default::default() });
        ws
    }

    /// Noon `days_ago` days before today, in the user's time zone.
    fn noon(days_ago: u64) -> f64 {
        let day = super::super::clock::now().date_naive() - chrono::Days::new(days_ago);
        super::super::clock::local_minute(super::super::clock::zone(), day, 12 * 60).unwrap() as f64
    }

    fn today() -> NaiveDate {
        super::super::clock::now().date_naive()
    }

    #[test]
    fn items_opened_here_are_seen() {
        let it = |key: &str, kind: Kind, when: f64| item(key, kind, "C1", "1.0", None, true, when);
        let thread = |latest: &str| Kind::Thread { unread: 2, latest: latest.into(), first_unread: String::new() };
        let mut items = vec![it("m1", Kind::Mention("at_user".into()), 5.0), it("m2", Kind::Mention("at_user".into()), 9.0), it("t1", thread("6.0"), 6.0), it("t2", thread("12.0"), 12.0)];
        items[3].ts = "2.0".into();
        let seen = super::super::threads::Seen::from([("m1".to_string(), 7.0), ("m2".to_string(), 7.0), ("C1/1.0".to_string(), 7.0), ("C1/2.0".to_string(), 7.0)]);
        apply_seen(&mut items, &seen);
        assert_eq!(items.iter().map(|i| i.unread).collect::<Vec<_>>(), [false, true, false, true], "opened after it came: seen; newer: still unread; a thread opened anywhere counts");
    }

    #[test]
    fn parses_real_items() {
        // Shapes captured from activity.feed in the testbed.
        let v = serde_json::json!({"items": [
            {"feed_ts":"1792326270.026269","is_unread":true,"item":{"bundle_info":{"payload":{"thread_entry":{"channel_id":"D1","latest_ts":"1792326270.026269","min_unread_ts":"1792326100.000100","thread_ts":"1792325783.787339","unread_msg_count":1}}},"type":"thread_v2"},"key":"thread_v2-D1-1792325783.787339"},
            {"feed_ts":"1792326101.733899","is_unread":false,"item":{"message":{"author_user_id":"U2","channel":"C1","is_broadcast":false,"ts":"1792326101.733899"},"type":"at_user"},"key":"at_user-C1-1792326101.733899"},
            {"feed_ts":"1792323624.000000","is_unread":false,"item":{"message":{"channel":"D1","ts":"1792323487.950999"},"reaction":{"name":"heart","user":"U2"},"type":"message_reaction"},"key":"k3"},
            {"feed_ts":"1792323700.000000","is_unread":true,"is_archived":false,"item":{"bundle_info":{"payload":{"dm_entry":{"latest_message":{"channel":"D2","ts":"1792323699.000100"}}}},"type":"dm"},"key":"dm-D2","version":"1"}
        ]});
        let items = parse(&v);
        assert_eq!(items.len(), 4);
        assert_eq!((&items[3].kind, items[3].channel.as_str(), items[3].ts.as_str()), (&Kind::Dm { latest_ts: "1792323699.000100".into() }, "D2", "1792323699.000100"), "a DM: its newest message");
        assert_eq!(unread_count(&items), 2);
        assert_eq!(items[0].kind, Kind::Thread { unread: 1, latest: "1792326270.026269".into(), first_unread: "1792326100.000100".into() });
        assert_eq!((items[0].channel.as_str(), items[0].ts.as_str(), items[0].unread), ("D1", "1792325783.787339", true));
        assert_eq!((items[1].kind.clone(), items[1].author.as_deref()), (Kind::Mention("at_user".into()), Some("U2")));
        assert_eq!(items[2].kind, Kind::Reaction { name: "heart".into(), user: "U2".into() });
    }

    #[test]
    fn parses_a_dm_item() {
        let v = serde_json::json!({"items": [{"feed_ts": "2.0", "is_unread": true, "key": "dm-D1", "item": {"type": "dm", "bundle_info": {"payload": {"dm_entry": {"latest_message": {"channel": "D1", "ts": "1.0"}}}}}}]});
        let items = parse(&v);
        assert_eq!(items.len(), 1);
        assert_eq!((&items[0].kind, items[0].channel.as_str(), items[0].ts.as_str(), items[0].unread), (&Kind::Dm { latest_ts: "1.0".into() }, "D1", "1.0", true));
    }

    #[test]
    fn tabs_ask_for_their_types() {
        assert!(Tab::All.types().starts_with("at_user,at_user_group,at_channel,at_everyone,keyword,message_reaction,thread_v2,dm,mpdm,"));
        assert_eq!(Tab::Dms.types(), "dm,mpdm,bot_dm_bundle");
        assert_eq!(Tab::Mentions.types(), "at_user,at_user_group,at_channel,at_everyone,keyword,unjoined_channel_mention");
        assert_eq!(Tab::Threads.types(), "thread_v2");
        for t in Tab::ALL {
            // `bot_dm` and `thread_reply` fail the whole request.
            assert!(!t.types().split(',').any(|ty| ty == "bot_dm" || ty == "thread_reply"), "{t:?}");
        }
        assert_eq!(ALL_TYPES, Tab::All.types(), "the sidebar's count is the All tab's");
        assert_eq!((Tab::All.next(), Tab::Threads.next(), Tab::All.prev(), Tab::Mentions.prev()), (Tab::Dms, Tab::All, Tab::Threads, Tab::Dms), "round");
    }

    #[test]
    fn the_all_tab_lists_every_type_the_badge_counts() {
        // (The feed took each of them alone and all together on the test workspace.)
        for ty in BADGE_TYPES {
            assert!(Tab::All.lists(ty), "{ty}");
        }
        assert!(Tab::All.lists("message_reaction") && Tab::All.lists("mpdm"), "and what it listed before");
        let types: Vec<&str> = ALL_TYPES.split(',').collect();
        assert_eq!(types.len(), types.iter().collect::<HashSet<_>>().len(), "each once");
    }

    /// An `activity.feed` item of type `ty` with `item` (made up, in the shapes Slack's web client reads).
    fn feed_item(ty: &str, key: &str, feed_ts: &str, unread: bool, mut item: Value) -> Value {
        item["type"] = ty.into();
        serde_json::json!({"feed_ts": feed_ts, "is_unread": unread, "is_archived": false, "key": key, "version": "1", "item": item})
    }

    /// One of each type the badge counts beyond the mentions, threads and DMs (made up, in the web client's shapes).
    fn other_types() -> Vec<Item> {
        let v = serde_json::json!({"items": [
            feed_item("channel", "channel-C1", "20.0", true, serde_json::json!({"bundle_info": {"payload": {"channel_entry": {"latest_message": {"channel": "C1", "ts": "20.0"}, "unread_msg_count": 3}}}})),
            feed_item("bot_dm_bundle", "bot-D9", "19.0", true, serde_json::json!({"bundle_info": {"unread_count": 2, "payload": {"message": {"channel": "D9", "ts": "19.0", "thread_ts": "18.0"}}}})),
            feed_item("internal_channel_invite", "inv-C1", "17.0", true, serde_json::json!({"message": {"author_user_id": "U2", "channel": "C1", "is_broadcast": false, "ts": "17.0"}})),
            feed_item("quietly_added_to_channel", "quiet-C1", "16.0", true, serde_json::json!({"quietly_added_to_channel_payload": {"channel_id": "C1"}})),
            feed_item("unjoined_channel_mention", "unjoined-C7", "15.0", true, serde_json::json!({"unjoined_channel_mention_payload": {"channel_id": "C7", "message_ts": "14.0"}})),
            feed_item("prejoin_dm_welcome_party_alert", "welcome-D1", "13.0", true, serde_json::json!({"prejoin_dm_welcome_party_payload": {"channel_id": "D1", "invitee_user_id": "U3"}})),
            feed_item("external_channel_invite", "ext-1", "12.0", true, serde_json::json!({"invite": {"id": "I1"}})),
            feed_item("list_record_assigned", "list-1", "11.0", true, serde_json::json!({"linked_item_id": "F1", "list_payload": {"list_id": "F1", "row_id": "R1", "assigned_by": "U2"}})),
            feed_item("list_record_edited", "list-2", "10.0", false, serde_json::json!({"linked_item_id": "F1", "list_edit_payload": {"list_id": "F1", "row_id": "R1", "edited_by": "U3"}})),
            feed_item("list_user_mentioned", "list-3", "9.0", false, serde_json::json!({"list_user_mention_payload": {"list_id": "F1", "mentioned_by": "U2"}})),
        ]});
        parse(&v)
    }

    #[test]
    fn the_badges_other_types_are_read() {
        let items = other_types();
        assert_eq!(items.len(), 10, "none dropped");
        let got = |n: usize| (items[n].kind.clone(), items[n].channel.as_str(), items[n].ts.as_str(), items[n].author.as_deref());
        assert_eq!(got(0), (Kind::Channel { unread: 3 }, "C1", "20.0", None), "a channel's new messages: its newest");
        assert_eq!(got(1), (Kind::App { unread: 2, thread: Some("18.0".into()) }, "D9", "19.0", None), "an app's messages: its DM");
        assert_eq!(got(2), (Kind::Invite, "C1", "17.0", Some("U2")), "added to a channel: its join message");
        assert_eq!(got(3), (Kind::Notice("quietly_added_to_channel".into()), "C1", "", None), "no message of its own");
        assert_eq!(got(4), (Kind::Mention("unjoined_channel_mention".into()), "C7", "14.0", None));
        assert_eq!(got(5), (Kind::Notice("prejoin_dm_welcome_party_alert".into()), "D1", "", Some("U3")), "who to welcome");
        assert_eq!(got(6), (Kind::Notice("external_channel_invite".into()), "", "", None), "an invitation to accept: no conversation yet");
        assert_eq!(got(7), (Kind::Notice("list_record_assigned".into()), "", "", Some("U2")), "who assigned it");
        assert_eq!((got(8).3, got(9).3), (Some("U3"), Some("U2")), "who edited, who mentioned");
        assert!(items.iter().all(|it| !it.key.is_empty() && !it.feed_ts.is_empty()));
        // Pushed the same way as listed.
        let pushed = serde_json::json!({"type": "activity", "subtype": "activity_updated", "key": "channel-C1", "entry": feed_item("channel", "channel-C1", "20.0", true, serde_json::json!({"bundle_info": {"payload": {"channel_entry": {"latest_message": {"channel": "C1", "ts": "20.0"}, "unread_msg_count": 3}}}}))});
        assert_eq!(Item::from_event(&pushed).as_ref(), Some(&items[0]));
    }

    #[test]
    fn enter_on_the_badges_other_types() {
        use super::super::applink::Target;
        let mut a = Activity::default();
        a.set_items(Tab::All, other_types(), None);
        let at = |a: &mut Activity, n: usize| {
            a.selected = n;
            a.target()
        };
        let msg = |c: &str, ts: &str, thread: Option<&str>| Some(Target::Message { channel: c.into(), ts: ts.into(), thread_ts: thread.map(String::from) });
        let conv = |c: &str| Some(Target::Conversation { channel: c.into() });
        assert_eq!(at(&mut a, 0), msg("C1", "20.0", None), "the channel at its newest message");
        assert_eq!(at(&mut a, 1), msg("D9", "19.0", Some("18.0")), "the app's DM, in its thread");
        assert_eq!(at(&mut a, 2), msg("C1", "17.0", None), "the channel it adds you to");
        assert_eq!(at(&mut a, 3), conv("C1"), "the channel you were added to");
        assert_eq!(at(&mut a, 4), msg("C7", "14.0", None), "the mention, in a channel you are not in");
        assert_eq!(at(&mut a, 5), conv("D1"));
        assert_eq!((at(&mut a, 6), at(&mut a, 7)), (None, None), "nowhere in klack: an invitation to accept, a list");
    }

    #[test]
    fn the_badges_other_types_are_laid_out() {
        let mut ws = ws();
        ws.conversations.insert("D9".into(), slack_core::types::Conversation { id: "D9".into(), is_im: true, user: Some("U3".into()), ..Default::default() });
        let items = other_types();
        let join = Message { ts: "17.0".into(), user: Some("U1".into()), subtype: Some("channel_join".into()), text: "<@U1> has joined the channel".into(), ..Default::default() };
        let newest = Message { ts: "20.0".into(), user: Some("U2".into()), text: "newest in the channel".into(), ..Default::default() };
        let messages = HashMap::from([(("C1".to_string(), "17.0".to_string()), Some(join)), (("C1".to_string(), "20.0".to_string()), Some(newest))]);
        let (lines, owners) = layout(&ws, &items, &messages, 70, today());
        let text: Vec<String> = lines.iter().map(plain).collect();
        let of = |n: usize| (0..owners.len()).filter(|&r| owners[r] == Some(n)).map(|r| text[r].clone()).collect::<Vec<_>>();
        let last = |n: usize| of(n).last().map(|l| l.trim_start_matches('▌').to_string()).unwrap_or_default();
        assert!(of(0).iter().any(|l| l.contains("newest in the channel")), "{:#?}", of(0));
        assert_eq!(last(0).trim(), "New messages in #c · 3 new", "{text:#?}");
        assert_eq!(last(1).trim(), "App message in bob · 2 new", "{text:#?}");
        assert_eq!(last(2).trim(), "You were added to #c", "{text:#?}");
        assert_eq!(last(3).trim(), "You were added to #c", "{text:#?}");
        assert_eq!(last(4).trim(), "Mention in C7, a channel you are not in", "{text:#?}");
        assert_eq!(last(5).trim(), "Someone new to welcome in ann", "{text:#?}");
        assert_eq!(last(6).trim(), "An invitation to a Slack Connect channel (accept it in Slack)", "{text:#?}");
        assert_eq!(last(7).trim(), "A list item assigned to you (open it in Slack)", "{text:#?}");
        assert_eq!(last(8).trim(), "A list item you follow was edited (open it in Slack)", "{text:#?}");
        assert_eq!(last(9).trim(), "A mention in a list (open it in Slack)", "{text:#?}");
        // Those with no message of their own wait for none; who is known shows.
        for n in [3, 5, 6, 7, 8, 9] {
            assert!(!of(n).iter().any(|l| l.contains("Loading…") || l.contains("not available")), "{n}: {:#?}", of(n));
        }
        assert!(of(7)[0].contains("ann") && of(7)[0].starts_with('▌'), "who assigned it, unread: {:#?}", of(7));
        assert!(of(6)[0].starts_with('▌'), "the unread bar on its only line: {:#?}", of(6));
        // Only the messages there are asked for.
        let a = Activity { items: Some(items), ..Default::default() };
        let wanted = a.wanted();
        assert!(wanted.iter().all(|(c, ts)| !c.is_empty() && !ts.is_empty()), "{wanted:?}");
        assert!(wanted.contains(&("C7".into(), "14.0".into())) && wanted.contains(&("D9".into(), "19.0".into())), "{wanted:?}");
    }

    #[test]
    fn the_badge_counts_what_slacks_own_does() {
        // `client.counts` / `badge_counts_updated` → `activity_v2`: every type listed, most at 0.
        let v = serde_json::json!({"at_channel": 0, "at_everyone": 0, "at_user": 1, "at_user_group": 0, "bot_dm_bundle": 0, "channel": 1, "code_review_requested": 7, "dm": 2,
            "internal_channel_invite": 1, "keyword": 0, "message_reaction": 4, "saved_reminder": 3, "thread_v2": 3, "unjoined_channel_mention": 0, "mpdm": 5});
        assert_eq!(badge_total(&v), Some(8), "reactions, group DMs, reminders and code reviews do not count");
        assert_eq!(badge_total(&serde_json::json!({})), Some(0));
        assert_eq!(badge_total(&Value::Null), None, "not said: not known");
        assert!(counts_in_badge("thread_v2") && counts_in_badge("dm") && !counts_in_badge("message_reaction") && !counts_in_badge("thread_reply"));
    }

    /// A pushed `activity_updated` entry (made up, in the shape seen on the gateway).
    fn pushed(ty: &str, key: &str, feed_ts: &str, unread: bool) -> Value {
        let item = match ty {
            "dm" => serde_json::json!({"type": "dm", "bundle_info": {"payload": {"dm_entry": {"latest_message": {"channel": "D1", "ts": feed_ts}}}}}),
            "thread_v2" => serde_json::json!({"type": "thread_v2", "bundle_info": {"payload": {"thread_entry": {"channel_id": "C1", "latest_ts": feed_ts, "min_unread_ts": feed_ts, "thread_ts": "1.0", "unread_msg_count": 1}}}}),
            "message_reaction" => serde_json::json!({"type": ty, "message": {"channel": "C1", "ts": "1.0"}, "reaction": {"name": "eyes", "user": "U2"}}),
            _ => serde_json::json!({"type": ty, "message": {"author_user_id": "U2", "channel": "C1", "is_broadcast": false, "ts": feed_ts}}),
        };
        serde_json::json!({"type": "activity", "subtype": "activity_updated", "key": key, "event_ts": feed_ts,
            "entry": {"feed_ts": feed_ts, "is_archived": false, "is_bot": false, "is_unread": unread, "key": key, "version": "1", "item": item}})
    }

    #[test]
    fn a_pushed_item_reads_like_a_listed_one() {
        let it = Item::from_event(&pushed("at_user", "at_user-C1-9.5", "9.5", true)).unwrap();
        assert_eq!((it.key.as_str(), &it.kind, it.channel.as_str(), it.ts.as_str(), it.unread), ("at_user-C1-9.5", &Kind::Mention("at_user".into()), "C1", "9.5", true));
        assert_eq!((it.ty.as_str(), it.feed_ts.as_str(), it.when), ("at_user", "9.5", 9.5));
        let listed = parse(&serde_json::json!({"items": [pushed("at_user", "at_user-C1-9.5", "9.5", true)["entry"]]}));
        assert_eq!(listed, vec![it]);
        assert!(Item::from_event(&serde_json::json!({"type": "activity", "subtype": "activity_deleted", "key": "k"})).is_none());
    }

    #[test]
    fn pushed_items_go_into_the_lists_they_belong_to() {
        let entry = |ty: &str, key: &str, ts: &str, unread: bool| Item::from_event(&pushed(ty, key, ts, unread)).unwrap();
        let mut a = Activity::default();
        a.set_items(Tab::All, vec![entry("at_user", "m5", "5.0", false), entry("thread_v2", "t1", "1.0", true)], None);
        a.selected = 1;
        a.set_items(Tab::Mentions, vec![entry("at_user", "m5", "5.0", false)], None);
        a.set_items(Tab::Threads, vec![], None);
        // A new mention: on top of All (the selection stays on its item) and of the Mentions tab's list.
        a.upsert(entry("at_user", "m9", "9.0", true));
        let keys = |items: &Option<Vec<Item>>| items.as_ref().map(|l| l.iter().map(|i| i.key.clone()).collect::<Vec<_>>());
        assert_eq!(keys(&a.items), Some(vec!["m9".into(), "m5".into(), "t1".into()]));
        assert_eq!(a.selected_item().map(|i| i.key.as_str()), Some("t1"));
        a.set_tab(Tab::Mentions);
        assert_eq!(keys(&a.items), Some(vec!["m9".into(), "m5".into()]));
        a.set_tab(Tab::Threads);
        assert_eq!(keys(&a.items), Some(vec![]), "not a thread");
        a.set_tab(Tab::All);
        // Read elsewhere: the same item, read now; a newer reply to a thread moves it up.
        a.upsert(entry("at_user", "m9", "9.0", false));
        a.upsert(entry("thread_v2", "t1", "12.0", true));
        assert_eq!(keys(&a.items), Some(vec!["t1".into(), "m9".into(), "m5".into()]));
        assert_eq!(a.items.as_ref().unwrap().iter().map(|i| i.unread).collect::<Vec<_>>(), [true, false, false]);
        assert_eq!(a.selected_item().map(|i| i.key.as_str()), Some("t1"));
        // Types the feed does not list (the reply that comes with a thread's entry) stay out; a list not loaded yet too.
        a.upsert(entry("thread_reply", "r1", "13.0", true));
        assert_eq!(a.items.as_ref().unwrap().len(), 3);
        a.upsert(entry("dm", "dm-D1", "14.0", true));
        assert_eq!(a.items.as_ref().unwrap()[0].key, "dm-D1");
        a.set_tab(Tab::Dms);
        assert!(a.items.is_none(), "the DMs tab was never loaded: it is loaded when shown");
        a.set_tab(Tab::All);
        // Deleted: out of every list.
        a.remove_key("m9");
        assert_eq!(keys(&a.items), Some(vec!["dm-D1".into(), "t1".into(), "m5".into()]));
        a.set_tab(Tab::Mentions);
        assert_eq!(keys(&a.items), Some(vec!["m5".into()]));
    }

    #[test]
    fn opening_an_unread_item_marks_it_read_on_slack() {
        let it = |ty: &str, unread: bool| Item::from_event(&pushed(ty, "k1", "9.5", unread)).unwrap();
        let args = |i: &Item| mark_read_params(i).map(|p| p.into_iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>());
        assert_eq!(args(&it("at_user", true)), Some(vec!["type=at_user".into(), "feed_ts=9.5".into(), "key=k1".into()]));
        assert_eq!(args(&it("thread_v2", true)), Some(vec!["type=thread_v2".into(), "feed_ts=9.5".into(), "key=k1".into()]));
        assert_eq!(args(&it("dm", true)), Some(vec!["type=dm".into(), "feed_ts=9.5".into(), "key=k1".into(), "ts=9.5".into()]), "a DM: its feed ts as `ts` too");
        assert_eq!(args(&it("message_reaction", true)), None, "reactions are never marked");
        assert_eq!(args(&it("at_user", false)), None, "read already");
        // Invitations and lists: their feed ts as `ts` too, as Slack's client sends.
        let items = other_types();
        let of = |ty: &str| args(items.iter().find(|i| i.ty == ty).unwrap());
        for ty in ["internal_channel_invite", "quietly_added_to_channel", "external_channel_invite", "list_record_assigned"] {
            assert!(of(ty).is_some_and(|a| a.last().is_some_and(|l| l.starts_with("ts="))), "{ty}: {:?}", of(ty));
        }
        assert_eq!(of("channel").map(|a| a.len()), Some(3));
        assert_eq!(of("bot_dm_bundle").map(|a| a.len()), Some(3));
    }

    #[test]
    fn wants_each_message_once() {
        let mut a = Activity::default();
        let it = |ts: &str| item(ts, Kind::Other("x".into()), "C1", ts, None, false, 0.0);
        let mut thread = it("3");
        thread.kind = Kind::Thread { unread: 1, latest: "4".into(), first_unread: "4".into() };
        a.items = Some(vec![it("1"), it("2"), it("1"), thread]);
        assert_eq!(a.wanted().len(), 4, "1, 2, and the thread's newest reply and parent");
        assert!(a.wanted().contains(&("C1".to_string(), "4".to_string())), "a thread: its newest reply");
        a.requested.insert(("C1".into(), "1".into()));
        assert!(!a.wanted().contains(&("C1".to_string(), "1".to_string())));
    }

    #[test]
    fn item_layout() {
        let ws = ws();
        let p = super::super::theme::p();
        let reply = Message { ts: "1729440000.000200".into(), thread_ts: Some("1729440000.000100".into()), user: Some("U2".into()), text: "one\ntwo\nthree\nfour\nfive\nsix".into(), ..Default::default() };
        let mine = Message { ts: "1729430000.000100".into(), user: Some("U1".into()), text: "my message".into(), reactions: vec![slack_core::types::Reaction { name: "x".into(), count: 2, users: vec!["U3".into()] }], ..Default::default() };
        let dm = Message { ts: "1729420000.000100".into(), user: Some("U2".into()), text: "hi there".into(), ..Default::default() };
        let messages = HashMap::from([(("C1".to_string(), reply.ts.clone()), Some(reply.clone())), (("C1".to_string(), mine.ts.clone()), Some(mine.clone())), (("D1".to_string(), dm.ts.clone()), Some(dm.clone()))]);
        let items = [
            item("m", Kind::Mention("at_user".into()), "C1", &reply.ts, Some("U2"), true, reply.time()),
            item("r", Kind::Reaction { name: "x".into(), user: "U3".into() }, "C1", &mine.ts, None, false, 1729431000.0),
            item("d", Kind::Dm { latest_ts: dm.ts.clone() }, "D1", &dm.ts, None, false, dm.time()),
        ];
        let (lines, owners) = layout(&ws, &items, &messages, 60, today());
        assert_eq!(lines.len(), owners.len());
        let text: Vec<String> = lines.iter().map(plain).collect();
        let rows = |n: usize| (0..owners.len()).filter(|&r| owners[r] == Some(n)).collect::<Vec<usize>>();
        // The mention: who, the status emoji and when; three lines of it, then "…"; where.
        let m = rows(0);
        let time = super::super::clock::at(reply.time() as i64).unwrap().format(ws.settings.time_format()).to_string();
        assert!(text[m[0]].contains("ann") && text[m[0]].contains("🌴") && text[m[0]].contains(&time), "{text:#?}");
        assert!(text[m[0]].starts_with('▌'), "unread: the bar");
        assert_eq!(m.len(), 6, "{text:#?}");
        assert!(text[m[1]].contains("one") && text[m[2]].contains("two") && text[m[3]].contains("three"), "{text:#?}");
        assert_eq!(text[m[4]].trim(), "…");
        assert!(text[m[5]].contains("Mention in #c thread"), "{text:#?}");
        assert!(!text.iter().any(|l| l.contains("four")));
        // The reaction: who reacted, "Reacted :x: to:" with the pill, the message reacted to, where.
        let r = rows(1);
        assert!(text[r[0]].contains("bob") && !text[r[0]].starts_with('▌'), "{text:#?}");
        let lead = r.iter().copied().find(|&n| text[n].contains("Reacted")).expect("the reaction line");
        assert!(text[lead].contains("to:"), "{text:#?}");
        let emoji = text::emoji(&ws, "x", None);
        let pill = lines[lead].spans.iter().find(|s| s.content.contains(&emoji)).expect("the pill");
        assert!(pill.style.bg == Some(p.code_bg) || pill.content.starts_with('['), "a pill: {pill:?}");
        assert!(r.iter().any(|&n| text[n].contains("my message")) && r.iter().any(|&n| text[n].contains("Your message in #c")), "{text:#?}");
        // The DM: who, what, "Direct message".
        let d = rows(2);
        assert!(text[d[0]].contains("ann") && d.iter().any(|&n| text[n].contains("hi there")) && text[*d.last().unwrap()].contains("Direct message"), "{text:#?}");
        // A blank row between items, owning nothing.
        assert!(m.last().unwrap() + 1 < r[0] && text[m.last().unwrap() + 1].trim().is_empty() && owners[m.last().unwrap() + 1].is_none());
    }

    #[test]
    fn a_thread_item_shows_its_newest_reply_and_the_others_in_it() {
        let mut ws = ws();
        ws.self_id = "U1".into();
        let parent = Message { ts: "1729440000.000100".into(), thread_ts: Some("1729440000.000100".into()), user: Some("U1".into()), text: "the parent".into(), reply_count: 4, reply_users: vec!["U2".into(), "U3".into(), "U1".into()], ..Default::default() };
        let newest = Message { ts: "1729440000.000500".into(), thread_ts: Some(parent.ts.clone()), user: Some("U2".into()), text: "the newest reply".into(), ..Default::default() };
        let thread = item("t", Kind::Thread { unread: 2, latest: newest.ts.clone(), first_unread: "1729440000.000400".into() }, "C1", &parent.ts, None, true, newest.time());
        let mut a = Activity { items: Some(vec![thread.clone()]), ..Default::default() };
        assert_eq!(a.wanted(), [("C1".to_string(), newest.ts.clone()), ("C1".to_string(), parent.ts.clone())], "its newest reply, and its parent for who is in it");
        a.messages.insert(("C1".into(), newest.ts.clone()), Some(newest.clone()));
        let (lines, _) = layout(&ws, std::slice::from_ref(&thread), &a.messages, 60, today());
        let text: Vec<String> = lines.iter().map(plain).collect();
        let header = text.iter().find(|l| l.contains("ann")).expect("who");
        assert!(!header.contains('+'), "the others are not known yet: {header}");
        assert!(text.iter().any(|l| l.contains("the newest reply")) && text.iter().any(|l| l.contains("Thread in #c · 2 new replies")), "{text:#?}");
        a.messages.insert(("C1".into(), parent.ts.clone()), Some(parent));
        let (lines, _) = layout(&ws, &[thread], &a.messages, 60, today());
        let text: Vec<String> = lines.iter().map(plain).collect();
        assert!(text.iter().any(|l| l.contains("ann +1")), "one more person in it besides you: {text:#?}");
        // Enter: the thread at its first unread reply.
        assert_eq!(a.target(), Some(super::super::applink::Target::Message { channel: "C1".into(), ts: "1729440000.000400".into(), thread_ts: Some("1729440000.000100".into()) }));
    }

    #[test]
    fn missing_message_says_so() {
        let ws = ws();
        let items = [item("m", Kind::Mention("at_user".into()), "C1", "1729440000.000200", Some("U2"), false, 1729440000.0), item("n", Kind::Mention("at_user".into()), "C1", "1729440000.000300", Some("U3"), false, 1729440000.0)];
        // The first one is gone; the second not loaded yet.
        let messages = HashMap::from([(("C1".to_string(), "1729440000.000200".to_string()), None)]);
        let (lines, owners) = layout(&ws, &items, &messages, 60, today());
        let text: Vec<String> = lines.iter().map(plain).collect();
        let gone: Vec<usize> = (0..owners.len()).filter(|&r| owners[r] == Some(0)).collect();
        assert!(text[gone[0]].contains("ann"), "who, still: {text:#?}");
        let at = gone.iter().copied().find(|&n| text[n].contains("message not available")).expect("said so");
        assert_eq!(lines[at].spans.last().and_then(|s| s.style.fg), Some(super::super::theme::p().dim), "dim");
        assert!(text[*gone.last().unwrap()].contains("Mention in #c"), "where, still");
        let waiting: Vec<usize> = (0..owners.len()).filter(|&r| owners[r] == Some(1)).collect();
        assert!(waiting.iter().any(|&n| text[n].contains("Loading…")) && !waiting.iter().any(|&n| text[n].contains("not available")), "{text:#?}");
    }

    #[test]
    fn date_headers_group_items() {
        let ws = ws();
        let it = |key: &str, when: f64| item(key, Kind::Mention("at_user".into()), "C1", key, None, false, when);
        let items = [it("1", noon(0)), it("2", noon(1)), it("3", noon(3)), it("4", noon(3) - 60.0)];
        let (lines, owners) = layout(&ws, &items, &HashMap::new(), 60, today());
        let text: Vec<String> = lines.iter().map(plain).collect();
        let headers: Vec<(usize, String)> = text.iter().enumerate().filter(|(n, l)| owners[*n].is_none() && l.contains('─')).map(|(n, l)| (n, l.trim_matches(|c| c == '─' || c == ' ').to_string())).collect();
        let third = super::super::messages::day_label(&ws, noon(3));
        assert_eq!(headers.iter().map(|h| h.1.as_str()).collect::<Vec<_>>(), ["Today", "Yesterday", third.as_str()], "{text:#?}");
        assert!(headers[0].0 < owners.iter().position(|o| *o == Some(0)).unwrap(), "above its items");
        let row = |n: usize| owners.iter().position(|o| *o == Some(n)).unwrap();
        assert!(headers[1].0 > row(0) && headers[1].0 < row(1) && headers[2].0 < row(2) && row(2) < row(3));
    }

    #[test]
    fn unread_filter() {
        let it = |key: &str, unread: bool| item(key, Kind::Mention("at_user".into()), "C1", key, None, unread, 0.0);
        let mut a = Activity { items: Some(vec![it("1", true), it("2", false), it("3", true), it("4", false)]), selected: 1, ..Default::default() };
        a.toggle_unread();
        assert!(a.unread_only);
        assert_eq!(a.visible(), [0, 2], "the read ones are hidden");
        assert_eq!(a.selected, 2, "the selection moves to a shown item (the next one)");
        a.move_by(1);
        assert_eq!(a.selected, 2, "the last shown");
        a.move_by(-1);
        assert_eq!(a.selected, 0, "past the hidden one");
        a.toggle_unread();
        assert_eq!((a.visible(), a.selected), (vec![0, 1, 2, 3], 0), "all again, the selection where it was");
        // On the last read item with none unread after it: the one before.
        a.selected = 3;
        a.toggle_unread();
        assert_eq!(a.selected, 2);
        // Drawn: only the unread ones.
        let ws = ws();
        let media = super::super::media::Media::with(ratatui_image::picker::Picker::halfblocks());
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| a.draw(f, f.area(), &Here::alone(&ws), true, &media)).unwrap();
        let shown: Vec<usize> = a.rows.iter().flatten().copied().collect::<std::collections::BTreeSet<_>>().into_iter().collect();
        assert_eq!(shown, [0, 2]);
    }

    #[test]
    fn tabs_keep_their_lists() {
        let it = |key: &str| item(key, Kind::Mention("at_user".into()), "C1", key, None, false, 0.0);
        let mut a = Activity { items: Some(vec![it("1"), it("2")]), selected: 1, ..Default::default() };
        a.set_tab(Tab::Dms);
        assert_eq!((a.tab, a.items.is_none()), (Tab::Dms, true), "not loaded yet");
        a.set_items(Tab::Dms, vec![it("d1")], None);
        a.set_items(Tab::Threads, vec![it("t1"), it("t2")], None);
        assert_eq!(a.items.as_ref().unwrap().len(), 1, "another tab's list is kept for it");
        a.set_tab(Tab::All);
        assert_eq!((a.items.as_ref().map(Vec::len), a.selected), (Some(2), 1), "back: its list and selection");
        a.set_tab(Tab::Threads);
        assert_eq!(a.items.as_ref().map(Vec::len), Some(2));
        // A fresh list keeps the selected item.
        a.selected = 1;
        a.set_items(Tab::Threads, vec![it("t0"), it("t1"), it("t2")], None);
        assert_eq!(a.selected_item().map(|i| i.key.as_str()), Some("t2"));
    }

    #[test]
    fn pages_load_as_the_end_comes() {
        let ws = ws();
        let media = super::super::media::Media::with(ratatui_image::picker::Picker::halfblocks());
        let it = |key: &str| item(key, Kind::Mention("at_user".into()), "C1", key, None, false, noon(0));
        let mut a = Activity::default();
        let draw = |a: &mut Activity, h: u16| ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, h)).unwrap().draw(|f| a.draw(f, f.area(), &Here::alone(&ws), true, &media)).map(|_| ()).unwrap();
        a.set_items(Tab::All, (1..=9).map(|n| it(&n.to_string())).collect(), Some("next".into()));
        draw(&mut a, 12);
        assert_eq!(a.next_page(), None, "the end is not on screen");
        a.move_by(8);
        draw(&mut a, 12);
        assert_eq!(a.next_page().as_deref(), Some("next"), "now it is");
        assert_eq!(a.next_page(), None, "not while it loads");
        a.add_page(Tab::All, vec![it("9"), it("10")], None);
        assert_eq!(a.items.as_ref().unwrap().iter().map(|i| i.key.as_str()).collect::<Vec<_>>(), ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"], "those listed already are left out");
        assert_eq!(a.selected, 8, "the selection stays");
        draw(&mut a, 40);
        assert_eq!(a.next_page(), None, "no more");
        // A page nobody asked for (the list was loaded afresh meanwhile) is left out.
        a.add_page(Tab::All, vec![it("x")], Some("again".into()));
        assert_eq!((a.items.as_ref().unwrap().len(), a.cursor.as_deref()), (10, None));
        // Another tab's page goes to its list.
        a.cursor = Some("more".into());
        a.move_by(9);
        draw(&mut a, 12);
        assert!(a.next_page().is_some());
        a.set_tab(Tab::Dms);
        a.add_page(Tab::All, vec![it("11")], None);
        a.set_tab(Tab::All);
        assert_eq!(a.items.as_ref().unwrap().len(), 11);
    }

    #[test]
    fn a_tiny_area_draws_without_panicking() {
        let ws = ws();
        let media = super::super::media::Media::with(ratatui_image::picker::Picker::halfblocks());
        let it = |key: &str| item(key, Kind::Mention("at_user".into()), "C1", key, None, false, noon(0));
        let mut a = Activity::default();
        a.set_items(Tab::All, (1..=3).map(|n| it(&n.to_string())).collect(), Some("next".into()));
        a.loading = true;
        for height in 0..=4 {
            a.scroll = 99;
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, height)).unwrap().draw(|f| a.draw(f, f.area(), &Here::alone(&ws), true, &media)).unwrap();
        }
        // With no room the end is not on screen: no page is asked for.
        a.loading = false;
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 3)).unwrap().draw(|f| a.draw(f, f.area(), &Here::alone(&ws), true, &media)).unwrap();
        assert_eq!(a.next_page(), None);
    }

    #[test]
    fn the_tab_bar_and_clicks() {
        let ws = ws();
        let media = super::super::media::Media::with(ratatui_image::picker::Picker::halfblocks());
        let it = |key: &str| item(key, Kind::Mention("at_user".into()), "C1", key, None, false, noon(0));
        let mut a = Activity { items: Some(vec![it("1"), it("2")]), ..Default::default() };
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| a.draw(f, f.area(), &Here::alone(&ws), true, &media)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let bar = (0..buf.area.width).map(|x| buf[(x, 1)].symbol()).collect::<String>();
        let col = |what: &str| (0..buf.area.width).find(|&x| (x..buf.area.width).map(|x| buf[(x, 1)].symbol()).collect::<String>().starts_with(what)).expect("the tabs");
        let (all, threads) = (col("All"), col("Threads"));
        assert!(bar.contains("DMs") && bar.contains("Mentions") && all < threads, "{bar}");
        assert_eq!(a.tab_at(threads, 1), Some(Tab::Threads));
        assert_eq!(a.tab_at(col("Mentions"), 1), Some(Tab::Mentions));
        assert_eq!(a.tab_at(all, 1), Some(Tab::All));
        assert_eq!(a.tab_at(all, 2), None);
        // The list under it: a click on an item finds it, not the header above it.
        let first = (2..30).find(|&y| a.row_at(5, y) == Some(0)).expect("item 0 drawn");
        assert_eq!(a.row_at(5, first - 1), None, "its date header");
        let second = (first..30).find(|&y| a.row_at(5, y) == Some(1)).expect("item 1 drawn");
        assert!(second > first + 1 && a.row_at(5, second - 1).is_none(), "a gap between them");
        a.hover_at(5, second);
        assert_eq!((a.hover, a.selected), (Some(1), 0));
        // Narrow: the bar is cut, not wrapped.
        let mut narrow = ratatui::Terminal::new(ratatui::backend::TestBackend::new(20, 10)).unwrap();
        narrow.draw(|f| a.draw(f, f.area(), &Here::alone(&ws), true, &media)).unwrap();
        let buf = narrow.backend().buffer();
        assert!((0..20).map(|x| buf[(x, 1)].symbol()).collect::<String>().contains("All"));
    }

    #[test]
    fn the_list_is_built_again_only_when_something_in_it_changes() {
        let mut ws = ws();
        let media = super::super::media::Media::with(ratatui_image::picker::Picker::halfblocks());
        let it = |key: &str, unread: bool| item(key, Kind::Mention("at_user".into()), "C1", key, Some("U2"), unread, noon(0) + key.parse::<f64>().unwrap());
        let mut a = Activity::default();
        a.set_items(Tab::All, vec![it("2.0", true), it("1.0", false)], None);
        let draw = |a: &mut Activity, ws: &Workspace, width: u16| {
            let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 30)).unwrap();
            terminal.draw(|f| a.draw(f, f.area(), &Here::alone(ws), true, &media)).unwrap();
            let buf = terminal.backend().buffer().clone();
            (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
        };
        draw(&mut a, &ws, 80);
        assert_eq!(a.builds, 1);
        // Frames with nothing new in the list — the pointer or the selection moving, the clock — are not built again.
        a.hover_at(5, 4);
        draw(&mut a, &ws, 80);
        a.move_by(1);
        draw(&mut a, &ws, 80);
        let screen = draw(&mut a, &ws, 80);
        assert_eq!(a.builds, 1, "{screen}");
        assert!(screen.contains("Loading…"));
        // A message, an item, the filter, a person, the settings, the width, the tab: built again, once each.
        a.add_message(("C1".into(), "2.0".into()), Some(Message { ts: "2.0".into(), user: Some("U2".into()), text: "the message itself".into(), ..Default::default() }));
        assert!(draw(&mut a, &ws, 80).contains("the message itself"));
        a.upsert(it("3.0", true));
        assert!(draw(&mut a, &ws, 80).contains("Mention in #c"));
        a.toggle_unread();
        draw(&mut a, &ws, 80);
        assert_eq!(a.builds, 4);
        crate::tui::live::apply(&mut ws, &serde_json::json!({"type": "user_profile_changed", "user": {"id": "U2", "profile": {"display_name": "annie"}}}), None);
        assert!(draw(&mut a, &ws, 80).contains("annie"), "a new name shows");
        ws.settings.messages.avatars = !ws.settings.messages.avatars;
        draw(&mut a, &ws, 80);
        draw(&mut a, &ws, 100);
        a.remove_key("3.0");
        draw(&mut a, &ws, 100);
        assert_eq!(a.builds, 8);
        a.set_tab(Tab::Dms);
        draw(&mut a, &ws, 100);
        a.set_tab(Tab::All);
        draw(&mut a, &ws, 100);
        draw(&mut a, &ws, 100);
        assert_eq!(a.builds, 9, "its list again (none for the tab not loaded)");
        let mut seen = super::super::threads::Seen::new();
        seen.insert("2.0".into(), noon(0) + 10.0);
        a.apply_seen(&seen);
        draw(&mut a, &ws, 100);
        assert_eq!(a.builds, 10, "an item read");
    }

    #[test]
    fn a_conversation_joined_again_under_a_new_name_shows_it() {
        let mut ws = ws();
        let media = super::super::media::Media::with(ratatui_image::picker::Picker::halfblocks());
        let mut a = Activity::default();
        a.set_items(Tab::All, vec![item("1.0", Kind::Mention("at_user".into()), "C1", "1.0", Some("U2"), false, noon(0))], None);
        let draw = |a: &mut Activity, ws: &Workspace| {
            let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 20)).unwrap();
            terminal.draw(|f| a.draw(f, f.area(), &Here::alone(ws), true, &media)).unwrap();
            let buf = terminal.backend().buffer().clone();
            (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
        };
        assert!(draw(&mut a, &ws).contains("Mention in #c"));
        // The same conversation again (as many as before), under another name.
        crate::tui::live::apply(&mut ws, &serde_json::json!({"type": "channel_joined", "channel": {"id": "C1", "name": "renamed", "is_channel": true}}), None);
        assert!(draw(&mut a, &ws).contains("Mention in #renamed"));
    }

    #[test]
    fn a_note_in_place_of_the_list_keeps_it_built() {
        let ws = ws();
        let media = super::super::media::Media::with(ratatui_image::picker::Picker::halfblocks());
        let mut a = Activity::default();
        a.set_items(Tab::All, vec![item("1.0", Kind::Mention("at_user".into()), "C1", "1.0", Some("U2"), false, noon(0))], None);
        a.toggle_unread();
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 20)).unwrap();
        for _ in 0..3 {
            terminal.draw(|f| a.draw(f, f.area(), &Here::alone(&ws), true, &media)).unwrap();
        }
        let buf = terminal.backend().buffer();
        assert!((0..buf.area.height).any(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>().contains("Nothing unread here.")));
        assert_eq!(a.builds, 1, "\"Nothing unread\" frames do not build it again");
    }

    #[test]
    fn the_filter_loads_pages_until_one_has_unread_items() {
        let ws = ws();
        let media = super::super::media::Media::with(ratatui_image::picker::Picker::halfblocks());
        let it = |key: &str, unread: bool| item(key, Kind::Mention("at_user".into()), "C1", key, Some("U2"), unread, noon(0));
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 20)).unwrap();
        let mut draw = |a: &mut Activity| {
            terminal.draw(|f| a.draw(f, f.area(), &Here::alone(&ws), true, &media)).unwrap();
            let buf = terminal.backend().buffer();
            (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
        };
        let mut a = Activity::default();
        a.toggle_unread();
        // The first page all read (the one unread there was opened), unread ones further down.
        a.set_items(Tab::All, (1..=50).map(|n| it(&n.to_string(), false)).collect(), Some("next".into()));
        let screen = draw(&mut a);
        assert!(!screen.contains(NONE_UNREAD), "more to load: not \"nothing unread\" yet\n{screen}");
        assert_eq!(a.next_page().as_deref(), Some("next"), "the next page is asked for");
        assert!(draw(&mut a).contains(LOADING_MORE));
        a.add_page(Tab::All, vec![it("51", false), it("52", true)], Some("last".into()));
        draw(&mut a);
        assert_eq!((a.visible(), a.selected), (vec![51], 51), "the unread one on the next page is shown, and selected");
        assert_eq!(a.next_page().as_deref(), Some("last"), "and the list goes on loading (its end is on screen)");
        a.add_page(Tab::All, vec![it("53", false)], None);
        draw(&mut a);
        assert_eq!(a.next_page(), None, "all loaded");
        // All loaded, none unread: said so.
        a.items.as_mut().unwrap()[51].unread = false;
        a.dirty = true;
        assert!(draw(&mut a).contains(NONE_UNREAD));
        assert_eq!(a.next_page(), None);
    }

    #[test]
    fn a_page_that_failed_is_said_and_asked_for_again_only_when_you_act() {
        let ws = ws();
        let media = super::super::media::Media::with(ratatui_image::picker::Picker::halfblocks());
        let it = |key: &str, unread: bool| item(key, Kind::Mention("at_user".into()), "C1", key, Some("U2"), unread, noon(0));
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 20)).unwrap();
        let mut draw = |a: &mut Activity| {
            terminal.draw(|f| a.draw(f, f.area(), &Here::alone(&ws), true, &media)).unwrap();
            let buf = terminal.backend().buffer();
            (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
        };
        let mut a = Activity::default();
        a.toggle_unread();
        a.set_items(Tab::All, (1..=50).map(|n| it(&n.to_string(), false)).collect(), Some("next".into()));
        draw(&mut a);
        assert!(a.next_page().is_some());
        a.fail(Tab::All, true, "ratelimited".into());
        // Said in place of "Loading more…", and not asked for again by itself.
        for _ in 0..3 {
            let screen = draw(&mut a);
            assert!(screen.contains(FAILED) && screen.contains("ratelimited") && !screen.contains(LOADING_MORE), "{screen}");
            assert_eq!(a.next_page(), None, "not asked for again by itself");
        }
        // You act (a key, the wheel): asked for again.
        a.move_by(1);
        draw(&mut a);
        assert_eq!(a.next_page().as_deref(), Some("next"));
        // Under a list shown: said at its end, and the filter toggled asks again.
        a.toggle_unread();
        a.move_by(60);
        a.fail(Tab::All, true, "ratelimited".into());
        let screen = draw(&mut a);
        assert!(screen.contains(FAILED) && !screen.contains(LOADING_MORE), "{screen}");
        assert_eq!(a.next_page(), None);
        a.toggle_unread();
        draw(&mut a);
        assert!(a.next_page().is_some(), "the filter toggled: asked for again");
        // Another tab shown and back: asked for again (the first page loads afresh too).
        a.fail(Tab::All, true, "ratelimited".into());
        a.set_tab(Tab::Dms);
        a.set_tab(Tab::All);
        draw(&mut a);
        assert!(a.next_page().is_some());
        // The first page failing again with a list shown is not a page's failure.
        a.fail(Tab::All, false, "gone".into());
        assert!(!draw(&mut a).contains("gone"));
    }

    #[test]
    fn the_selection_fills_the_whole_row() {
        let it = |ts: &str| item(ts, Kind::Other("x".into()), "C1", ts, None, false, 0.0);
        let mut a = Activity { items: Some(vec![it("1.0"), it("2.0")]), ..Default::default() };
        let ws = Workspace::default();
        let media = super::super::media::Media::with(ratatui_image::picker::Picker::halfblocks());
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| a.draw(f, f.area(), &Here::alone(&ws), true, &media)).unwrap();
        // Inside the view's frame, under the tab bar.
        assert_eq!(a.list, Rect::new(1, 2, 98, 27));
        let buf = terminal.backend().buffer().clone();
        let p = super::super::theme::p();
        let (x0, x1) = (a.list.x, a.list.right() - 1);
        let selected: Vec<u16> = (a.list.y..a.list.bottom()).filter(|&y| a.row_at(5, y) == Some(0)).collect();
        assert!(!selected.is_empty());
        for y in a.list.y..a.list.bottom() {
            let lit = (buf[(x0, y)].bg, buf[(x1, y)].bg) == (p.selection, p.selection);
            assert_eq!(lit, selected.contains(&y), "row {y}");
        }
        assert_ne!(buf[(x1 + 1, selected[0])].bg, p.selection, "not the border");
    }
}

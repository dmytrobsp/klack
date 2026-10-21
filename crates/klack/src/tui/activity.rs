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

use super::applink::{AppLink, Here};
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
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub key: String,
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
        let (kind, channel, ts) = match ty {
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
            kind,
            channel: channel.to_string(),
            ts: ts.to_string(),
            author: it["message"]["author_user_id"].as_str().map(String::from),
            unread: v["is_unread"].as_bool().unwrap_or(false),
            when: v["feed_ts"].as_str().and_then(|t| t.parse().ok()).unwrap_or(0.0),
        })
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
            Tab::All => "at_user,at_user_group,at_channel,at_everyone,keyword,message_reaction,thread_v2,dm,mpdm",
            Tab::Dms => "dm,mpdm,bot_dm_bundle",
            Tab::Mentions => "at_user,at_user_group,at_channel,at_everyone,keyword",
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
}

/// The feed's item types on the All tab (`activity.feed`; the sidebar row's count).
pub const ALL_TYPES: &str = "at_user,at_user_group,at_channel,at_everyone,keyword,message_reaction,thread_v2,dm,mpdm";

/// How many of `items` are unread (the sidebar's Activity row).
pub fn unread_count(items: &[Item]) -> usize {
    items.iter().filter(|it| it.unread).count()
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
const UNREAD_ONLY: &str = "unread only";
const EMPTY: &str = "Nothing here yet: mentions, reactions to your messages, thread replies and direct messages show up here.";
const NONE_UNREAD: &str = "Nothing unread here.";
const FAILED: &str = "The activity could not be loaded";

/// The view's words, for the locale template.
pub const WORDS: &[&str] = &[
    TODAY, YESTERDAY, MISSING, LOADING, LOADING_MORE, MENTION, GROUP_MENTION, CHANNEL_MENTION, EVERYONE_MENTION, KEYWORD, IN_THREAD, THREAD, NEW_REPLY, NEW_REPLIES, DIRECT, GROUP_DM, REACTED, YOUR_MESSAGE, OTHER, UNREAD_ONLY, EMPTY, NONE_UNREAD, FAILED, "All", "DMs", "Mentions", "Threads",
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
    /// Only the unread items are shown.
    pub unread_only: bool,
    /// Where the next page of the list starts (none: it is all there), and whether it is being loaded.
    pub cursor: Option<String>,
    pub loading: bool,
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
}

impl Activity {
    /// The items shown (all, or the unread ones), as indices into `items`.
    pub fn visible(&self) -> Vec<usize> {
        let Some(items) = &self.items else { return Vec::new() };
        (0..items.len()).filter(|&n| !self.unread_only || items[n].unread).collect()
    }

    /// Move the selection by `delta` shown items.
    pub fn move_by(&mut self, delta: isize) {
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
        self.unread_only = !self.unread_only;
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
        let here = Kept { items: self.items.take(), selected: self.selected, scroll: self.scroll, cursor: self.cursor.take(), loading: self.loading };
        self.kept.insert(self.tab, here);
        let there = self.kept.remove(&tab).unwrap_or_default();
        (self.tab, self.items, self.selected, self.scroll, self.cursor, self.loading) = (tab, there.items, there.selected, there.scroll, there.cursor, there.loading);
        (self.failed, self.hover, self.at_end) = (None, None, false);
        self.keep_selection_shown();
    }

    /// Tab `tab`'s first page came (`cursor`: where the next one starts): shown if it is the tab shown (the selected
    /// item stays selected), else kept for it.
    pub fn set_items(&mut self, tab: Tab, items: Vec<Item>, cursor: Option<String>) {
        if tab != self.tab {
            let kept = self.kept.entry(tab).or_default();
            (kept.items, kept.cursor, kept.loading) = (Some(items), cursor, false);
            return;
        }
        let key = self.selected_item().map(|it| it.key.clone());
        self.selected = key.and_then(|k| items.iter().position(|it| it.key == k)).unwrap_or(0);
        (self.items, self.failed, self.cursor, self.loading) = (Some(items), None, cursor, false);
        self.keep_selection_shown();
    }

    /// The next page of tab `tab` came: its items not listed yet go at the end (none asked for — the list was loaded
    /// afresh meanwhile —: left out).
    pub fn add_page(&mut self, tab: Tab, page: Vec<Item>, cursor: Option<String>) {
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
        let cursor = self.cursor.clone().filter(|_| self.at_end && !self.loading && self.items.is_some())?;
        self.loading = true;
        Some(cursor)
    }

    /// Tab `tab`'s list could not be loaded: said there when it has none to show.
    pub fn fail(&mut self, tab: Tab, error: String) {
        if tab == self.tab && self.items.is_none() {
            self.failed = Some(error);
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

    /// Where Enter on the selected item goes: (channel, message ts, thread parent) — a thread at its first unread
    /// reply (none unread: its newest), anything else at its message (a reply: in its thread).
    pub fn target(&self) -> Option<(String, String, Option<String>)> {
        let it = self.selected_item()?;
        match &it.kind {
            Kind::Thread { unread, latest, first_unread } => {
                let at = if *unread > 0 && !first_unread.is_empty() { first_unread } else if !latest.is_empty() { latest } else { &it.ts };
                Some((it.channel.clone(), at.clone(), Some(it.ts.clone())))
            }
            _ => {
                let parent = self.messages.get(&it.message_key()).and_then(|m| m.as_ref()).filter(|m| m.is_reply()).and_then(|m| m.thread_ts.clone());
                Some((it.channel.clone(), it.ts.clone(), parent))
            }
        }
    }

    /// The messages of the items on screen and around them (and the selection) not asked for yet.
    pub fn wanted(&self) -> Vec<(String, String)> {
        let Some(items) = &self.items else { return vec![] };
        let shown = self.visible();
        let positions: Vec<usize> = self.shown.iter().chain(std::iter::once(&self.selected)).filter_map(|n| shown.iter().position(|s| s == n)).collect();
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
        let shown = self.visible();
        let note = match &self.items {
            None if self.failed.is_some() => Some(Line::styled(format!("{}: {}", tr(FAILED), self.failed.as_deref().unwrap_or_default()), Style::default().fg(p.danger))),
            None => Some(Line::styled(tr(LOADING).into_owned(), Style::default().fg(p.dim))),
            Some(items) if shown.is_empty() => Some(Line::styled(tr(if items.is_empty() { EMPTY } else { NONE_UNREAD }).into_owned(), Style::default().fg(p.dim))),
            Some(_) => None,
        };
        if let Some(note) = note {
            f.render_widget(Paragraph::new(note).wrap(ratatui::widgets::Wrap { trim: true }), list);
            return;
        }
        let items: Vec<Item> = self.items.as_deref().unwrap_or_default().iter().enumerate().filter(|(n, _)| shown.contains(n)).map(|(_, it)| it.clone()).collect();
        let avatars = if !ws.settings.messages.avatars {
            Avatars::Off
        } else if media.avatars {
            Avatars::Pictures
        } else {
            Avatars::Initials
        };
        let built = build(here, &items, &self.messages, list.width, super::clock::now().date_naive(), avatars);
        let owners: Vec<Option<usize>> = built.owners.iter().map(|o| o.map(|n| shown[n])).collect();
        let height = list.height as usize;
        // Bring a selection moved by the keys into view (with its date header, when it is the first of its day).
        if std::mem::take(&mut self.follow) {
            if let (Some(first), Some(last)) = (owners.iter().position(|o| *o == Some(self.selected)), owners.iter().rposition(|o| *o == Some(self.selected))) {
                let first = if built.headers.contains(&first.saturating_sub(1)) { first - 1 } else { first };
                self.scroll = super::threads::bring_into_view(self.scroll, height, first, last);
            }
        }
        let more = usize::from(self.loading);
        let total = built.lines.len() + more;
        self.scroll = self.scroll.min(total.saturating_sub(height));
        let start = self.scroll;
        let end = (start + height).min(built.lines.len());
        let mut lines: Vec<Line<'static>> = built.lines[start..end].to_vec();
        let mut rows: Vec<Option<usize>> = owners[start..end].to_vec();
        if self.loading && lines.len() < height {
            lines.push(Line::styled(tr(LOADING_MORE).into_owned(), Style::default().fg(p.dim)));
            rows.push(None);
        }
        // (The last item's rows, or the selection on the last item: time for the next page.)
        self.at_end = end == built.lines.len() || shown.last() == Some(&self.selected);
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
        for spot in messages::link_spots(here, &built.messages, &built.lines, &built.message_rows).iter().filter(|s| (start..end).contains(&s.line)) {
            let (x, y) = (list.x + spot.col, list.y + (spot.line - start) as u16);
            let width = spot.width.min(list.right().saturating_sub(x));
            messages::hyperlink(f.buffer_mut(), x, y, width, &spot.url);
            self.links.push((Rect { x, y, width, height: 1 }, spot.url.clone(), spot.app.clone(), spot.wants_author));
        }
        super::scroll::indicator_beside(f, area, list, start, end - start, total, border);
        self.pictures = messages::draw_pictures(f, list, slots, start, 0, media);
        for n in rows.iter().flatten() {
            if !self.shown.contains(n) {
                self.shown.push(*n);
            }
        }
        self.rows = rows;
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
    let built = build(&Here::alone(ws), items, messages, width, today, Avatars::Off);
    (built.lines, built.owners)
}

/// [`layout`] with the avatars, the custom emoji pictures and the links (`today`: the day "Today" is).
pub fn build(here: &Here, items: &[Item], messages: &HashMap<(String, String), Option<Message>>, width: u16, today: NaiveDate, avatars: Avatars) -> Built {
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
        (None, None) => it.author.clone().map(|u| Message { user: Some(u), ts: it.message_key().1, ..Default::default() }),
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
    if message.is_none() {
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
        Kind::Other(t) => tr(OTHER).replace("{what}", t).replace("{where}", &in_thread),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::applink::Here;

    fn item(key: &str, kind: Kind, channel: &str, ts: &str, author: Option<&str>, unread: bool, when: f64) -> Item {
        Item { key: key.into(), kind, channel: channel.into(), ts: ts.into(), author: author.map(String::from), unread, when }
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
        assert_eq!(Tab::All.types(), "at_user,at_user_group,at_channel,at_everyone,keyword,message_reaction,thread_v2,dm,mpdm");
        assert_eq!(Tab::Dms.types(), "dm,mpdm,bot_dm_bundle");
        assert_eq!(Tab::Mentions.types(), "at_user,at_user_group,at_channel,at_everyone,keyword");
        assert_eq!(Tab::Threads.types(), "thread_v2");
        for t in Tab::ALL {
            // `bot_dm` fails the whole request.
            assert!(!t.types().contains("bot_dm,") && !t.types().ends_with("bot_dm"), "{t:?}");
        }
        assert_eq!(ALL_TYPES, Tab::All.types(), "the sidebar's count is the All tab's");
        assert_eq!((Tab::All.next(), Tab::Threads.next(), Tab::All.prev(), Tab::Mentions.prev()), (Tab::Dms, Tab::All, Tab::Threads, Tab::Dms), "round");
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
        assert_eq!(a.target(), Some(("C1".into(), "1729440000.000400".into(), Some("1729440000.000100".into()))));
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

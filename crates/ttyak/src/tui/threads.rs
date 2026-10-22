//! Threads (Ctrl+T), a view in the main pane: every thread you are in, from
//! `subscriptions.thread.getView` — unread ones first, then by latest reply — as a feed of blocks
//! with the messages in full (`feed`); the selection moves message by message, Enter opens the
//! thread at the selected one, the composer under the feed replies in its thread. More are loaded
//! as the end of the feed comes on screen.

use std::ops::Range;
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use serde_json::Value;
use slack_core::types::{File, Message};

use super::applink::{AppLink, Here};
use super::feed::{Feed, ReadClock};
use super::media::Media;
use super::messages::{Avatars, ImageSlot, LinkSpot, PillSpot};

#[derive(Debug, Clone, Default)]
pub struct Entry {
    pub channel: String,
    pub root: Message,
    /// Replies not read yet (oldest first).
    pub unread: Vec<Message>,
    /// The newest replies, when all are read.
    pub latest: Vec<Message>,
    /// Marked read here (on screen in the feed, or replied to): no longer shown as unread, though
    /// its "new" rule stays until the view loads again (as in a conversation).
    pub read: bool,
}

impl Entry {
    fn from_json(v: &Value) -> Option<Entry> {
        let r = &v["root_msg"];
        // A parent of a shape ttyak does not know keeps its thread, shown as not available.
        let root: Message = serde_json::from_value(r.clone()).unwrap_or_else(|e| {
            tracing::debug!("a thread's parent ttyak cannot read: {e}");
            let ts = r["ts"].as_str().or(r["thread_ts"].as_str()).unwrap_or_default().to_string();
            Message { thread_ts: Some(ts.clone()), ts, reply_count: r["reply_count"].as_u64().unwrap_or(0), latest_reply: r["latest_reply"].as_str().map(String::from), ..Default::default() }
        });
        if root.ts.is_empty() {
            return None;
        }
        let replies = |k: &str| v[k].as_array().into_iter().flatten().filter_map(|m| serde_json::from_value(m.clone()).ok()).collect::<Vec<Message>>();
        Some(Entry { channel: v["root_msg"]["channel"].as_str()?.to_string(), root, unread: replies("unread_replies"), latest: replies("latest_replies"), read: false })
    }

    /// When the thread last had a reply.
    pub fn latest_reply(&self) -> f64 {
        self.root.latest_reply.as_deref().and_then(|t| t.parse().ok()).unwrap_or_else(|| self.root.time())
    }
}

/// What was seen in this run: thread keys (`channel/ts`, and activity item keys) → when it was
/// opened (epoch seconds). Slack learns of it a moment later, so lists loaded meanwhile are
/// corrected with it.
pub type Seen = std::collections::HashMap<String, f64>;

/// Take out of `page` the unread replies of threads opened since they were posted; a thread
/// left without any goes after those that still have some (in the order Slack gave).
pub fn apply_seen(page: &mut Vec<Entry>, seen: &Seen) {
    for e in page.iter_mut() {
        let Some(&at) = seen.get(&super::state::thread_key(&e.channel, &e.root.ts)) else { continue };
        let (old, new): (Vec<Message>, Vec<Message>) = std::mem::take(&mut e.unread).into_iter().partition(|r| r.time() <= at);
        e.unread = new;
        e.latest.extend(old);
    }
    page.sort_by_key(|e| e.unread.is_empty());
}

/// A page of the view: its threads and whether there are more.
pub fn parse(v: &Value) -> (Vec<Entry>, bool) {
    (v["threads"].as_array().into_iter().flatten().filter_map(Entry::from_json).collect(), v["has_more"].as_bool().unwrap_or(false))
}

/// The feed is built again at most this often for what it looks like (names, statuses and
/// pictures that changed meanwhile show then); a change to its threads or its width shows at once.
const REBUILD: Duration = Duration::from_secs(1);

const LOADING: &str = "Loading…";
const LOADING_MORE: &str = "Loading more…";
const EMPTY: &str = "No threads yet: the threads you start, reply in or follow show up here.";
const FAILED: &str = "The threads could not be loaded";

/// The view's words, for `ctl locale-template`.
pub const WORDS: &[&str] = &[LOADING, LOADING_MORE, EMPTY, FAILED];

#[derive(Default)]
pub struct Threads {
    pub entries: Option<Vec<Entry>>,
    pub has_more: bool,
    /// How many threads on top had new replies when loaded ("You're up to date" goes under them; it stays there).
    unread_first: usize,
    /// A page is being loaded.
    pub loading: bool,
    /// Why the first page could not be loaded (said in the view).
    pub failed: Option<String>,
    /// The selected message (channel, ts); none: the first one.
    pub selected: Option<(String, String)>,
    /// The message (channel, ts, its thread's parent's ts) a step back or forward lands on: selected once its page is
    /// in (see [`Threads::want_more`]).
    pub want: Option<(String, String, String)>,
    /// Pages asked for to find `want`.
    want_pages: usize,
    /// Lines of the feed above the view.
    pub scroll: usize,
    /// Bring the selection into view at the next draw (it moved by the keys, or the view was drawn at another size: a
    /// re-wrap is not a scroll).
    follow: bool,
    /// The size (columns, rows) of the list at the last draw.
    size: (u16, u16),
    /// Scrolled by you (the wheel) since the last draw: only then does the selection follow the view.
    scrolled: bool,
    /// The selection and the row its first line was on at the last draw (above the view: negative).
    spot: Option<((String, String), isize)>,
    /// Last drawn area (empty while the view is not shown), for the mouse.
    pub area: Rect,
    /// The feed's rows at the last draw.
    pub list: Rect,
    /// The message (channel, ts) of each row on screen at the last draw, and the thread (channel, its parent's ts)
    /// each row belongs to.
    rows: Vec<Option<(String, String)>>,
    row_threads: Vec<Option<(String, String)>>,
    /// The message the pointer is over (not the selection), at the last `Moved`.
    pub hover: Option<(String, String)>,
    /// The feed as last built, its links, and how it looked then (its width, people, …) and when.
    feed: Feed,
    feed_links: Vec<LinkSpot>,
    feed_pills: Vec<PillSpot>,
    built: Option<(super::messages::Look, Instant)>,
    /// How many times the feed was built.
    pub builds: u64,
    /// The threads changed since: build the feed again at the next draw.
    dirty: bool,
    /// The messages in feed order (for ↑ / ↓).
    order: Vec<(String, String)>,
    /// The feed's lines on screen at the last draw (empty while not drawn).
    pub visible: Range<usize>,
    /// When the blocks with unread replies have been on screen long enough.
    pub clock: ReadClock,
    /// Threads whose newest replies were asked for (since the last load or their last new reply).
    fetched: std::collections::HashSet<(String, String)>,
    /// Where links were drawn at the last draw: as in the message panes.
    links: Vec<(Rect, String, Option<AppLink>, bool)>,
    /// Where reaction pills were drawn at the last draw: the message (channel, ts) and the reaction.
    pills: Vec<(Rect, (String, String), String)>,
    /// Pictures on screen but not loaded at the last draw.
    pub wanted: Vec<(File, (u16, u16))>,
    /// Replies deleted here (`delete_here`), to put back if Slack refuses.
    deleted: std::collections::HashSet<(String, String)>,
}

impl Threads {
    /// Add a page, leaving out threads already listed.
    pub fn add_page(&mut self, page: Vec<Entry>, has_more: bool) {
        if self.entries.is_none() {
            self.unread_first = 0;
        }
        let entries = self.entries.get_or_insert_with(Vec::new);
        // Counted on only while every thread so far had new replies.
        let mut counting = self.unread_first == entries.len();
        for e in page {
            if !entries.iter().any(|o| o.channel == e.channel && o.root.ts == e.root.ts) {
                counting &= !e.unread.is_empty();
                self.unread_first += counting as usize;
                entries.push(e);
            }
        }
        self.has_more = has_more;
        self.loading = false;
        self.dirty = true;
    }

    /// A page is in while a step lands on `want`: listed (selected at the next draw), or only its thread (its parent
    /// then, the reply gone from the block); else true when the next page is to be asked for (up to
    /// [`AUTO_PAGES`](super::activity::AUTO_PAGES)), and given up otherwise (the first message).
    pub fn want_more(&mut self) -> bool {
        let Some((channel, ts, parent)) = self.want.clone() else { return false };
        if self.thread_of(&channel, &ts).is_some() {
            return false;
        }
        if self.thread_of(&channel, &parent).is_some() {
            self.want = Some((channel, parent.clone(), parent));
            return false;
        }
        if self.has_more && self.want_pages < super::activity::AUTO_PAGES {
            self.want_pages += 1;
            return true;
        }
        self.want = None;
        false
    }

    /// Where the next page starts: before the oldest latest reply listed.
    pub fn next_page(&self) -> Option<String> {
        let oldest = self.entries.as_ref()?.iter().map(Entry::latest_reply).fold(f64::INFINITY, f64::min);
        (self.has_more && !self.loading && oldest.is_finite()).then(|| format!("{oldest:.6}"))
    }

    /// Move the selection by `delta` messages through the feed.
    pub fn move_by(&mut self, delta: isize) {
        if self.order.is_empty() {
            return;
        }
        let at = self.selected.as_ref().and_then(|s| self.order.iter().position(|o| o == s)).unwrap_or(0) as isize;
        self.selected = Some(self.order[(at + delta).clamp(0, self.order.len() as isize - 1) as usize].clone());
        (self.follow, self.want) = (true, None);
    }

    /// Messages a page moves (those on screen at the last draw).
    pub fn page(&self) -> isize {
        let mut shown: Vec<&(String, String)> = self.rows.iter().flatten().collect();
        shown.dedup();
        shown.len().max(1) as isize
    }

    /// Scroll the feed by `delta` lines (the wheel); a selection scrolled off goes onto the screen at the next draw.
    pub fn scroll_by(&mut self, delta: isize) {
        self.scroll = self.scroll.saturating_add_signed(delta);
        self.scrolled = true;
    }

    /// The message drawn at a terminal position.
    pub fn message_at(&self, column: u16, row: u16) -> Option<(String, String)> {
        self.list.contains((column, row).into()).then(|| self.rows.get((row - self.list.y) as usize).cloned().flatten()).flatten()
    }

    /// The thread (channel, its parent's ts) whose header or "more replies" row is drawn at a terminal position.
    pub fn thread_at(&self, column: u16, row: u16) -> Option<(String, String)> {
        if !self.list.contains((column, row).into()) || self.message_at(column, row).is_some() {
            return None;
        }
        self.row_threads.get((row - self.list.y) as usize).cloned().flatten()
    }

    /// The pointer moved to a position: hover the message there (none off the feed).
    pub fn hover_at(&mut self, column: u16, row: u16) {
        self.hover = self.message_at(column, row);
    }

    /// The end of the feed was on screen at the last draw (time to load more).
    pub fn at_end(&self) -> bool {
        self.entries.is_some() && !self.visible.is_empty() && self.visible.end + self.list.height as usize >= self.feed.lines.len()
    }

    /// The thread (channel, its parent's ts) of the selected message.
    pub fn selected_thread(&self) -> Option<(String, String)> {
        let (channel, ts) = self.selected.as_ref().or(self.order.first())?;
        self.thread_of(channel, ts)
    }

    /// The thread (channel, its parent's ts) message `ts` of `channel` is in, in the feed.
    pub fn thread_of(&self, channel: &str, ts: &str) -> Option<(String, String)> {
        let e = self.entries.as_ref()?.iter().find(|e| e.channel == channel && (e.root.ts == ts || e.unread.iter().chain(&e.latest).any(|r| r.ts == ts)))?;
        Some((e.channel.clone(), e.root.ts.clone()))
    }

    /// Where Enter goes: (channel, the selected message, its thread's parent).
    pub fn target(&self) -> Option<(String, String, Option<String>)> {
        let (_, ts) = self.selected.as_ref().or(self.order.first())?.clone();
        let (channel, parent) = self.selected_thread()?;
        Some((channel, ts, Some(parent)))
    }

    /// The thread of `channel` with parent `thread_ts`.
    fn entry_mut(&mut self, channel: &str, thread_ts: &str) -> Option<&mut Entry> {
        self.entries.as_mut()?.iter_mut().find(|e| e.channel == channel && e.root.ts == thread_ts)
    }

    /// The newest reply known in a thread (what marking it read goes up to).
    pub fn newest_reply(&self, channel: &str, thread_ts: &str) -> Option<String> {
        let e = self.entries.as_ref()?.iter().find(|e| e.channel == channel && e.root.ts == thread_ts)?;
        e.unread.iter().chain(&e.latest).max_by(|a, b| a.time().total_cmp(&b.time())).map(|m| m.ts.clone()).or(e.root.latest_reply.clone())
    }

    /// The thread was marked read here: it is no longer shown as unread (its "new" rule stays until the next load).
    pub fn mark_read(&mut self, channel: &str, thread_ts: &str) {
        if let Some(e) = self.entry_mut(channel, thread_ts) {
            e.read = true;
            self.dirty = true;
        }
    }

    /// A reply `m` in `channel` came (`unread`: from someone else): into its thread's block, if the feed has it
    /// (and not yet). True when the feed has its thread.
    pub fn add_reply(&mut self, channel: &str, m: &Message, unread: bool) -> bool {
        let Some(parent) = m.thread_ts.clone().filter(|_| m.is_reply()) else { return false };
        let Some(e) = self.entry_mut(channel, &parent) else { return false };
        if e.unread.iter().chain(&e.latest).any(|r| r.ts == m.ts) {
            return true;
        }
        // The parent's own update may already have brought Slack's count for this reply (as in a timeline).
        let counted = e.root.latest_reply.as_deref().and_then(|l| l.parse::<f64>().ok()).is_some_and(|l| l >= m.time());
        if !counted {
            e.root.reply_count += 1;
            e.root.latest_reply = Some(m.ts.clone());
        }
        if let Some(u) = m.user.as_ref().filter(|u| !e.root.reply_users.contains(u)) {
            e.root.reply_users.push(u.clone());
        }
        if unread {
            // Read here already: what is new starts at this one.
            if e.read {
                let seen = std::mem::take(&mut e.unread);
                e.latest.extend(seen);
            }
            e.unread.push(m.clone());
            e.read = false;
            self.clock.forget(channel, &parent);
        } else {
            e.latest.push(m.clone());
        }
        self.fetched.remove(&(channel.to_string(), parent));
        self.dirty = true;
        true
    }

    /// A live edit, deletion or reaction (an RTM event) on a parent or a reply in the feed: applied in
    /// place, the threads keep their order. True when the feed has the message.
    pub fn apply_live(&mut self, ev: &Value) -> bool {
        let Some(entries) = self.entries.as_mut() else { return false };
        let Some((channel, ts)) = super::live::changed_message(ev) else { return false };
        let Some(e) = entries.iter_mut().find(|e| e.channel == channel && (e.root.ts == ts || e.unread.iter().chain(&e.latest).any(|r| r.ts == ts))) else { return false };
        if ev["subtype"] == "message_deleted" {
            if e.root.ts == ts {
                // Its thread stays (its row too): its parent says it is not available.
                let root = std::mem::take(&mut e.root);
                e.root = Message { ts: root.ts, thread_ts: root.thread_ts, user: root.user, reply_count: root.reply_count, reply_users: root.reply_users, latest_reply: root.latest_reply, ..Default::default() };
            } else {
                e.unread.retain(|r| r.ts != ts);
                e.latest.retain(|r| r.ts != ts);
                // Not when the parent's update came first (its newest reply is already an older one); who replied
                // is left to that update.
                let gone = ts.parse::<f64>().unwrap_or(f64::INFINITY);
                if e.root.latest_reply.as_deref().and_then(|l| l.parse::<f64>().ok()).is_none_or(|l| l >= gone) {
                    e.root.reply_count = e.root.reply_count.saturating_sub(1);
                    e.root.latest_reply = e.unread.iter().chain(&e.latest).max_by(|a, b| a.time().total_cmp(&b.time())).map(|r| r.ts.clone());
                }
                // The selection goes to the message above it.
                let key = (channel.to_string(), ts.to_string());
                if self.selected.as_ref() == Some(&key) {
                    self.selected = self.order.iter().position(|o| *o == key).and_then(|n| n.checked_sub(1)).map(|n| self.order[n].clone());
                }
                if self.hover.as_ref() == Some(&key) {
                    self.hover = None;
                }
            }
        } else {
            for m in std::iter::once(&mut e.root).chain(&mut e.unread).chain(&mut e.latest).filter(|m| m.ts == ts) {
                super::live::apply_change(m, ev);
            }
        }
        self.dirty = true;
        true
    }

    /// The selected message (channel, ts); none selected yet: the first one.
    pub fn selected_message(&self) -> Option<(String, String)> {
        let (channel, ts) = self.selected.as_ref().or(self.order.first())?;
        self.find(channel, ts).map(|_| (channel.clone(), ts.clone()))
    }

    /// Message `ts` of `channel` in the feed (a parent or a reply; not a parent shown as not available).
    pub fn find(&self, channel: &str, ts: &str) -> Option<&Message> {
        let gone = |m: &Message| m.text.trim().is_empty() && m.blocks.is_empty() && m.files.is_empty() && m.attachments.is_empty();
        self.entries.as_ref()?.iter().filter(|e| e.channel == channel).flat_map(|e| std::iter::once(&e.root).chain(&e.unread).chain(&e.latest)).find(|m| m.ts == ts).filter(|m| !gone(m))
    }

    /// Every copy of message `ts` of `channel` in the feed, to change in place (it is built again for them).
    pub fn copies_mut(&mut self, channel: &str, ts: &str) -> Vec<&mut Message> {
        let Some(entries) = self.entries.as_mut() else { return Vec::new() };
        let out: Vec<&mut Message> = entries.iter_mut().filter(|e| e.channel == channel).flat_map(|e| std::iter::once(&mut e.root).chain(&mut e.unread).chain(&mut e.latest)).filter(|m| m.ts == ts).collect();
        self.dirty |= !out.is_empty();
        out
    }

    /// A change ttyak made to message `m` of `channel` failed: it is put back as it was (a reply deleted here goes
    /// into its block again; one the feed never showed stays out).
    pub fn put_back(&mut self, channel: &str, m: &Message) {
        let mut copies = self.copies_mut(channel, &m.ts);
        if !copies.is_empty() {
            for c in copies.iter_mut() {
                **c = m.clone();
            }
            return;
        }
        drop(copies);
        if self.deleted.remove(&(channel.to_string(), m.ts.clone())) {
            self.add_reply(channel, m, false);
        }
    }

    /// Message `ts` of `channel` deleted here: out of the feed as Slack's own deletion would take it (that comes later
    /// and changes nothing then). True when the feed showed it.
    pub fn delete_here(&mut self, channel: &str, ts: &str) -> bool {
        let reply = self.find(channel, ts).is_some_and(Message::is_reply);
        let gone = serde_json::json!({"type": "message", "subtype": "message_deleted", "channel": channel, "deleted_ts": ts});
        let shown = self.apply_live(&gone);
        if shown && reply {
            self.deleted.insert((channel.to_string(), ts.to_string()));
        }
        shown
    }

    /// Where a menu for message (`channel`, `ts`) opens: its first row on screen, past the gutter (the feed's middle
    /// when it is not on screen).
    pub fn spot(&self, channel: &str, ts: &str) -> (u16, u16) {
        match self.rows.iter().position(|o| o.as_ref().is_some_and(|(c, t)| c == channel && t == ts)) {
            Some(row) => (self.list.x + super::messages::GUTTER + 2, self.list.y + row as u16),
            None => (self.list.x + self.list.width / 3, self.list.y + self.list.height / 3),
        }
    }

    /// The threads whose blocks have been on screen long enough (`after`) at `now`: to mark read (each once).
    pub fn tick(&mut self, shown: bool, now: Instant, after: Duration) -> Vec<(String, String)> {
        let visible = if shown { self.visible.clone() } else { 0..0 };
        self.clock.tick(visible, &self.feed.blocks, now, after)
    }

    /// The threads on screen at the last draw that show fewer replies than they have (up to
    /// [`super::feed::REPLIES_SHOWN`]): to fetch their newest replies, each once (again after a
    /// load, or once a new reply came).
    pub fn wanted_replies(&mut self) -> Vec<(String, String)> {
        let Some(entries) = &self.entries else { return Vec::new() };
        let mut out = Vec::new();
        for b in self.feed.blocks.iter().filter(|b| b.rows.start < self.visible.end && self.visible.start < b.rows.end) {
            let key = (b.channel.clone(), b.thread_ts.clone());
            let Some(e) = entries.iter().find(|e| e.channel == b.channel && e.root.ts == b.thread_ts) else { continue };
            let known = e.unread.len() + e.latest.len();
            if known < super::feed::REPLIES_SHOWN.min(e.root.reply_count as usize) && self.fetched.insert(key.clone()) {
                out.push(key);
            }
        }
        out
    }

    /// The newest replies of a thread came (`page`: as `conversations.replies` gives them, with the
    /// parent): those not known yet go into its block, as read.
    pub fn merge_replies(&mut self, channel: &str, thread_ts: &str, page: Vec<Message>) {
        let Some(e) = self.entry_mut(channel, thread_ts) else { return };
        for m in page.into_iter().filter(|m| m.ts != thread_ts) {
            if !e.unread.iter().chain(&e.latest).any(|r| r.ts == m.ts) {
                e.latest.push(m);
            }
        }
        self.dirty = true;
    }

    /// The links to messages drawn now whose author is worth asking for.
    pub fn authors_wanted(&self) -> impl Iterator<Item = &AppLink> {
        self.links.iter().filter(|l| l.3).filter_map(|(_, _, app, _)| app.as_ref())
    }

    /// Any link drawn at a terminal position: its address and where it goes in ttyak.
    pub fn link_under(&self, column: u16, row: u16) -> Option<(String, Option<AppLink>)> {
        if !self.area.contains((column, row).into()) {
            return None;
        }
        self.links.iter().find(|(at, ..)| at.contains((column, row).into())).map(|(_, url, app, _)| (url.clone(), app.clone()))
    }

    /// The reaction pill drawn at a terminal position: its message (channel, ts) and the reaction's name.
    pub fn pill_at(&self, column: u16, row: u16) -> Option<((String, String), String)> {
        if !self.area.contains((column, row).into()) {
            return None;
        }
        self.pills.iter().find(|(at, ..)| at.contains((column, row).into())).map(|(_, m, name)| (m.clone(), name.clone()))
    }

    /// Not drawn this time: nothing of it is under the mouse or on screen.
    pub fn hide(&mut self) {
        (self.area, self.list, self.hover, self.visible) = (Rect::default(), Rect::default(), None, 0..0);
        self.rows.clear();
        self.row_threads.clear();
        self.links.clear();
        self.pills.clear();
    }

    /// Draw the view into `area` (the main pane), its frame lit while it has the focus.
    pub fn draw(&mut self, f: &mut Frame, area: Rect, here: &Here, focused: bool, media: &Media) {
        let p = super::theme::p();
        let ws = here.ws;
        // (The pointer's message stays: it is hovered until it moves.)
        (self.visible, self.area) = (0..0, area);
        self.rows.clear();
        self.row_threads.clear();
        self.links.clear();
        self.pills.clear();
        self.wanted.clear();
        let border = Style::default().fg(if focused { p.accent } else { p.dim });
        let block = Block::default().borders(Borders::ALL).border_style(border).title(format!(" {} ", super::i18n::tr("Threads")));
        let inner = block.inner(area);
        f.render_widget(block, area);
        self.list = inner;
        let note = match &self.entries {
            None if self.failed.is_some() => Some(Line::styled(format!("{}: {}", super::i18n::tr(FAILED), self.failed.as_deref().unwrap_or_default()), Style::default().fg(p.danger))),
            None => Some(Line::styled(super::i18n::tr(LOADING).into_owned(), Style::default().fg(p.dim))),
            Some(entries) if entries.is_empty() => Some(Line::styled(super::i18n::tr(EMPTY).into_owned(), Style::default().fg(p.dim))),
            Some(_) => None,
        };
        if let Some(note) = note {
            f.render_widget(Paragraph::new(note), inner);
            return;
        }
        let entries = self.entries.as_deref().unwrap_or_default();
        let avatars = if !ws.settings.messages.avatars {
            Avatars::Off
        } else if media.avatars {
            Avatars::Pictures
        } else {
            Avatars::Initials
        };
        // Built again at once when its threads or its width changed; when only what it looks like did, at most every
        // [`REBUILD`]. Never just because time passed.
        let stale = self.dirty || self.built.as_ref().is_none_or(|(look, at)| look.width() != inner.width || (!look.holds(here, inner.width, avatars) && at.elapsed() >= REBUILD));
        if stale {
            let thumb_size = |file: &File, max_cols: u16| media.thumb_size(file, max_cols).filter(|_| ws.settings.messages.inline_images);
            self.feed = super::feed::build(here, entries, inner.width, super::feed::REPLIES_SHOWN, self.unread_first, &thumb_size, avatars);
            let messages: Vec<Message> = entries.iter().flat_map(|e| std::iter::once(&e.root).chain(&e.unread).chain(&e.latest)).cloned().collect();
            let owners: Vec<Option<String>> = self.feed.owners.iter().map(|o| o.as_ref().map(|(_, ts)| ts.clone())).collect();
            self.feed_links = super::messages::link_spots(here, &messages, &self.feed.lines, &owners);
            self.feed_pills = super::messages::pill_spots(ws, &messages, &self.feed.lines, &owners);
            self.order = self.feed.owners.iter().flatten().cloned().collect();
            self.order.dedup();
            let people = messages.iter().filter_map(|m| m.user.as_deref());
            let look = super::messages::Look::new(here, inner.width, avatars, people);
            (self.built, self.dirty) = (Some((look, Instant::now())), false);
            self.builds += 1;
        }
        if let Some((channel, ts, _)) = self.want.take_if(|w| self.order.iter().any(|(c, t)| *c == w.0 && *t == w.1)) {
            (self.selected, self.follow) = (Some((channel, ts)), true);
        }
        if self.selected.as_ref().is_none_or(|s| !self.order.contains(s)) {
            self.selected = self.order.first().cloned();
        }
        if self.size != (inner.width, inner.height) {
            (self.size, self.follow) = ((inner.width, inner.height), true);
        }
        let feed = &self.feed;
        let height = inner.height as usize;
        let more = if self.loading { 1 } else { 0 };
        let total = feed.lines.len() + more;
        let scrolled = std::mem::take(&mut self.scrolled);
        if let Some(sel) = self.selected.as_ref().filter(|_| !self.follow && !scrolled) {
            let spot = self.spot.as_ref().map(|(s, row)| (s, *row));
            (self.scroll, self.follow) = keep_spot(&feed.owners, self.scroll, height, sel, spot);
        }
        // Bring a selection moved by the keys into view (with its thread's header, for a parent).
        if std::mem::take(&mut self.follow) {
            if let Some(sel) = &self.selected {
                if let (Some(first), Some(last)) = (feed.owners.iter().position(|o| o.as_ref() == Some(sel)), feed.owners.iter().rposition(|o| o.as_ref() == Some(sel))) {
                    let first = if feed.blocks.iter().any(|b| b.rows.start + 1 == first) { first - 1 } else { first };
                    self.scroll = bring_into_view(self.scroll, height, first, last);
                }
            }
        }
        self.scroll = self.scroll.min(total.saturating_sub(height));
        // (No room at all leaves `total` one past the lines: start at most at their end.)
        let start = self.scroll.min(feed.lines.len());
        let end = (start + height).min(feed.lines.len());
        // Scrolled off the selection by you (the wheel): it follows onto the screen; the view stays.
        if let Some(next) = self.selected.as_ref().filter(|_| scrolled).and_then(|s| follow_view(&feed.owners, start, end, s)) {
            self.selected = Some(next);
        }
        self.spot = self.selected.clone().and_then(|s| Some((feed.owners.iter().position(|o| o.as_ref() == Some(&s))? as isize - start as isize, s))).map(|(row, s)| (s, row));
        let mut lines: Vec<Line<'static>> = feed.lines[start..end].to_vec();
        let mut rows: Vec<Option<(String, String)>> = feed.owners[start..end].to_vec();
        if self.loading && lines.len() < height {
            lines.push(Line::styled(super::i18n::tr(LOADING_MORE).into_owned(), Style::default().fg(p.dim)));
            rows.push(None);
        }
        let selected = self.selected.as_ref().filter(|_| focused);
        if let Some(sel) = selected {
            let highlight = Style::default().bg(p.selection);
            for (line, owner) in lines.iter_mut().zip(&rows) {
                if owner.as_ref() == Some(sel) {
                    *line = std::mem::take(line).patch_style(highlight);
                }
            }
        }
        let slots: Vec<ImageSlot> = feed.slots.iter().filter(|s| s.line + s.size.1 as usize > start && s.line < end).cloned().collect();
        super::messages::fill_rows(f, inner, &rows, &[selected], self.hover.as_ref(), &slots, start, 0, media);
        f.render_widget(Paragraph::new(lines), inner);
        for spot in self.feed_links.iter().filter(|s| (start..end).contains(&s.line)) {
            let (x, y) = (inner.x + spot.col, inner.y + (spot.line - start) as u16);
            let width = spot.width.min(inner.right().saturating_sub(x));
            super::messages::hyperlink(f.buffer_mut(), x, y, width, &spot.url);
            self.links.push((Rect { x, y, width, height: 1 }, spot.url.clone(), spot.app.clone(), spot.wants_author));
        }
        for spot in self.feed_pills.iter().filter(|s| (start..end).contains(&s.line)) {
            let (x, y) = (inner.x + spot.col, inner.y + (spot.line - start) as u16);
            if let Some(m) = feed.owners[spot.line].clone() {
                self.pills.push((Rect { x, y, width: spot.width.min(inner.right().saturating_sub(x)), height: 1 }, m, spot.name.clone()));
            }
        }
        super::scroll::indicator(f, area, start, end - start, total, border);
        self.wanted = super::messages::draw_pictures(f, inner, slots, start, 0, media);
        self.row_threads = (start..start + rows.len()).map(|n| feed.blocks.iter().find(|b| b.rows.contains(&n)).map(|b| (b.channel.clone(), b.thread_ts.clone()))).collect();
        self.rows = rows;
        self.visible = start..end;
    }
}

/// The scroll (lines above the view) that shows lines `first..=last` of a top-anchored list: moved as little as it
/// takes; one taller than the view shows from its start.
pub(super) fn bring_into_view(scroll: usize, height: usize, first: usize, last: usize) -> usize {
    if last + 1 - first > height || first < scroll {
        first
    } else if last >= scroll + height {
        last + 1 - height
    } else {
        scroll
    }
}

/// The item to select instead of `sel` once a list (`owners`: the item of each of its lines) shows lines `start..end`:
/// none while `sel` is on screen (all of it, or filling the view), or is not in the list; else the nearest one on
/// screen ([`view_edge`]) — the top one when `sel` is above the view, the bottom one when it is below.
pub(super) fn follow_view<T: PartialEq + Clone>(owners: &[Option<T>], start: usize, end: usize, sel: &T) -> Option<T> {
    let (first, last) = span(owners, sel)?;
    if (first >= start && last < end) || (first <= start && last + 1 >= end) {
        return None;
    }
    view_edge(owners, start, end, first < start)
}

/// The item on screen (lines `start..end` of a list, `owners` as in [`follow_view`]) nearest its top (`top`) or its
/// bottom edge: one shown whole, if any is.
pub(super) fn view_edge<T: PartialEq + Clone>(owners: &[Option<T>], start: usize, end: usize, top: bool) -> Option<T> {
    let end = end.min(owners.len());
    let mut shown: Vec<&T> = owners.get(start..end)?.iter().flatten().collect();
    shown.dedup();
    let whole = |t: &&T| span(owners, t).is_some_and(|(first, last)| first >= start && last < end);
    let full: Vec<&T> = shown.iter().copied().filter(whole).collect();
    let pick = if full.is_empty() { shown } else { full };
    if top { pick.first() } else { pick.last() }.map(|t| (*t).clone())
}

/// The scroll of a list (`owners` as in [`follow_view`], `height` rows) that nothing scrolled since its last draw, and
/// whether its selection `sel` must still be brought into view: back on the row it was on (`spot`: the selection then
/// and that row) after the list changed above it (a reply, a message coming in, a pill row, an edit), which is not a
/// scroll and never moves the selection.
pub(super) fn keep_spot<T: PartialEq + Clone>(owners: &[Option<T>], scroll: usize, height: usize, sel: &T, spot: Option<(&T, isize)>) -> (usize, bool) {
    let mut scroll = scroll;
    if let (Some((_, row)), Some((first, _))) = (spot.filter(|(was, _)| *was == sel), span(owners, sel)) {
        scroll = (first as isize - row).max(0) as usize;
    }
    let off = follow_view(owners, scroll, (scroll + height).min(owners.len()), sel).is_some();
    (scroll, off)
}

/// The first and last lines of item `t` in a list (`owners` as in [`follow_view`]).
fn span<T: PartialEq>(owners: &[Option<T>], t: &T) -> Option<(usize, usize)> {
    Some((owners.iter().position(|o| o.as_ref() == Some(t))?, owners.iter().rposition(|o| o.as_ref() == Some(t))?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::state::Workspace;

    #[test]
    fn threads_opened_here_are_seen() {
        let msg = |ts: &str| Message { ts: ts.into(), ..Default::default() };
        let entry = |root: &str, unread: &[&str]| Entry { channel: "C1".into(), root: msg(root), unread: unread.iter().map(|t| msg(t)).collect(), ..Default::default() };
        let mut page = vec![entry("1.0", &["5.0", "9.0"]), entry("2.0", &["6.0"]), entry("3.0", &[])];
        let seen = Seen::from([("C1/1.0".to_string(), 7.0), ("C1/2.0".to_string(), 8.0)]);
        apply_seen(&mut page, &seen);
        let state: Vec<(&str, usize, usize)> = page.iter().map(|e| (e.root.ts.as_str(), e.unread.len(), e.latest.len())).collect();
        assert_eq!(state, [("1.0", 1, 1), ("2.0", 0, 1), ("3.0", 0, 0)], "a reply after the opening stays unread; a seen thread goes after the unread ones");
        assert_eq!(page[1].latest[0].ts, "6.0", "its reply is still there, read");
    }

    /// The shape of a real `subscriptions.thread.getView` answer (testbed, texts shortened).
    fn page() -> Value {
        serde_json::json!({"ok": true, "has_more": true, "max_ts": "1792369591.000000", "new_threads_count": 1, "total_unread_replies": 1, "threads": [
            {"root_msg": {"channel": "D1", "ts": "1792349791.643499", "thread_ts": "1792349791.643499", "user": "U1", "text": "C3 notification click test", "reply_count": 2, "latest_reply": "1792349881.855209", "last_read": "1792349791.643499", "subscribed": true, "type": "message"},
             "unread_replies": [{"ts": "1792349881.855209", "thread_ts": "1792349791.643499", "user": "U2", "text": "a reply", "type": "message"}]},
            {"root_msg": {"channel": "C1", "ts": "1792324476.437109", "thread_ts": "1792324476.437109", "user": "U2", "text": "older", "reply_count": 7, "latest_reply": "1792325495.184089", "last_read": "1792325511.368919", "type": "message"},
             "latest_replies": [{"ts": "1792325400.000000", "user": "U1", "text": "six", "type": "message"}, {"ts": "1792325495.184089", "user": "U2", "text": "seven", "type": "message"}]}
        ]})
    }

    #[test]
    fn parses_a_real_page() {
        let (entries, more) = parse(&page());
        assert!(more);
        assert_eq!(entries.len(), 2);
        assert_eq!((entries[0].channel.as_str(), entries[0].root.ts.as_str(), entries[0].unread.len()), ("D1", "1792349791.643499", 1));
        assert_eq!(entries[0].unread[0].text, "a reply", "the unread reply");
        assert_eq!(entries[1].latest.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), ["six", "seven"], "all read: the newest");
        assert_eq!(entries[1].root.reply_count, 7);
    }

    /// A feed of `page()` drawn at 100×30 (focused).
    fn drawn(t: &mut Threads) -> ratatui::Terminal<ratatui::backend::TestBackend> {
        let ws = Workspace::default();
        let media = Media::with(ratatui_image::picker::Picker::halfblocks());
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| t.draw(f, f.area(), &Here::alone(&ws), true, &media)).unwrap();
        terminal
    }

    #[test]
    fn paging_and_targets() {
        let mut t = Threads::default();
        let (entries, more) = parse(&page());
        t.add_page(entries.clone(), more);
        assert_eq!(t.next_page().as_deref(), Some("1792325495.184089"), "before the oldest latest reply");
        t.loading = true;
        assert_eq!(t.next_page(), None, "not while one is loading");
        t.add_page(entries, false);
        assert_eq!(t.entries.as_ref().unwrap().len(), 2, "threads already listed are left out");
        assert_eq!(t.next_page(), None, "no more");
        drawn(&mut t);
        // The first message is selected: Enter opens its thread at it.
        assert_eq!(t.target(), Some(("D1".into(), "1792349791.643499".into(), Some("1792349791.643499".into()))));
        t.move_by(1);
        assert_eq!(t.target(), Some(("D1".into(), "1792349881.855209".into(), Some("1792349791.643499".into()))), "its reply");
        t.move_by(1);
        assert_eq!(t.selected_thread(), Some(("C1".into(), "1792324476.437109".into())), "message by message into the next thread");
        assert!(t.at_end(), "the whole feed is on screen");
        t.move_by(9);
        assert_eq!(t.target().unwrap().1, "1792325495.184089", "the last message");
    }

    #[test]
    fn a_tiny_area_draws_without_panicking() {
        let ws = Workspace::default();
        let media = Media::with(ratatui_image::picker::Picker::halfblocks());
        let mut t = Threads::default();
        t.add_page(parse(&page()).0, true);
        t.loading = true;
        for height in 0..=3 {
            t.scroll = 99;
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, height)).unwrap().draw(|f| t.draw(f, f.area(), &Here::alone(&ws), true, &media)).unwrap();
        }
    }

    #[test]
    fn clicks_and_the_pointer_find_the_message_drawn_there() {
        let mut t = Threads::default();
        t.add_page(parse(&page()).0, false);
        let terminal = drawn(&mut t);
        let buf = terminal.backend().buffer();
        let row_of = |what: &str| (0..buf.area.height).find(|&y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>().contains(what)).unwrap_or_else(|| panic!("{what} is drawn"));
        let reply = row_of("a reply");
        assert_eq!(t.message_at(5, reply), Some(("D1".into(), "1792349881.855209".into())));
        assert_eq!(t.thread_at(5, reply), None, "a message is not its thread's header");
        // The header (the first row) belongs to the thread, not a message.
        assert_eq!((t.message_at(5, 1), t.thread_at(5, 1)), (None, Some(("D1".into(), "1792349791.643499".into()))));
        let more = row_of("5 more replies");
        assert_eq!(t.thread_at(5, more), Some(("C1".into(), "1792324476.437109".into())), "its hidden replies' row");
        assert_eq!((t.message_at(0, reply), t.message_at(5, 0)), (None, None), "the border");
        t.hover_at(5, reply);
        assert_eq!(t.hover, Some(("D1".into(), "1792349881.855209".into())));
        assert_eq!(t.selected, Some(("D1".into(), "1792349791.643499".into())), "the pointer leaves the selection");
        t.hover_at(5, 1);
        assert_eq!(t.hover, None, "the header");
    }

    /// Reply `k` (by U2) in thread `n` of C1.
    fn reply(n: usize, k: usize) -> Message {
        Message { ts: format!("{n}.000{k}00"), thread_ts: Some(format!("{n}.000100")), user: Some("U2".into()), text: format!("reply {n}"), ..Default::default() }
    }

    /// Thread `n` in C1: parent `n.000100` by U1, one reply `n.000200` by U2 (`unread` or not).
    fn thread(n: usize, unread: bool) -> Entry {
        let parent = format!("{n}.000100");
        let root = Message { ts: parent.clone(), thread_ts: Some(parent), user: Some("U1".into()), text: format!("parent {n}"), reply_count: 1, ..Default::default() };
        let (unread, latest) = if unread { (vec![reply(n, 2)], vec![]) } else { (vec![], vec![reply(n, 2)]) };
        Entry { channel: "C1".into(), root, unread, latest, ..Default::default() }
    }

    /// Three threads in C1, one reply each (the first unread), parents `1.0`, `2.0`, `3.0`.
    fn three() -> Vec<Entry> {
        vec![thread(1, true), thread(2, false), thread(3, false)]
    }

    #[test]
    fn the_up_to_date_line_stays_where_the_load_put_it() {
        // The thread whose block the line is under.
        let under = |t: &mut Threads| -> Option<String> {
            drawn(t);
            let at = t.feed.lines.iter().position(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>().contains("You're up to date"))?;
            t.feed.blocks.iter().find(|b| b.rows.end + super::super::feed::GAP == at).map(|b| b.thread_ts.clone())
        };
        let mut t = Threads::default();
        t.add_page(three(), false);
        assert_eq!(under(&mut t).as_deref(), Some("1.000100"));
        // New replies in the last thread and in one below it, reading the one above: it stays.
        assert!(t.add_reply("C1", &reply(3, 3), true));
        assert_eq!(under(&mut t).as_deref(), Some("1.000100"), "a reply in the last thread");
        assert!(t.add_reply("C1", &reply(2, 3), true));
        assert_eq!(under(&mut t).as_deref(), Some("1.000100"), "a reply below it");
        t.mark_read("C1", "1.000100");
        assert_eq!(under(&mut t).as_deref(), Some("1.000100"), "read here");
        // Loaded again with nothing new: none, and a reply then brings none.
        t.entries = None;
        t.add_page(vec![thread(1, false), thread(2, false), thread(3, false)], false);
        assert_eq!(under(&mut t), None);
        t.add_reply("C1", &reply(2, 3), true);
        assert_eq!(under(&mut t), None, "none at load stays none");
        // Pages: while every thread so far has new replies, the next page goes on counting; after that, never.
        t.entries = None;
        t.add_page(vec![thread(1, true), thread(2, true)], true);
        assert_eq!(under(&mut t), None, "all of them so far");
        t.add_page(vec![thread(3, true), thread(4, false)], true);
        assert_eq!(under(&mut t).as_deref(), Some("3.000100"));
        t.add_page(vec![thread(5, true)], false);
        assert_eq!(under(&mut t).as_deref(), Some("3.000100"), "a later page's thread with new replies stays below it");
    }

    #[test]
    fn a_reply_slack_already_counted_is_not_counted_again() {
        let mut t = Threads::default();
        t.add_page(three(), false);
        let parent = |count: u64, latest: &str| serde_json::json!({"type": "message", "subtype": "message_changed", "channel": "C1", "message": {"type": "message", "user": "U1", "text": "parent 2", "ts": "2.000100", "thread_ts": "2.000100", "reply_count": count, "latest_reply": latest, "reply_users": ["U2"]}});
        let root = |t: &Threads| t.entries.as_ref().unwrap()[1].root.clone();
        // The parent's new count comes before the reply.
        assert!(t.apply_live(&parent(2, "2.000300")));
        t.add_reply("C1", &reply(2, 3), true);
        assert_eq!(root(&t).reply_count, 2, "counted once");
        // Deleted, then the parent's count (as Slack sends them): one fewer, the newest reply the one before.
        assert!(t.apply_live(&serde_json::json!({"type": "message", "subtype": "message_deleted", "channel": "C1", "deleted_ts": "2.000300"})));
        assert_eq!((root(&t).reply_count, root(&t).latest_reply.as_deref()), (1, Some("2.000200")));
        t.apply_live(&parent(1, "2.000200"));
        assert_eq!(root(&t).reply_count, 1);
        // The other way round: the parent's count first, then the deletion it already counts.
        t.add_reply("C1", &reply(2, 4), true);
        assert_eq!(root(&t).reply_count, 2);
        t.apply_live(&parent(1, "2.000200"));
        t.apply_live(&serde_json::json!({"type": "message", "subtype": "message_deleted", "channel": "C1", "deleted_ts": "2.000400"}));
        assert_eq!(root(&t).reply_count, 1, "not counted off twice");
    }

    #[test]
    fn the_rows_between_threads_belong_to_nothing() {
        let mut t = Threads::default();
        // All read (no "You're up to date" line).
        let mut read = three();
        read[0].latest = std::mem::take(&mut read[0].unread);
        t.add_page(read, false);
        let terminal = drawn(&mut t);
        let buf = terminal.backend().buffer().clone();
        let p = super::super::theme::p();
        let row = |y: u16| (t.list.x..t.list.right()).map(|x| buf[(x, y)].symbol()).collect::<String>();
        let rules: Vec<u16> = (t.list.y..t.list.bottom()).filter(|&y| row(y) == "─".repeat(t.list.width as usize)).collect();
        assert_eq!(rules.len(), 2, "one between each two threads: {:#?}", (0..buf.area.height).map(row).collect::<Vec<_>>());
        for &y in &rules {
            assert_eq!(buf[(t.list.x + 3, y)].fg, p.dim, "dim");
            let gap = super::super::feed::GAP as u16;
            for y in y - gap..=y + gap {
                assert_eq!((t.message_at(5, y), t.thread_at(5, y)), (None, None), "row {y}: neither a message nor a thread");
            }
            assert_eq!(t.thread_at(5, y + gap + 1), Some(("C1".into(), if y == rules[0] { "2.000100" } else { "3.000100" }.into())), "then the next thread's header");
        }
        // The selection walks the messages and never lands on them.
        for _ in 0..6 {
            let terminal = drawn(&mut t);
            let buf = terminal.backend().buffer();
            let gap = super::super::feed::GAP as u16;
            assert!(rules.iter().all(|&y| (y - gap..=y + gap).all(|y| buf[(t.list.x + 3, y)].bg != p.selection)), "at {:?}", t.selected);
            t.move_by(1);
        }
        assert_eq!(t.selected, Some(("C1".into(), "3.000200".into())), "six messages, rows between them skipped");
    }

    #[test]
    fn up_to_date_is_drawn_under_the_unread_threads_and_belongs_to_nothing() {
        let mut t = Threads::default();
        t.add_page(three(), false);
        let terminal = drawn(&mut t);
        let buf = terminal.backend().buffer().clone();
        let p = super::super::theme::p();
        let row = |y: u16| (t.list.x..t.list.right()).map(|x| buf[(x, y)].symbol()).collect::<String>();
        let screen: Vec<String> = (0..buf.area.height).map(row).collect();
        let at = (t.list.y..t.list.bottom()).find(|&y| row(y).contains("You're up to date")).unwrap_or_else(|| panic!("{screen:#?}"));
        assert!(row(at - 1).trim().is_empty() && row(at - 2).trim().is_empty() && row(at - 3).contains("reply 1") && row(at + 3).contains("C1"), "between the unread thread and the first read one: {screen:#?}");
        assert_eq!(buf[(t.list.x + 1, at)].fg, p.dim, "dim");
        assert_eq!((t.message_at(5, at), t.thread_at(5, at)), (None, None), "neither a message nor a thread");
        let rules = (t.list.y..t.list.bottom()).filter(|&y| row(y) == "─".repeat(t.list.width as usize)).count();
        assert_eq!(rules, 1, "the rule between the two read threads stays: {screen:#?}");
        // The selection goes from the unread reply straight to the next parent.
        t.selected = Some(("C1".into(), "1.000200".into()));
        t.move_by(1);
        assert_eq!(t.selected, Some(("C1".into(), "2.000100".into())));
    }

    #[test]
    fn edits_deletions_and_reactions_apply_in_place() {
        let mut t = Threads::default();
        t.add_page(three(), false);
        let text = |t: &mut Threads| {
            let terminal = drawn(t);
            let buf = terminal.backend().buffer().clone();
            (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
        };
        let order = |t: &Threads| t.entries.as_ref().unwrap().iter().map(|e| e.root.ts.clone()).collect::<Vec<_>>();
        text(&mut t);
        let builds = t.builds;
        // A reaction on a parent, then an edit of it that leaves its reactions out: both shown, the reaction kept.
        assert!(t.apply_live(&serde_json::json!({"type": "reaction_added", "user": "U2", "reaction": "eyes", "item": {"type": "message", "channel": "C1", "ts": "2.000100"}})));
        let edit = serde_json::json!({"type": "message", "subtype": "message_changed", "channel": "C1", "message": {"type": "message", "user": "U1", "text": "parent 2, edited", "ts": "2.000100", "thread_ts": "2.000100", "edited": {"user": "U1", "ts": "9.0"}}});
        assert!(t.apply_live(&edit));
        let shown = text(&mut t);
        assert!(shown.contains("parent 2, edited (edited)") && shown.contains("👀"), "{shown}");
        assert_eq!(t.entries.as_ref().unwrap()[1].root.reactions[0].count, 1);
        assert_eq!(t.builds, builds + 1, "built again once, at the draw");
        assert_eq!(t.entries.as_ref().unwrap()[1].root.reply_count, 1, "the thread's fields kept");
        // Removed again.
        assert!(t.apply_live(&serde_json::json!({"type": "reaction_removed", "user": "U2", "reaction": "eyes", "item": {"type": "message", "channel": "C1", "ts": "2.000100"}})));
        assert!(!text(&mut t).contains("👀"));
        // The unread reply deleted while selected: gone, the thread no longer counts it; the selection goes to the message above it.
        t.selected = Some(("C1".into(), "1.000200".into()));
        assert!(t.apply_live(&serde_json::json!({"type": "message", "subtype": "message_deleted", "channel": "C1", "deleted_ts": "1.000200"})));
        let shown = text(&mut t);
        assert!(!shown.contains("reply 1") && !shown.contains(" 1 new ") && !shown.contains("1 more reply"), "{shown}");
        assert_eq!(t.entries.as_ref().unwrap()[0].root.reply_count, 0);
        assert_eq!(t.selected, Some(("C1".into(), "1.000100".into())));
        // A read reply deleted.
        assert!(t.apply_live(&serde_json::json!({"type": "message", "subtype": "message_deleted", "channel": "C1", "deleted_ts": "2.000200"})));
        assert!(!text(&mut t).contains("reply 2"));
        assert_eq!((t.entries.as_ref().unwrap()[1].latest.len(), t.entries.as_ref().unwrap()[1].root.reply_count), (0, 0));
        // A parent deleted while selected and under the pointer: its thread stays, saying so, its row and author too.
        let parent = ("C1".to_string(), "3.000100".to_string());
        (t.selected, t.hover) = (Some(parent.clone()), Some(parent.clone()));
        assert!(t.apply_live(&serde_json::json!({"type": "message", "subtype": "message_deleted", "channel": "C1", "deleted_ts": "3.000100"})));
        let shown = text(&mut t);
        assert!(shown.contains("message not available") && shown.contains("reply 3") && !shown.contains("parent 3"), "{shown}");
        assert_eq!((t.selected.as_ref(), t.hover.as_ref()), (Some(&parent), Some(&parent)), "the selection and the pointer stay on it");
        let header = t.feed.blocks.iter().find(|b| b.thread_ts == "3.000100").map(|b| t.feed.lines[b.rows.start].spans.iter().map(|s| s.content.to_string()).collect::<String>()).unwrap();
        assert!(header.contains("U1"), "its author still among who is in it: {header:?}");
        assert_eq!(order(&t), ["1.000100", "2.000100", "3.000100"], "nothing moved");
        // Not in the feed (another channel, another message): nothing to build again.
        let builds = t.builds;
        assert!(!t.apply_live(&serde_json::json!({"type": "reaction_added", "user": "U2", "reaction": "eyes", "item": {"type": "message", "channel": "C9", "ts": "2.000100"}})));
        assert!(!t.apply_live(&serde_json::json!({"type": "message", "subtype": "message_deleted", "channel": "C1", "deleted_ts": "7.0"})));
        assert!(!t.apply_live(&serde_json::json!({"type": "message", "subtype": "message_changed", "channel": "C1", "message": {"type": "message", "text": "x", "ts": "7.0"}})));
        text(&mut t);
        assert_eq!(t.builds, builds);
    }

    #[test]
    fn only_a_reply_deleted_here_is_put_back() {
        let mut t = Threads::default();
        t.add_page(three(), false);
        // An old reply the feed never showed (an edit of it failed elsewhere): not added.
        let old = Message { ts: "2.000150".into(), ..reply(2, 1) };
        t.put_back("C1", &old);
        assert_eq!(t.entries.as_ref().unwrap()[1].latest.len(), 1);
        assert!(t.find("C1", "2.000150").is_none());
        // One deleted here whose deletion failed: back in its block.
        let gone = reply(2, 2);
        assert!(t.delete_here("C1", &gone.ts));
        assert!(t.find("C1", &gone.ts).is_none());
        t.put_back("C1", &gone);
        assert!(t.find("C1", &gone.ts).is_some());
        assert_eq!(t.entries.as_ref().unwrap()[1].root.reply_count, 1, "counted again");
        // An edit that failed: the copy as it was.
        let mut edited = gone.clone();
        edited.text = "reply 2, edited".into();
        t.copies_mut("C1", &gone.ts)[0].text = edited.text.clone();
        t.put_back("C1", &gone);
        assert_eq!(t.find("C1", &gone.ts).unwrap().text, "reply 2");
    }

    #[test]
    fn the_selection_fills_the_whole_row() {
        let mut t = Threads::default();
        t.add_page(parse(&page()).0, false);
        let terminal = drawn(&mut t);
        // Drawn into the main pane's whole area: the feed fills it inside the view's frame (title on top).
        assert_eq!(t.list, Rect::new(1, 1, 98, 28));
        let buf = terminal.backend().buffer().clone();
        let p = super::super::theme::p();
        let (x0, x1) = (t.list.x, t.list.right() - 1);
        let parent: Vec<u16> = (t.list.y..t.list.bottom()).filter(|&y| t.message_at(5, y) == Some(("D1".into(), "1792349791.643499".into()))).collect();
        assert!(!parent.is_empty());
        for &y in &parent {
            assert_eq!((buf[(x0, y)].bg, buf[(x1, y)].bg), (p.selection, p.selection), "row {y}: edge to edge");
        }
        for y in (t.list.y..t.list.bottom()).filter(|y| !parent.contains(y)) {
            assert!((x0..=x1).all(|x| buf[(x, y)].bg != p.selection), "row {y}: not the selected message");
        }
        assert_ne!(buf[(x1 + 1, parent[0])].bg, p.selection, "not the border");
    }

    #[test]
    fn the_feed_is_built_again_only_when_something_in_it_changes() {
        let mut ws = Workspace::default();
        for id in ["U1", "U2"] {
            ws.users.insert(id.into(), slack_core::types::User { id: id.into(), name: id.to_lowercase(), ..Default::default() });
        }
        ws.users_changed();
        let media = Media::with(ratatui_image::picker::Picker::halfblocks());
        let draw = |t: &mut Threads, ws: &Workspace, width: u16| {
            let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 30)).unwrap();
            terminal.draw(|f| t.draw(f, f.area(), &Here::alone(ws), true, &media)).unwrap();
            let buf = terminal.backend().buffer().clone();
            (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
        };
        let a_second_on = |t: &mut Threads| t.built.as_mut().unwrap().1 -= Duration::from_secs(2);
        let mut t = Threads::default();
        t.add_page(parse(&page()).0, false);
        draw(&mut t, &ws, 100);
        assert_eq!(t.builds, 1);
        // Seconds go by with nothing new: not built again (it was, every second).
        a_second_on(&mut t);
        draw(&mut t, &ws, 100);
        t.hover_at(5, 3);
        t.move_by(1);
        draw(&mut t, &ws, 100);
        assert_eq!(t.builds, 1);
        // Someone's name or status changed: built again, at most once a second.
        let renamed = |ws: &mut Workspace, name: &str| crate::tui::live::apply(ws, &serde_json::json!({"type": "user_profile_changed", "user": {"id": "U2", "profile": {"display_name": name}}}), None);
        renamed(&mut ws, "annie");
        assert!(draw(&mut t, &ws, 100).contains("annie"));
        assert_eq!(t.builds, 2);
        renamed(&mut ws, "ann2");
        draw(&mut t, &ws, 100);
        assert_eq!(t.builds, 2, "within a second of the last build");
        a_second_on(&mut t);
        assert!(draw(&mut t, &ws, 100).contains("ann2"));
        assert_eq!(t.builds, 3);
        // A reply in a thread, the width: at once.
        let reply = Message { ts: "1792349990.000100".into(), thread_ts: Some("1792349791.643499".into()), user: Some("U1".into()), text: "brand new".into(), ..Default::default() };
        t.add_reply("D1", &reply, true);
        assert!(draw(&mut t, &ws, 100).contains("brand new"));
        draw(&mut t, &ws, 90);
        assert_eq!(t.builds, 5);
    }

    #[test]
    fn an_unparsable_parent_keeps_its_thread() {
        let mut v = page();
        // A parent whose shape ttyak does not know (a field of another type).
        v["threads"][1]["root_msg"]["reactions"] = serde_json::json!("not a list");
        let (entries, _) = parse(&v);
        assert_eq!(entries.len(), 2, "the thread stays");
        let e = &entries[1];
        assert_eq!((e.channel.as_str(), e.root.ts.as_str(), e.root.reply_count, e.latest.len()), ("C1", "1792324476.437109", 7, 2));
        assert!(e.root.text.is_empty() && e.root.user.is_none(), "nothing of it is shown as if it were its text");
        let mut t = Threads::default();
        t.add_page(entries, false);
        let terminal = drawn(&mut t);
        let screen: String = { let buf = terminal.backend().buffer(); (0..buf.area.height).flat_map(|y| (0..buf.area.width).map(move |x| (x, y))).map(|(x, y)| buf[(x, y)].symbol().to_string()).collect() };
        assert!(screen.contains("message not available") && screen.contains("seven"), "said so, its replies shown");
        // Without even a channel or ts there is nothing to place it by.
        let none = serde_json::json!({"threads": [{"root_msg": {"text": 5}}]});
        assert!(parse(&none).0.is_empty());
    }

    #[test]
    fn a_block_short_of_replies_asks_for_them_once() {
        let m = |ts: &str, text: &str| Message { ts: ts.into(), thread_ts: Some("1.000100".into()), user: Some("U2".into()), text: text.into(), ..Default::default() };
        let mut root = m("1.000100", "the parent");
        root.reply_count = 5;
        let full = Entry { channel: "C1".into(), root: Message { reply_count: 1, ..m("2.000100", "another parent") }, latest: vec![m("2.000200", "its only reply")], ..Default::default() };
        let short = Entry { channel: "C1".into(), root, latest: vec![m("1.000500", "reply 5")], ..Default::default() };
        let mut t = Threads::default();
        t.add_page(vec![short, full], false);
        drawn(&mut t);
        assert_eq!(t.wanted_replies(), [("C1".to_string(), "1.000100".to_string())], "only the one with fewer than it has");
        assert!(t.wanted_replies().is_empty(), "once");
        // The parent and its four newest replies (one known already).
        let page: Vec<Message> = std::iter::once(m("1.000100", "the parent")).chain((2..=5).map(|n| m(&format!("1.000{n}00"), &format!("reply {n}")))).collect();
        t.merge_replies("C1", "1.000100", page);
        let terminal = drawn(&mut t);
        let screen: String = { let buf = terminal.backend().buffer(); (0..buf.area.height).flat_map(|y| (0..buf.area.width).map(move |x| (x, y))).map(|(x, y)| buf[(x, y)].symbol().to_string()).collect() };
        assert!(["reply 3", "reply 4", "reply 5", "2 more replies"].iter().all(|w| screen.contains(w)) && !screen.contains("reply 2"), "the last three");
        assert_eq!(t.entries.as_ref().unwrap()[0].latest.len(), 4, "the known one not twice");
        assert!(t.wanted_replies().is_empty());
        // A new reply: it may ask again (here it has enough).
        t.add_reply("C1", &m("1.000600", "reply 6"), true);
        drawn(&mut t);
        assert!(t.wanted_replies().is_empty());
    }

    #[test]
    fn a_reply_goes_into_its_block() {
        let mut t = Threads::default();
        t.add_page(parse(&page()).0, false);
        let reply = Message { ts: "1792349900.000100".into(), thread_ts: Some("1792349791.643499".into()), user: Some("U3".into()), text: "late".into(), ..Default::default() };
        assert!(t.add_reply("D1", &reply, true));
        assert!(t.add_reply("D1", &reply, true), "already there: the feed has the thread");
        let e = &t.entries.as_ref().unwrap()[0];
        assert_eq!((e.unread.len(), e.root.reply_count, e.read), (2, 3, false));
        // Read here, then another: only that one is new.
        t.mark_read("D1", "1792349791.643499");
        let another = Message { ts: "1792349950.000100".into(), text: "later".into(), ..reply.clone() };
        t.add_reply("D1", &another, true);
        let e = &t.entries.as_ref().unwrap()[0];
        assert_eq!((e.unread.len(), e.latest.len(), e.read), (1, 2, false), "the rule moves to it");
        assert!(e.root.reply_users.contains(&"U3".to_string()));
        assert_eq!(t.newest_reply("D1", "1792349791.643499").as_deref(), Some("1792349950.000100"));
        assert!(!t.add_reply("C9", &reply, true), "a thread the feed does not have");
        t.mark_read("D1", "1792349791.643499");
        assert!(t.entries.as_ref().unwrap()[0].read);
        let terminal = drawn(&mut t);
        let screen: String = { let buf = terminal.backend().buffer(); (0..buf.area.height).flat_map(|y| (0..buf.area.width).map(move |x| (x, y))).map(|(x, y)| buf[(x, y)].symbol().to_string()).collect() };
        assert!(screen.contains("late") && screen.contains(" new "), "shown, the rule stays until the next load");
        assert!(!screen.contains(" 2 new "), "but no longer counted unread");
    }

    #[test]
    fn the_selection_follows_the_wheel_onto_the_screen() {
        let mut t = Threads::default();
        t.add_page((1..=12).map(|n| thread(n, false)).collect(), false);
        drawn(&mut t);
        let on_screen = |t: &Threads| -> Vec<(String, String)> {
            let mut v: Vec<(String, String)> = t.rows.iter().flatten().cloned().collect();
            v.dedup();
            v
        };
        // The wheel down: the selection (the first message) was above the view; the top message on screen now.
        t.scroll_by(30);
        drawn(&mut t);
        let shown = on_screen(&t);
        assert_eq!(t.selected.as_ref(), shown.first(), "the top one");
        // ↓ moves from there, the view stays.
        let visible = t.visible.clone();
        t.move_by(1);
        drawn(&mut t);
        assert_eq!((t.selected.as_ref(), t.visible.clone()), (shown.get(1), visible), "the next one");
        // The wheel up: the bottom one on screen.
        t.scroll_by(-30);
        drawn(&mut t);
        assert_eq!(t.selected.as_ref(), on_screen(&t).last(), "the bottom one");
        // A scroll leaving it on screen leaves it.
        let kept = t.selected.clone();
        t.scroll_by(1);
        drawn(&mut t);
        assert_eq!(t.selected, kept);
    }

    #[test]
    fn a_rewrap_keeps_the_selection_on_screen() {
        // Shorter or narrower (a resize): not a scroll, the view goes to the selection instead.
        let ws = Workspace::default();
        let media = Media::with(ratatui_image::picker::Picker::halfblocks());
        let mut t = Threads::default();
        t.add_page((1..=12).map(|n| thread(n, false)).collect(), false);
        let draw = |t: &mut Threads, width: u16, height: u16| {
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap().draw(|f| t.draw(f, f.area(), &Here::alone(&ws), true, &media)).unwrap();
        };
        draw(&mut t, 100, 30);
        while t.rows.iter().position(|r| r.as_ref() == t.selected.as_ref()).is_none_or(|row| row < 20) {
            t.move_by(1);
            draw(&mut t, 100, 30);
        }
        let sel = t.selected.clone();
        for (width, height) in [(100, 16), (50, 16)] {
            draw(&mut t, width, height);
            assert_eq!(t.selected, sel, "{width}×{height}: the same message");
            assert!(t.rows.contains(&sel), "{width}×{height}: on screen");
        }
    }

    #[test]
    fn growth_above_the_selection_never_moves_it() {
        // Not a scroll: a live reply, replies fetched, a pill row, an edit in a block above the selection.
        let mut t = Threads::default();
        t.add_page((1..=12).map(|n| thread(n, false)).collect(), false);
        drawn(&mut t);
        let row_of = |t: &Threads| t.rows.iter().position(|r| r.as_ref() == t.selected.as_ref());
        while row_of(&t).is_none_or(|row| row < 25) {
            t.move_by(1);
            drawn(&mut t);
        }
        let (sel, at) = (t.selected.clone(), row_of(&t));
        let check = |t: &mut Threads, what: &str| {
            drawn(t);
            assert_eq!(t.selected, sel, "{what}: the same message selected");
            assert_eq!(t.rows.iter().position(|r| r.as_ref() == t.selected.as_ref()), at, "{what}: on its row");
        };
        assert!(t.add_reply("C1", &reply(1, 3), true));
        check(&mut t, "a live reply above");
        t.merge_replies("C1", "2.000100", vec![reply(2, 4), reply(2, 5)]);
        check(&mut t, "replies fetched above");
        for m in t.copies_mut("C1", "1.000100") {
            m.reactions.push(slack_core::types::Reaction { name: "eyes".into(), users: vec!["U2".into()], count: 1 });
        }
        check(&mut t, "a pill row above");
        for m in t.copies_mut("C1", "2.000100") {
            m.text = "parent 2\nnow\nfour\nlines".into();
        }
        check(&mut t, "an edit above");
    }
}

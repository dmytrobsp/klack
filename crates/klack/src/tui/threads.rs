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
use super::messages::{Avatars, ImageSlot, LinkSpot};

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
        let root: Message = serde_json::from_value(v["root_msg"].clone()).ok()?;
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

/// The feed is built again this often at most while nothing in it changed (names, statuses and
/// pictures that arrived meanwhile show then).
const REBUILD: Duration = Duration::from_secs(1);

#[derive(Default)]
pub struct Threads {
    pub entries: Option<Vec<Entry>>,
    pub has_more: bool,
    /// A page is being loaded.
    pub loading: bool,
    /// Why the first page could not be loaded (said in the view).
    pub failed: Option<String>,
    /// The selected message (channel, ts); none: the first one.
    pub selected: Option<(String, String)>,
    /// Lines of the feed above the view.
    pub scroll: usize,
    /// Bring the selection into view at the next draw (it moved by the keys).
    follow: bool,
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
    /// The feed as last built, its links, and the width it was built for and when.
    feed: Feed,
    feed_links: Vec<LinkSpot>,
    built: Option<(u16, Instant)>,
    /// The threads changed since: build the feed again at the next draw.
    dirty: bool,
    /// The messages in feed order (for ↑ / ↓).
    order: Vec<(String, String)>,
    /// The feed's lines on screen at the last draw (empty while not drawn).
    pub visible: Range<usize>,
    /// When the blocks with unread replies have been on screen long enough.
    pub clock: ReadClock,
    /// Where links were drawn at the last draw: as in the message panes.
    links: Vec<(Rect, String, Option<AppLink>, bool)>,
    /// Pictures on screen but not loaded at the last draw.
    pub wanted: Vec<(File, (u16, u16))>,
}

impl Threads {
    /// Add a page, leaving out threads already listed.
    pub fn add_page(&mut self, page: Vec<Entry>, has_more: bool) {
        let entries = self.entries.get_or_insert_with(Vec::new);
        for e in page {
            if !entries.iter().any(|o| o.channel == e.channel && o.root.ts == e.root.ts) {
                entries.push(e);
            }
        }
        self.has_more = has_more;
        self.loading = false;
        self.dirty = true;
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
        self.follow = true;
    }

    /// Messages a page moves (those on screen at the last draw).
    pub fn page(&self) -> isize {
        let mut shown: Vec<&(String, String)> = self.rows.iter().flatten().collect();
        shown.dedup();
        shown.len().max(1) as isize
    }

    /// Scroll the feed by `delta` lines (the wheel), leaving the selection where it is.
    pub fn scroll_by(&mut self, delta: isize) {
        self.scroll = self.scroll.saturating_add_signed(delta);
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
        let e = self.entries.as_ref()?.iter().find(|e| e.channel == *channel && (e.root.ts == *ts || e.unread.iter().chain(&e.latest).any(|r| r.ts == *ts)))?;
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
        e.root.reply_count += 1;
        if let Some(u) = m.user.as_ref().filter(|u| !e.root.reply_users.contains(u)) {
            e.root.reply_users.push(u.clone());
        }
        e.root.latest_reply = Some(m.ts.clone());
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
        self.dirty = true;
        true
    }

    /// The threads whose blocks have been on screen long enough (`after`) at `now`: to mark read (each once).
    pub fn tick(&mut self, shown: bool, now: Instant, after: Duration) -> Vec<(String, String)> {
        let visible = if shown { self.visible.clone() } else { 0..0 };
        self.clock.tick(visible, &self.feed.blocks, now, after)
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

    /// Not drawn this time: nothing of it is under the mouse or on screen.
    pub fn hide(&mut self) {
        (self.area, self.list, self.hover, self.visible) = (Rect::default(), Rect::default(), None, 0..0);
        self.rows.clear();
        self.row_threads.clear();
        self.links.clear();
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
        self.wanted.clear();
        let border = Style::default().fg(if focused { p.accent } else { p.dim });
        let block = Block::default().borders(Borders::ALL).border_style(border).title(format!(" {} ", super::i18n::tr("Threads")));
        let inner = block.inner(area);
        f.render_widget(block, area);
        self.list = inner;
        let note = match &self.entries {
            None if self.failed.is_some() => Some(Line::styled(format!("{}: {}", super::i18n::tr("The threads could not be loaded"), self.failed.as_deref().unwrap_or_default()), Style::default().fg(p.danger))),
            None => Some(Line::styled("Loading…", Style::default().fg(p.dim))),
            Some(entries) if entries.is_empty() => Some(Line::styled("No threads yet: the threads you start, reply in or follow show up here.", Style::default().fg(p.dim))),
            Some(_) => None,
        };
        if let Some(note) = note {
            f.render_widget(Paragraph::new(note), inner);
            return;
        }
        let entries = self.entries.as_deref().unwrap_or_default();
        let stale = self.dirty || self.built.is_none_or(|(w, at)| w != inner.width || at.elapsed() >= REBUILD);
        if stale {
            let thumb_size = |file: &File, max_cols: u16| media.thumb_size(file, max_cols).filter(|_| ws.settings.messages.inline_images);
            let avatars = if !ws.settings.messages.avatars {
                Avatars::Off
            } else if media.avatars {
                Avatars::Pictures
            } else {
                Avatars::Initials
            };
            self.feed = super::feed::build(here, entries, inner.width, super::feed::REPLIES_SHOWN, &thumb_size, avatars);
            let messages: Vec<Message> = entries.iter().flat_map(|e| std::iter::once(&e.root).chain(&e.unread).chain(&e.latest)).cloned().collect();
            let owners: Vec<Option<String>> = self.feed.owners.iter().map(|o| o.as_ref().map(|(_, ts)| ts.clone())).collect();
            self.feed_links = super::messages::link_spots(here, &messages, &self.feed.lines, &owners);
            self.order = self.feed.owners.iter().flatten().cloned().collect();
            self.order.dedup();
            (self.built, self.dirty) = (Some((inner.width, Instant::now())), false);
        }
        if self.selected.as_ref().is_none_or(|s| !self.order.contains(s)) {
            self.selected = self.order.first().cloned();
        }
        let feed = &self.feed;
        let height = inner.height as usize;
        let more = if self.loading { 1 } else { 0 };
        let total = feed.lines.len() + more;
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
        let start = self.scroll;
        let end = (start + height).min(feed.lines.len());
        let mut lines: Vec<Line<'static>> = feed.lines[start..end].to_vec();
        let mut rows: Vec<Option<(String, String)>> = feed.owners[start..end].to_vec();
        if self.loading && lines.len() < height {
            lines.push(Line::styled("Loading more…", Style::default().fg(p.dim)));
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
        super::scroll::indicator(f, area, start, end - start, total, border);
        self.wanted = super::messages::draw_pictures(f, inner, slots, start, 0, media);
        self.row_threads = (start..start + rows.len()).map(|n| feed.blocks.iter().find(|b| b.rows.contains(&n)).map(|b| (b.channel.clone(), b.thread_ts.clone()))).collect();
        self.rows = rows;
        self.visible = start..end;
    }
}

/// The scroll (lines above the view) that shows lines `first..=last` of a top-anchored list: moved as little as it
/// takes; one taller than the view shows from its start.
fn bring_into_view(scroll: usize, height: usize, first: usize, last: usize) -> usize {
    if last + 1 - first > height || first < scroll {
        first
    } else if last >= scroll + height {
        last + 1 - height
    } else {
        scroll
    }
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
}

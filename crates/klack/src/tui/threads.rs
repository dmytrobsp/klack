//! Threads (Ctrl+T): every thread you are in, from `subscriptions.thread.getView` — unread
//! ones first, then by latest reply — each with its parent and its newest (or first unread)
//! reply; Enter opens the thread at that reply. More are loaded at the end of the list.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use serde_json::Value;
use slack_core::format;
use slack_core::types::Message;

use super::state::Workspace;
use super::text;

#[derive(Debug, Clone)]
pub struct Entry {
    pub channel: String,
    pub root: Message,
    /// Replies not read yet (oldest first).
    pub unread: Vec<Message>,
    /// The newest replies, when all are read.
    pub latest: Vec<Message>,
}

impl Entry {
    fn from_json(v: &Value) -> Option<Entry> {
        let root: Message = serde_json::from_value(v["root_msg"].clone()).ok()?;
        let replies = |k: &str| v[k].as_array().into_iter().flatten().filter_map(|m| serde_json::from_value(m.clone()).ok()).collect::<Vec<Message>>();
        Some(Entry { channel: v["root_msg"]["channel"].as_str()?.to_string(), root, unread: replies("unread_replies"), latest: replies("latest_replies") })
    }

    /// The reply shown under the parent and opened by Enter: the first unread, else the newest.
    pub fn reply(&self) -> Option<&Message> {
        self.unread.first().or(self.latest.last())
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

#[derive(Default)]
pub struct Threads {
    pub entries: Option<Vec<Entry>>,
    pub selected: usize,
    pub has_more: bool,
    /// A page is being loaded.
    pub loading: bool,
    /// Entries per page on the last draw.
    pub height: usize,
    /// Last drawn area, for clicks.
    pub area: Rect,
    /// The list's rows at the last draw, the first thread shown and how many are.
    pub list: Rect,
    pub first: usize,
    pub shown: usize,
    /// The row the pointer is over (not the selection), at the last `Moved`.
    pub hover: Option<usize>,
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
    }

    /// Where the next page starts: before the oldest latest reply listed.
    pub fn next_page(&self) -> Option<String> {
        let oldest = self.entries.as_ref()?.iter().map(Entry::latest_reply).fold(f64::INFINITY, f64::min);
        (self.has_more && !self.loading && oldest.is_finite()).then(|| format!("{oldest:.6}"))
    }

    pub fn move_by(&mut self, delta: isize) {
        let len = self.entries.as_ref().map_or(0, Vec::len);
        self.selected = if len == 0 { 0 } else { (self.selected as isize + delta).clamp(0, len as isize - 1) as usize };
    }

    /// The thread drawn at a terminal position (three rows and a gap each; the gap is no thread).
    pub fn row_at(&self, column: u16, row: u16) -> Option<usize> {
        let r = row.checked_sub(self.list.y)? as usize;
        (self.list.contains((column, row).into()) && r % 4 < 3).then(|| self.first + r / 4).filter(|n| *n < self.first + self.shown)
    }

    /// The pointer moved to a position: hover the thread there (none off the list).
    pub fn hover_at(&mut self, column: u16, row: u16) {
        self.hover = self.row_at(column, row);
    }

    /// At the end of the list (time to load more).
    pub fn at_end(&self) -> bool {
        self.entries.as_ref().is_some_and(|e| self.selected + 1 >= e.len())
    }

    /// Where Enter goes: (channel, the reply shown, the parent).
    pub fn target(&self) -> Option<(String, String, Option<String>)> {
        let e = self.entries.as_ref()?.get(self.selected)?;
        let ts = e.reply().map_or(e.root.ts.clone(), |r| r.ts.clone());
        Some((e.channel.clone(), ts, Some(e.root.ts.clone())))
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, ws: &Workspace) {
        let p = super::theme::p();
        let [popup] = Layout::horizontal([Constraint::Length(90.min(area.width))]).flex(Flex::Center).areas(area);
        let [popup] = Layout::vertical([Constraint::Length(30.min(area.height))]).flex(Flex::Center).areas(popup);
        self.area = popup;
        super::media::clear_modal(f, popup);
        let block = Block::default().borders(Borders::ALL).border_style(Style::default().fg(p.accent)).title(format!(" {} ", super::i18n::tr("Threads")));
        let inner = block.inner(popup);
        f.render_widget(block, popup);
        self.height = (inner.height / 4) as usize;
        let width = inner.width as usize;
        let mut marked = None;
        let lines = match &self.entries {
            None => vec![Line::styled("Loading…", Style::default().fg(p.dim))],
            Some(entries) if entries.is_empty() => vec![Line::styled("No threads yet: the threads you start, reply in or follow show up here.", Style::default().fg(p.dim))],
            Some(entries) => {
                let start = self.selected.saturating_sub(self.height.saturating_sub(1));
                let mut lines = Vec::new();
                for (n, e) in entries.iter().enumerate().skip(start).take(self.height) {
                    let unread = !e.unread.is_empty();
                    if n == self.selected {
                        marked = Some(Rect { y: inner.y + lines.len() as u16, height: 3, ..inner });
                    }
                    let mut rows = vec![headline(ws, e), quote(ws, &e.root, "", width, unread), e.reply().map_or_else(|| Line::raw(""), |r| quote(ws, r, "↳ ", width, unread)), Line::raw("")];
                    for l in rows.iter_mut().take(3) {
                        l.spans.insert(0, super::sidebar::unread_mark(unread));
                        if n == self.selected {
                            *l = std::mem::take(l).style(Style::default().bg(p.selection));
                        } else if self.hover == Some(n) {
                            *l = std::mem::take(l).style(super::theme::hover_style());
                        }
                    }
                    lines.extend(rows);
                }
                if self.loading {
                    lines.push(Line::styled("Loading more…", Style::default().fg(p.dim)));
                }
                lines
            }
        };
        // The selected thread's rows are filled edge to edge first; the text's own backgrounds draw over them.
        if let Some(rect) = marked {
            f.buffer_mut().set_style(rect.intersection(inner), Style::default().bg(p.selection));
        }
        f.render_widget(Paragraph::new(lines), inner);
        self.list = inner;
        (self.first, self.shown) = (0, 0);
        if let Some(entries) = &self.entries {
            let start = self.selected.saturating_sub(self.height.saturating_sub(1));
            (self.first, self.shown) = (start, entries.len().saturating_sub(start).min(self.height));
            super::scroll::indicator_beside(f, popup, inner, start, self.height, entries.len(), Style::default().fg(p.accent));
        }
    }
}

fn when(ws: &Workspace, t: f64) -> String {
    let Some(d) = super::clock::at(t as i64) else { return String::new() };
    let time = ws.settings.time_format();
    if d.date_naive() == super::clock::now().date_naive() { d.format(time).to_string() } else { d.format(&format!("%b %-d {time}")).to_string() }
}

/// "#general · 3 replies · 2 new  14:02".
pub fn headline(ws: &Workspace, e: &Entry) -> Line<'static> {
    let p = super::theme::p();
    let place = match ws.conversations.get(&e.channel) {
        Some(c) if c.is_dm() => ws.conversation_label(c),
        Some(c) => format!("#{}", c.name),
        None => e.channel.clone(),
    };
    let replies = e.root.reply_count;
    let unread = !e.unread.is_empty();
    let style = if unread { Style::default().add_modifier(Modifier::BOLD).fg(p.strong) } else { Style::default().fg(p.read) };
    let mut spans = vec![Span::styled(place, style), Span::styled(format!(" · {replies} {}", if replies == 1 { "reply" } else { "replies" }), Style::default().fg(p.dim))];
    if unread {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!(" {} new ", e.unread.len()), Style::default().fg(p.badge_fg).bg(p.badge_bg).add_modifier(Modifier::BOLD)));
    }
    spans.push(Span::styled(format!("  {}", when(ws, e.latest_reply())), Style::default().fg(p.dim)));
    Line::from(spans)
}

/// "   Bob: the message text" on one row (after the unread mark's column); the text a step
/// back when it was seen.
fn quote(ws: &Workspace, m: &Message, lead: &str, width: usize, unread: bool) -> Line<'static> {
    let p = super::theme::p();
    let plain = format::plain_text(&format::message_blocks(&m.text, &m.blocks), &|i| text::inline_plain(ws, i)).replace('\n', " ");
    let plain = if plain.is_empty() && !m.files.is_empty() { format!("[{}]", m.files[0].name) } else { plain };
    let author = format!("{}: ", ws.author(m));
    let room = width.saturating_sub(4 + lead.chars().count() + author.chars().count());
    let cut: String = if plain.chars().count() > room { plain.chars().take(room.saturating_sub(1)).chain(['…']).collect() } else { plain };
    let color = m.user.as_deref().map(text::user_color).unwrap_or(p.dim);
    let text = if unread { Style::default() } else { Style::default().fg(p.read) };
    Line::from(vec![Span::raw(format!("  {lead}")), Span::styled(author, Style::default().fg(color)), Span::styled(cut, text)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threads_opened_here_are_seen() {
        let msg = |ts: &str| Message { ts: ts.into(), ..Default::default() };
        let entry = |root: &str, unread: &[&str]| Entry { channel: "C1".into(), root: msg(root), unread: unread.iter().map(|t| msg(t)).collect(), latest: vec![] };
        let mut page = vec![entry("1.0", &["5.0", "9.0"]), entry("2.0", &["6.0"]), entry("3.0", &[])];
        let seen = Seen::from([("C1/1.0".to_string(), 7.0), ("C1/2.0".to_string(), 8.0)]);
        apply_seen(&mut page, &seen);
        let state: Vec<(&str, usize, usize)> = page.iter().map(|e| (e.root.ts.as_str(), e.unread.len(), e.latest.len())).collect();
        assert_eq!(state, [("1.0", 1, 1), ("2.0", 0, 1), ("3.0", 0, 0)], "a reply after the opening stays unread; a seen thread goes after the unread ones");
        assert_eq!(page[1].reply().map(|r| r.ts.as_str()), Some("6.0"), "its newest reply is still shown");
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
        assert_eq!(entries[0].reply().map(|r| r.text.as_str()), Some("a reply"), "the first unread reply");
        assert_eq!(entries[1].reply().map(|r| r.text.as_str()), Some("seven"), "all read: the newest");
        assert_eq!(entries[1].root.reply_count, 7);
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
        assert_eq!(t.target(), Some(("D1".into(), "1792349881.855209".into(), Some("1792349791.643499".into()))));
        assert!(!t.at_end());
        t.move_by(5);
        assert!(t.at_end());
        assert_eq!(t.target().unwrap().1, "1792325495.184089");
    }

    #[test]
    fn a_click_maps_to_the_thread_drawn_there() {
        // Four rows per thread (the last is a gap); thread 3 is the first shown, two are.
        let t = Threads { list: Rect::new(5, 10, 88, 28), first: 3, shown: 2, ..Default::default() };
        assert_eq!((t.row_at(6, 10), t.row_at(6, 12)), (Some(3), Some(3)));
        assert_eq!(t.row_at(6, 13), None, "the gap");
        assert_eq!(t.row_at(6, 14), Some(4));
        assert_eq!(t.row_at(6, 18), None, "below the last thread");
        assert_eq!(t.row_at(6, 9), None, "the border / title");
        assert_eq!(t.row_at(4, 10), None, "the side border");
    }

    #[test]
    fn the_selection_fills_the_whole_row() {
        let msg = |ts: &str| Message { ts: ts.into(), text: "hi".into(), ..Default::default() };
        let entry = |ts: &str| Entry { channel: "C1".into(), root: msg(ts), unread: vec![], latest: vec![msg("9.0")] };
        let mut t = Threads { selected: 0, ..Default::default() };
        t.add_page(vec![entry("1.0"), entry("2.0")], false);
        let ws = Workspace::default();
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 30)).unwrap();
        terminal.draw(|f| t.draw(f, f.area(), &ws)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let p = super::super::theme::p();
        let (x0, x1) = (t.list.x, t.list.right() - 1);
        for y in t.list.y..t.list.y + 3 {
            assert_eq!((buf[(x0, y)].bg, buf[(x1, y)].bg), (p.selection, p.selection), "row {y}: edge to edge");
        }
        for y in t.list.y + 3..t.list.y + 8 {
            assert!((x0..=x1).all(|x| buf[(x, y)].bg != p.selection), "row {y}: not the selected thread");
        }
        assert_ne!(buf[(x1 + 1, t.list.y)].bg, p.selection, "not the border");
    }

    #[test]
    fn hover_follows_the_pointer_and_leaves_the_selection() {
        let mut t = Threads { list: Rect::new(5, 10, 88, 28), first: 3, shown: 2, selected: 3, ..Default::default() };
        t.hover_at(6, 14);
        assert_eq!((t.hover, t.selected), (Some(4), 3));
        t.hover_at(6, 13);
        assert_eq!(t.hover, None, "the gap");
    }
}

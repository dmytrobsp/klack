//! Quick switcher (Ctrl+K): jump to any conversation, or open a DM with anyone.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::autocomplete::rank;
use super::state::Workspace;

/// At most this many entries (the list scrolls; ranking a few thousand is cheap).
const MAX: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Conversation(String),
    /// A person without an open DM: open one.
    User(String),
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub label: String,
    pub detail: String,
    pub target: Target,
    pub unread: bool,
}

#[derive(Debug, Default)]
pub struct Switcher {
    pub query: String,
    pub selected: usize,
    /// Rows of the list at the last draw (a page for PageUp / PageDown).
    pub height: usize,
    /// Last drawn area, for clicks.
    pub area: Rect,
    /// The list's rows at the last draw, the first entry shown and how many are.
    pub list: Rect,
    pub first: usize,
    pub shown: usize,
    /// The row the pointer is over (not the selection), at the last `Moved`.
    pub hover: Option<usize>,
}

impl Switcher {
    pub fn entries(&self, ws: &Workspace) -> Vec<Entry> {
        entries(ws, &self.query)
    }

    pub fn move_by(&mut self, delta: isize, len: usize) {
        self.selected = if len == 0 { 0 } else { (self.selected as isize + delta).clamp(0, len as isize - 1) as usize };
    }

    /// The entry drawn at a terminal position.
    pub fn row_at(&self, column: u16, row: u16) -> Option<usize> {
        self.list.contains((column, row).into()).then(|| self.first + (row - self.list.y) as usize).filter(|n| *n < self.first + self.shown)
    }

    /// The pointer moved to a position: hover the entry there (none off the list).
    pub fn hover_at(&mut self, column: u16, row: u16) {
        self.hover = self.row_at(column, row);
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, ws: &Workspace) {
        let [popup] = Layout::horizontal([Constraint::Length(60.min(area.width))]).flex(Flex::Center).areas(area);
        let [popup] = Layout::vertical([Constraint::Length(18.min(area.height))]).flex(Flex::Center).areas(popup);
        self.area = popup;
        super::media::clear_modal(f, popup);
        let items = self.entries(ws);
        let mut block = Block::default().borders(Borders::ALL).border_style(Style::default().fg(super::theme::p().accent)).title(format!(" {} ", super::i18n::tr("Jump to…")));
        let inner = block.inner(popup);
        let [input, list] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);
        self.height = list.height as usize;
        if items.len() > self.height {
            block = block.title_bottom(Line::styled(format!(" {} matches ", items.len()), Style::default().fg(super::theme::p().dim)).right_aligned());
        }
        f.render_widget(block, popup);
        f.render_widget(Paragraph::new(Line::from(vec![Span::styled("› ", Style::default().fg(super::theme::p().dim)), Span::raw(self.query.clone())])), input);
        f.set_cursor_position((input.x + 2 + self.query.chars().count() as u16, input.y));
        let height = list.height as usize;
        let start = self.selected.saturating_sub(height.saturating_sub(1));
        (self.list, self.first, self.shown) = (list, start, items.len().saturating_sub(start).min(height));
        let lines: Vec<Line> = items
            .iter()
            .enumerate()
            .skip(start)
            .take(height)
            .map(|(n, e)| {
                let style = if e.unread { Style::default().add_modifier(Modifier::BOLD) } else { Style::default() };
                let line = Line::from(vec![Span::styled(format!(" {}", e.label), style), Span::styled(format!("  {}", e.detail), Style::default().fg(super::theme::p().dim))]);
                if n == self.selected { line.style(Style::default().bg(super::theme::p().selection)) } else if self.hover == Some(n) { line.style(super::theme::hover_style()) } else { line }
            })
            .collect();
        let lines = if lines.is_empty() { vec![Line::styled(" nothing matches", Style::default().fg(super::theme::p().dim))] } else { lines };
        f.render_widget(Paragraph::new(lines), list);
        super::scroll::indicator_beside(f, popup, list, start, height, items.len(), Style::default().fg(super::theme::p().accent));
    }
}

/// Conversations in the sidebar, plus channels we are not in and people without a DM when
/// searching; a name equal to `query` first, then ranked against it (unread first when the query is empty), then by activity. A query
/// starting with `#` lists channels only, so `#` alone browses every channel.
pub fn entries(ws: &Workspace, query: &str) -> Vec<Entry> {
    let channels_only = query.trim_start().starts_with('#');
    let q = query.trim().trim_start_matches(['#', '@']).to_lowercase();
    let (channels, dms) = ws.listed();
    let mut out: Vec<(bool, u8, Entry)> = Vec::new();
    let exact = |names: &[&str]| !q.is_empty() && names.iter().any(|n| n.to_lowercase() == q);
    let mut dm_users = std::collections::HashSet::new();
    let dms = if channels_only { vec![] } else { dms };
    for id in channels.iter().chain(&dms) {
        let Some(c) = ws.conversations.get(id) else { continue };
        if let Some(u) = &c.user {
            dm_users.insert(u.clone());
        }
        let label = if c.is_dm() { ws.conversation_label(c) } else { format!("#{}", c.name) };
        let handle = c.user.as_deref().and_then(|u| ws.users.get(u)).map(|u| u.name.clone()).unwrap_or_default();
        let unread = ws.unread.get(id).is_some_and(|u| u.has_unreads || u.mentions > 0);
        let Some(r) = (if q.is_empty() { Some(if unread { 0 } else { 1 }) } else { rank(&q, &[label.trim_start_matches('#'), &handle]) }) else { continue };
        let detail = if c.is_im { "direct message".into() } else if c.is_mpim { "group message".into() } else if c.is_private { "private channel".into() } else { String::new() };
        out.push((!exact(&[label.trim_start_matches('#'), &handle]), r, Entry { label, detail, target: Target::Conversation(id.clone()), unread }));
    }
    if !q.is_empty() || channels_only {
        for c in ws.conversations.values().filter(|c| !c.is_dm() && !c.is_member && !c.is_archived && !channels.contains(&c.id)) {
            let Some(r) = (if q.is_empty() { Some(2) } else { rank(&q, &[&c.name]) }) else { continue };
            let members = c.num_members.map(|n| format!(" · {n} member{}", if n == 1 { "" } else { "s" })).unwrap_or_default();
            out.push((!exact(&[&c.name]), r + 2, Entry { label: format!("#{}", c.name), detail: format!("not a member{members}"), target: Target::Conversation(c.id.clone()), unread: false }));
        }
    }
    if !q.is_empty() && !channels_only {
        for u in ws.users.values().filter(|u| !u.deleted && !u.is_bot && u.id != ws.self_id && !dm_users.contains(&u.id)) {
            let real = u.real_name.clone().unwrap_or_default();
            if let Some(r) = rank(&q, &[u.display(), &u.name, &real]) {
                out.push((!exact(&[u.display(), &u.name, &real]), r + 3, Entry { label: u.display().to_string(), detail: format!("{} · new direct message", u.name), target: Target::User(u.id.clone()), unread: false }));
            }
        }
    }
    // As good a match: the most recently active first (as the sidebar lists DMs), then by name.
    let active = |e: &Entry| match &e.target {
        Target::Conversation(id) => ws.activity(id),
        Target::User(_) => 0.0,
    };
    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(active(&b.2).total_cmp(&active(&a.2))).then(a.2.label.to_lowercase().cmp(&b.2.label.to_lowercase())));
    out.into_iter().map(|(_, _, e)| e).take(MAX).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use slack_core::types::{Conversation, User};

    fn ws() -> Workspace {
        let mut w = Workspace { self_id: "UME".into(), ..Default::default() };
        for (id, name) in [("U2", "bob"), ("U3", "carol")] {
            w.users.insert(id.into(), User { id: id.into(), name: name.into(), ..Default::default() });
        }
        w.conversations.insert("C1".into(), Conversation { id: "C1".into(), name: "general".into(), is_channel: true, is_member: true, ..Default::default() });
        w.conversations.insert("C2".into(), Conversation { id: "C2".into(), name: "random".into(), is_channel: true, is_member: true, ..Default::default() });
        w.conversations.insert("D1".into(), Conversation { id: "D1".into(), is_im: true, user: Some("U2".into()), ..Default::default() });
        w.unread.insert("C2".into(), super::super::state::Unread { has_unreads: true, mentions: 0 });
        w
    }

    #[test]
    fn most_recently_active_first() {
        let mut w = ws();
        w.unread.clear();
        w.conversations.insert("C3".into(), Conversation { id: "C3".into(), name: "general-chat".into(), is_channel: true, is_member: true, ..Default::default() });
        w.note_activity("C1", 100.0);
        w.note_activity("D1", 300.0);
        w.note_activity("C3", 200.0);
        let labels = |w: &Workspace, q: &str| entries(w, q).into_iter().map(|e| e.label).collect::<Vec<_>>();
        assert_eq!(labels(&w, ""), ["bob", "#general-chat", "#general", "#random"], "by activity, never-active ones by name at the end");
        assert_eq!(labels(&w, "gen"), ["#general-chat", "#general"], "as good a match: the more recent first");
        w.unread.insert("C2".into(), super::super::state::Unread { has_unreads: true, mentions: 0 });
        assert_eq!(labels(&w, "")[0], "#random", "unread still first");
    }

    #[test]
    fn a_name_equal_to_the_query_comes_first() {
        let mut w = ws();
        w.unread.clear();
        w.conversations.insert("C3".into(), Conversation { id: "C3".into(), name: "random-123".into(), is_channel: true, is_member: true, ..Default::default() });
        w.conversations.insert("C4".into(), Conversation { id: "C4".into(), name: "random-old".into(), is_channel: true, ..Default::default() });
        w.users.insert("U4".into(), User { id: "U4".into(), name: "random".into(), real_name: Some("Rand Om".into()), ..Default::default() });
        w.users.insert("U5".into(), User { id: "U5".into(), name: "randomly".into(), ..Default::default() });
        w.note_activity("C2", 100.0);
        w.note_activity("C3", 900.0);
        let labels = |q: &str| entries(&w, q).into_iter().map(|e| e.label).collect::<Vec<_>>();
        assert_eq!(labels("random"), ["#random", "Rand Om", "#random-123", "#random-old", "randomly"], "the exact names (a person by handle too), then those starting with it as before");
        for q in ["#random", "@Random", "  RANDOM "] {
            assert_eq!(labels(q)[0], "#random", "{q}");
        }
        assert_eq!(labels("random-123")[0], "#random-123");
        assert_eq!(labels("rand")[0], "#random-123", "no exact name: ranked by activity as before");
    }

    #[test]
    fn empty_query_lists_unread_first() {
        let e = entries(&ws(), "");
        assert_eq!(e[0].target, Target::Conversation("C2".into()));
        assert_eq!(e.len(), 3, "people without DMs only appear when searching");
    }

    #[test]
    fn search_finds_channels_dms_and_new_people() {
        let w = ws();
        assert_eq!(entries(&w, "#gen")[0].target, Target::Conversation("C1".into()));
        assert_eq!(entries(&w, "bob")[0].target, Target::Conversation("D1".into()));
        let carol = entries(&w, "car");
        assert_eq!(carol[0].target, Target::User("U3".into()));
    }

    #[test]
    fn channels_we_are_not_in() {
        let mut w = ws();
        w.conversations.insert("C3".into(), Conversation { id: "C3".into(), name: "design".into(), is_channel: true, num_members: Some(4), ..Default::default() });
        assert!(entries(&w, "").iter().all(|e| e.target != Target::Conversation("C3".into())), "not listed without a query");
        let found = entries(&w, "desi");
        assert_eq!((found[0].label.as_str(), found[0].detail.as_str()), ("#design", "not a member · 4 members"));
        let browse: Vec<_> = entries(&w, "#").into_iter().map(|e| e.label).collect();
        assert_eq!(browse, vec!["#random", "#general", "#design"], "channels only: unread, members, then the rest");
        assert!(entries(&w, "#bob").is_empty(), "no people or DMs after #");
    }

    #[test]
    fn long_lists_are_not_cut_at_fifty() {
        let mut w = ws();
        for i in 0..300 {
            let id = format!("C{i:03}x");
            w.conversations.insert(id.clone(), Conversation { id, name: format!("chan-{i:03}"), is_channel: true, is_member: true, ..Default::default() });
        }
        assert_eq!(entries(&w, "chan").len(), 300, "every match, the list scrolls");
        let mut sw = Switcher { query: "chan".into(), height: 10, ..Default::default() };
        sw.move_by(10, 300);
        sw.move_by(10_000, 300);
        assert_eq!(sw.selected, 299);
    }

    #[test]
    fn a_click_maps_to_the_entry_drawn_there() {
        // Border at y 4, query at 5, list rows 6..=10; scrolled so entry 20 is the first shown.
        let sw = Switcher { list: Rect::new(10, 6, 40, 5), first: 20, shown: 5, ..Default::default() };
        assert_eq!(sw.row_at(12, 6), Some(20));
        assert_eq!(sw.row_at(12, 10), Some(24));
        assert_eq!(sw.row_at(12, 5), None, "the query line");
        assert_eq!(sw.row_at(12, 4), None, "the border / title");
        assert_eq!(sw.row_at(12, 11), None, "the bottom border");
        assert_eq!(sw.row_at(9, 7), None, "the side border");
        let short = Switcher { list: Rect::new(10, 6, 40, 5), first: 0, shown: 2, ..Default::default() };
        assert_eq!(short.row_at(12, 7), Some(1));
        assert_eq!(short.row_at(12, 8), None, "below the last entry");
    }

    #[test]
    fn hover_follows_the_pointer_and_leaves_the_selection() {
        let mut sw = Switcher { list: Rect::new(10, 6, 40, 5), first: 20, shown: 5, selected: 22, ..Default::default() };
        sw.hover_at(12, 8);
        assert_eq!((sw.hover, sw.selected), (Some(22), 22));
        sw.hover_at(12, 9);
        assert_eq!((sw.hover, sw.selected), (Some(23), 22));
        sw.hover_at(12, 5);
        assert_eq!(sw.hover, None, "off the list");
    }

    #[test]
    fn the_hovered_entry_is_shaded_and_the_selection_stays() {
        let w = ws();
        let mut sw = Switcher::default();
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 30)).unwrap();
        terminal.draw(|f| sw.draw(f, f.area(), &w)).unwrap();
        let list = sw.list;
        assert!(sw.shown >= 2);
        sw.hover_at(list.x + 2, list.y + 1);
        assert_eq!(sw.hover, Some(1));
        terminal.draw(|f| sw.draw(f, f.area(), &w)).unwrap();
        let bg = |y: u16| terminal.backend().buffer()[(list.x + 2, y)].bg;
        let p = crate::tui::theme::p();
        assert_eq!((bg(list.y), bg(list.y + 1)), (p.selection, p.hover));
    }
}

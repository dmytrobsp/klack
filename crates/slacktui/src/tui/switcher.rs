//! Quick switcher (Ctrl+K): jump to any conversation, or open a DM with anyone.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use super::autocomplete::rank;
use super::state::Workspace;

const MAX: usize = 50;

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
}

impl Switcher {
    pub fn entries(&self, ws: &Workspace) -> Vec<Entry> {
        entries(ws, &self.query)
    }

    pub fn move_by(&mut self, delta: isize, len: usize) {
        self.selected = if len == 0 { 0 } else { (self.selected as isize + delta).clamp(0, len as isize - 1) as usize };
    }

    pub fn draw(&self, f: &mut Frame, area: Rect, ws: &Workspace) {
        let [popup] = Layout::horizontal([Constraint::Length(60.min(area.width))]).flex(Flex::Center).areas(area);
        let [popup] = Layout::vertical([Constraint::Length(18.min(area.height))]).flex(Flex::Center).areas(popup);
        f.render_widget(Clear, popup);
        let block = Block::default().borders(Borders::ALL).border_style(Style::default().fg(super::theme::p().accent)).title(format!(" {} ", super::i18n::tr("Jump to…")));
        let inner = block.inner(popup);
        f.render_widget(block, popup);
        let [input, list] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);
        f.render_widget(Paragraph::new(Line::from(vec![Span::styled("› ", Style::default().fg(super::theme::p().dim)), Span::raw(self.query.clone())])), input);
        f.set_cursor_position((input.x + 2 + self.query.chars().count() as u16, input.y));
        let items = self.entries(ws);
        let height = list.height as usize;
        let start = self.selected.saturating_sub(height.saturating_sub(1));
        let lines: Vec<Line> = items
            .iter()
            .enumerate()
            .skip(start)
            .take(height)
            .map(|(n, e)| {
                let style = if e.unread { Style::default().add_modifier(Modifier::BOLD) } else { Style::default() };
                let line = Line::from(vec![Span::styled(format!(" {}", e.label), style), Span::styled(format!("  {}", e.detail), Style::default().fg(super::theme::p().dim))]);
                if n == self.selected { line.style(Style::default().bg(super::theme::p().selection)) } else { line }
            })
            .collect();
        let lines = if lines.is_empty() { vec![Line::styled(" nothing matches", Style::default().fg(super::theme::p().dim))] } else { lines };
        f.render_widget(Paragraph::new(lines), list);
    }
}

/// Conversations in the sidebar, plus channels we are not in and people without a DM when
/// searching; ranked against `query` (unread first when the query is empty). A query
/// starting with `#` lists channels only, so `#` alone browses every channel.
pub fn entries(ws: &Workspace, query: &str) -> Vec<Entry> {
    let channels_only = query.trim_start().starts_with('#');
    let q = query.trim().trim_start_matches(['#', '@']).to_lowercase();
    let (channels, dms) = ws.sidebar();
    let mut out: Vec<(u8, Entry)> = Vec::new();
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
        out.push((r, Entry { label, detail, target: Target::Conversation(id.clone()), unread }));
    }
    if !q.is_empty() || channels_only {
        for c in ws.conversations.values().filter(|c| !c.is_dm() && !c.is_member && !c.is_archived && !channels.contains(&c.id)) {
            let Some(r) = (if q.is_empty() { Some(2) } else { rank(&q, &[&c.name]) }) else { continue };
            let members = c.num_members.map(|n| format!(" · {n} member{}", if n == 1 { "" } else { "s" })).unwrap_or_default();
            out.push((r + 2, Entry { label: format!("#{}", c.name), detail: format!("not a member{members}"), target: Target::Conversation(c.id.clone()), unread: false }));
        }
    }
    if !q.is_empty() && !channels_only {
        for u in ws.users.values().filter(|u| !u.deleted && !u.is_bot && u.id != ws.self_id && !dm_users.contains(&u.id)) {
            let real = u.real_name.clone().unwrap_or_default();
            if let Some(r) = rank(&q, &[u.display(), &u.name, &real]) {
                out.push((r + 3, Entry { label: u.display().to_string(), detail: format!("{} · new direct message", u.name), target: Target::User(u.id.clone()), unread: false }));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.label.to_lowercase().cmp(&b.1.label.to_lowercase())));
    out.into_iter().map(|(_, e)| e).take(MAX).collect()
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
}

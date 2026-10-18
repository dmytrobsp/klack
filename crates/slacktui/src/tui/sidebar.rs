//! Left-hand conversation list: channels, then direct messages.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::state::Workspace;
use super::text::DIM;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Header(&'static str),
    Conversation(String),
}

#[derive(Debug, Default)]
pub struct Sidebar {
    pub items: Vec<Item>,
    /// Index into `items` (always a conversation when any exist).
    pub selected: usize,
    /// First visible row.
    pub offset: usize,
    /// Last drawn area, for mouse hit-testing.
    pub area: Rect,
}

impl Sidebar {
    /// Rebuild the item list from the workspace, keeping the selected conversation.
    pub fn rebuild(&mut self, ws: &Workspace) {
        let keep = self.selected_id().map(String::from);
        let (channels, dms) = ws.sidebar();
        self.items.clear();
        self.items.push(Item::Header("Channels"));
        self.items.extend(channels.into_iter().map(Item::Conversation));
        self.items.push(Item::Header("Direct messages"));
        self.items.extend(dms.into_iter().map(Item::Conversation));
        self.selected = keep
            .and_then(|id| self.items.iter().position(|i| *i == Item::Conversation(id.clone())))
            .or_else(|| self.items.iter().position(|i| matches!(i, Item::Conversation(_))))
            .unwrap_or(0);
    }

    pub fn selected_id(&self) -> Option<&str> {
        match self.items.get(self.selected) {
            Some(Item::Conversation(id)) => Some(id),
            _ => None,
        }
    }

    /// Move the selection by `delta` conversations, skipping headers.
    pub fn move_by(&mut self, delta: isize) {
        let convs: Vec<usize> = self.items.iter().enumerate().filter(|(_, i)| matches!(i, Item::Conversation(_))).map(|(n, _)| n).collect();
        if convs.is_empty() {
            return;
        }
        let pos = convs.iter().position(|&n| n == self.selected).unwrap_or(0) as isize;
        let next = (pos + delta).clamp(0, convs.len() as isize - 1) as usize;
        self.selected = convs[next];
    }

    pub fn select_id(&mut self, id: &str) {
        if let Some(n) = self.items.iter().position(|i| *i == Item::Conversation(id.to_string())) {
            self.selected = n;
        }
    }

    /// Select the conversation drawn at terminal row `row`; true if one was hit.
    pub fn click(&mut self, column: u16, row: u16) -> bool {
        let inner = self.area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
        if !inner.contains((column, row).into()) {
            return false;
        }
        let n = self.offset + (row - inner.y) as usize;
        if matches!(self.items.get(n), Some(Item::Conversation(_))) {
            self.selected = n;
            return true;
        }
        false
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, ws: &Workspace, focused: bool) {
        self.area = area;
        let height = area.height.saturating_sub(2) as usize;
        if self.selected < self.offset {
            self.offset = self.selected.saturating_sub(1);
        } else if height > 0 && self.selected >= self.offset + height {
            self.offset = self.selected + 1 - height;
        }
        let width = area.width.saturating_sub(2) as usize;
        let lines: Vec<Line> = self
            .items
            .iter()
            .enumerate()
            .skip(self.offset)
            .take(height)
            .map(|(n, item)| match item {
                Item::Header(h) => Line::styled(format!(" {h}"), Style::default().fg(DIM).add_modifier(Modifier::BOLD)),
                Item::Conversation(id) => conversation_line(ws, id, n == self.selected, width),
            })
            .collect();
        let border = if focused { Style::default().fg(Color::Rgb(0x61, 0xaf, 0xef)) } else { Style::default().fg(DIM) };
        let title = format!(" {} ", ws.team_name);
        f.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::ALL).border_style(border).title(title)), area);
    }
}

fn conversation_line(ws: &Workspace, id: &str, selected: bool, width: usize) -> Line<'static> {
    let Some(c) = ws.conversations.get(id) else { return Line::raw(id.to_string()) };
    let unread = ws.unread.get(id).cloned().unwrap_or_default();
    let muted = ws.muted.contains(id);
    let sigil = if c.is_im {
        let online = c.user.as_deref().and_then(|u| ws.presence.get(u)).copied().unwrap_or(false);
        Span::styled(if online { "● " } else { "○ " }, Style::default().fg(if online { Color::Green } else { DIM }))
    } else if c.is_mpim {
        Span::styled("◎ ", Style::default().fg(DIM))
    } else if c.is_private {
        Span::styled("🔒", Style::default().fg(DIM))
    } else {
        Span::styled("# ", Style::default().fg(DIM))
    };
    let mut name_style = Style::default();
    if unread.has_unreads && !muted {
        name_style = name_style.add_modifier(Modifier::BOLD).fg(Color::White);
    } else if muted {
        name_style = name_style.fg(DIM);
    }
    let badge = if unread.mentions > 0 { format!(" {}", unread.mentions) } else { String::new() };
    let huddle = ws.huddles.get(id).map(|(_, p)| format!(" 🎧{}", p.len())).unwrap_or_default();
    let preview = if !c.is_dm() && !c.is_member { " ·preview" } else { "" };
    let label = ws.conversation_label(c);
    let room = width.saturating_sub(3 + badge.chars().count() + huddle.chars().count() + preview.chars().count() + 1);
    let label: String = if label.chars().count() > room { label.chars().take(room.saturating_sub(1)).chain(['…']).collect() } else { label };
    let mut spans = vec![Span::raw(" "), sigil, Span::styled(label, name_style)];
    if !preview.is_empty() {
        spans.push(Span::styled(preview, Style::default().fg(DIM)));
    }
    if !huddle.is_empty() {
        spans.push(Span::styled(huddle, Style::default().fg(Color::Rgb(0x2b, 0xac, 0x76))));
    }
    if !badge.is_empty() {
        spans.push(Span::styled(badge, Style::default().fg(Color::White).bg(Color::Rgb(0xe0, 0x1e, 0x5a)).add_modifier(Modifier::BOLD)));
    }
    let mut line = Line::from(spans);
    if selected {
        line = line.style(Style::default().bg(Color::Rgb(0x2c, 0x3e, 0x5c)));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use slack_core::types::Conversation;

    fn ws() -> Workspace {
        let mut ws = Workspace::default();
        for (id, name, im) in [("C1", "general", false), ("C2", "random", false), ("D1", "", true)] {
            ws.conversations.insert(
                id.into(),
                Conversation { id: id.into(), name: name.into(), is_channel: !im, is_im: im, is_member: true, user: im.then(|| "U1".into()), ..Default::default() },
            );
        }
        ws
    }

    #[test]
    fn navigation_skips_headers() {
        let mut s = Sidebar::default();
        s.rebuild(&ws());
        assert_eq!(s.selected_id(), Some("C1"));
        s.move_by(1);
        assert_eq!(s.selected_id(), Some("C2"));
        s.move_by(1);
        assert_eq!(s.selected_id(), Some("D1"));
        s.move_by(5);
        assert_eq!(s.selected_id(), Some("D1"));
        s.move_by(-10);
        assert_eq!(s.selected_id(), Some("C1"));
    }

    #[test]
    fn rebuild_keeps_selection() {
        let mut s = Sidebar::default();
        let w = ws();
        s.rebuild(&w);
        s.select_id("D1");
        s.rebuild(&w);
        assert_eq!(s.selected_id(), Some("D1"));
    }
}

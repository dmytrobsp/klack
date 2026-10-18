//! Left-hand conversation list: channels, then direct messages.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::state::Workspace;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Header(String),
    Conversation(String),
    /// A blank row between sections.
    Gap,
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
    /// Item under the mouse pointer.
    pub hover: Option<usize>,
    /// Conversations with a draft kept (marked ✎).
    pub drafts: std::collections::HashSet<String>,
}

impl Sidebar {
    /// Rebuild the item list from the workspace, keeping the selected conversation.
    pub fn rebuild(&mut self, ws: &Workspace) {
        let keep = self.selected_id().map(String::from);
        self.items.clear();
        for (header, ids) in ws.sidebar_groups() {
            // Sections stand apart: a blank row before each (none in compact mode).
            if !self.items.is_empty() && !ws.settings.appearance.compact {
                self.items.push(Item::Gap);
            }
            self.items.push(Item::Header(header));
            self.items.extend(ids.into_iter().map(Item::Conversation));
        }
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

    /// The conversation item drawn at a terminal position.
    pub fn item_at(&self, column: u16, row: u16) -> Option<usize> {
        let inner = self.area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
        if !inner.contains((column, row).into()) {
            return None;
        }
        let n = self.offset + (row - inner.y) as usize;
        matches!(self.items.get(n), Some(Item::Conversation(_))).then_some(n)
    }

    /// Select the conversation drawn at terminal row `row`; true if one was hit.
    pub fn click(&mut self, column: u16, row: u16) -> bool {
        match self.item_at(column, row) {
            Some(n) => {
                self.selected = n;
                true
            }
            None => false,
        }
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, ws: &Workspace, focused: bool) {
        self.area = area;
        let height = area.height.saturating_sub(2) as usize;
        if self.selected < self.offset {
            // With the section's header (and the gap above it) when it is the first.
            let above = self.items[..self.selected].iter().rev().take_while(|i| !matches!(i, Item::Conversation(_))).count();
            self.offset = self.selected.saturating_sub(above.max(1));
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
                Item::Header(h) => header_line(h, width),
                Item::Gap => Line::raw(""),
                Item::Conversation(id) if n != self.selected && self.hover == Some(n) => conversation_line(ws, id, false, self.drafts.contains(id), width).style(Style::default().bg(super::theme::p().hover)),
                Item::Conversation(id) => conversation_line(ws, id, n == self.selected, self.drafts.contains(id), width),
            })
            .collect();
        let border = if focused { Style::default().fg(super::theme::p().accent) } else { Style::default().fg(super::theme::p().dim) };
        let title = format!(" {} ", ws.team_name);
        f.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::ALL).border_style(border).title(title)), area);
        super::scroll::indicator(f, area, self.offset, height.min(self.items.len().saturating_sub(self.offset)), self.items.len(), border);
    }
}

/// A section's header: its name, then a rule to the edge (" Channels ────────").
fn header_line(name: &str, width: usize) -> Line<'static> {
    let label = format!(" {name} ");
    let rule = width.saturating_sub(unicode_width::UnicodeWidthStr::width(label.as_str()));
    Line::from(vec![
        Span::styled(label, Style::default().fg(super::theme::p().accent).add_modifier(Modifier::BOLD)),
        Span::styled("─".repeat(rule), Style::default().fg(super::theme::p().dim)),
    ])
}

fn conversation_line(ws: &Workspace, id: &str, selected: bool, draft: bool, width: usize) -> Line<'static> {
    let Some(c) = ws.conversations.get(id) else { return Line::raw(id.to_string()) };
    let unread = ws.unread.get(id).cloned().unwrap_or_default();
    let muted = ws.muted.contains(id);
    let sigil = if c.is_im {
        let online = c.user.as_deref().and_then(|u| ws.presence.get(u)).copied().unwrap_or(false);
        Span::styled(if online { "● " } else { "○ " }, Style::default().fg(if online { super::theme::p().good } else { super::theme::p().dim }))
    } else if c.is_mpim {
        Span::styled("◎ ", Style::default().fg(super::theme::p().dim))
    } else if c.is_private {
        Span::styled("🔒", Style::default().fg(super::theme::p().dim))
    } else {
        Span::styled("# ", Style::default().fg(super::theme::p().dim))
    };
    let mut name_style = Style::default();
    if unread.has_unreads && !muted {
        name_style = name_style.add_modifier(Modifier::BOLD).fg(super::theme::p().strong);
    } else if muted {
        name_style = name_style.fg(super::theme::p().dim);
    }
    let badge = if unread.mentions > 0 { format!(" {}", unread.mentions) } else { String::new() };
    let huddle = ws.huddles.get(id).map(|h| format!(" 🎧{}", h.participants.len())).unwrap_or_default();
    let preview = if !c.is_dm() && !c.is_member {
        " ·preview"
    } else if c.is_im && c.user.as_ref().is_some_and(|u| ws.vip.contains(u)) {
        " VIP"
    } else {
        ""
    };
    let label = ws.conversation_label(c);
    let draft = if draft { " ✎" } else { "" };
    let room = width.saturating_sub(3 + badge.chars().count() + huddle.chars().count() + preview.chars().count() + draft.chars().count() + 1);
    let label: String = if label.chars().count() > room { label.chars().take(room.saturating_sub(1)).chain(['…']).collect() } else { label };
    let mut spans = vec![Span::raw(" "), sigil, Span::styled(label, name_style)];
    if !draft.is_empty() {
        spans.push(Span::styled(draft, Style::default().fg(super::theme::p().dim)));
    }
    if !preview.is_empty() {
        spans.push(Span::styled(preview, Style::default().fg(super::theme::p().dim)));
    }
    if !huddle.is_empty() {
        spans.push(Span::styled(huddle, Style::default().fg(super::theme::p().own)));
    }
    if !badge.is_empty() {
        spans.push(Span::styled(badge, Style::default().fg(super::theme::p().badge_fg).bg(super::theme::p().badge_bg).add_modifier(Modifier::BOLD)));
    }
    let mut line = Line::from(spans);
    if selected {
        line = line.style(Style::default().bg(super::theme::p().selection));
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
    fn draft_marker() {
        let w = ws();
        let text = |l: Line| l.spans.iter().map(|s| s.content.to_string()).collect::<String>();
        assert_eq!(text(conversation_line(&w, "C1", false, true, 30)), " # general ✎");
        assert_eq!(text(conversation_line(&w, "C1", false, false, 30)), " # general");
        // A long name makes room for the marker.
        let line = text(conversation_line(&w, "C2", false, true, 10));
        assert!(line.ends_with(" ✎") && line.contains('…'), "{line:?}");
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
    fn sections_stand_apart() {
        let mut s = Sidebar::default();
        let mut w = ws();
        s.rebuild(&w);
        let kinds: Vec<String> = s.items.iter().map(|i| match i {
            Item::Header(h) => format!("[{h}]"),
            Item::Conversation(id) => id.clone(),
            Item::Gap => "".into(),
        }).collect();
        assert_eq!(kinds, ["[Channels]", "C1", "C2", "", "[Direct messages]", "D1"], "a gap before every section but the first");
        w.settings.appearance.compact = true;
        s.rebuild(&w);
        assert!(!s.items.contains(&Item::Gap), "compact: no gaps");
        let text = |l: Line| l.spans.iter().map(|s| s.content.to_string()).collect::<String>();
        assert_eq!(text(header_line("Channels", 20)), " Channels ──────────");
        assert_eq!(text(header_line("A very long section name", 10)), " A very long section name ", "no rule when there is no room");
        // Scrolling up to a section's first conversation shows its header and the gap above.
        s.rebuild(&ws());
        s.select_id("D1");
        s.offset = 6;
        let backend = ratatui::backend::TestBackend::new(30, 5);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| s.draw(f, f.area(), &ws(), false)).unwrap();
        assert_eq!(s.offset, 3);
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

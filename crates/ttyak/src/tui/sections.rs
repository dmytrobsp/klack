//! "Move to section" picker (`m` in the sidebar): no section, one of yours, or a new one.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::state::Workspace;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    /// Back to Channels / Direct messages.
    NoSection,
    Section { id: String, name: String },
    New,
}

pub struct MovePicker {
    pub channel: String,
    pub choices: Vec<Choice>,
    pub selected: usize,
    /// Typing the new section's name.
    pub naming: Option<String>,
    /// Last drawn area, for clicks.
    pub area: Rect,
    /// The choice rows at the last draw.
    pub list: Rect,
    /// The row the pointer is over (not the selection), at the last `Moved`.
    pub hover: Option<usize>,
}

impl MovePicker {
    pub fn new(ws: &Workspace, channel: String) -> MovePicker {
        let current = ws.section_of(&channel).map(|s| s.id.clone());
        let mut choices = vec![Choice::NoSection];
        choices.extend(ws.sections.iter().filter(|s| s.kind == "standard").map(|s| Choice::Section { id: s.id.clone(), name: s.name.clone() }));
        choices.push(Choice::New);
        let selected = choices.iter().position(|c| matches!(c, Choice::Section { id, .. } if Some(id) == current.as_ref())).unwrap_or(0);
        MovePicker { channel, choices, selected, naming: None, area: Rect::default(), list: Rect::default(), hover: None }
    }

    pub fn move_by(&mut self, delta: isize) {
        self.selected = (self.selected as isize + delta).clamp(0, self.choices.len() as isize - 1) as usize;
    }

    /// The choice drawn at a terminal position (none while naming a section).
    pub fn row_at(&self, column: u16, row: u16) -> Option<usize> {
        (self.naming.is_none() && self.list.contains((column, row).into())).then(|| (row - self.list.y) as usize).filter(|n| *n < self.choices.len())
    }

    /// The pointer moved to a position: hover the choice there (none off the list).
    pub fn hover_at(&mut self, column: u16, row: u16) {
        self.hover = self.row_at(column, row);
    }

    pub fn choice(&self) -> &Choice {
        &self.choices[self.selected]
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, ws: &Workspace) {
        let rows = self.choices.len() as u16 + 2 + u16::from(self.naming.is_some());
        let [popup] = Layout::horizontal([Constraint::Length(44.min(area.width))]).flex(Flex::Center).areas(area);
        let [popup] = Layout::vertical([Constraint::Length(rows.min(area.height))]).flex(Flex::Center).areas(popup);
        self.area = popup;
        super::media::clear_modal(f, popup);
        let c = ws.conversations.get(&self.channel);
        let name = c.map(|c| if c.is_dm() { ws.conversation_label(c) } else { format!("#{}", c.name) }).unwrap_or_default();
        let current = ws.section_of(&self.channel).map(|s| s.id.as_str());
        let default = if c.is_some_and(|c| c.is_dm()) { "Direct messages" } else { "Channels" };
        let mut lines: Vec<Line> = self
            .choices
            .iter()
            .enumerate()
            .map(|(n, choice)| {
                let (label, here) = match choice {
                    Choice::NoSection => (format!(" {default} (no section)"), current.is_none()),
                    Choice::Section { id, name } => (format!(" {name}"), current == Some(id.as_str())),
                    Choice::New => (" + New section…".to_string(), false),
                };
                let mut spans = vec![Span::raw(label)];
                if here {
                    spans.push(Span::styled("  ✓", Style::default().fg(super::theme::p().good)));
                }
                let line = Line::from(spans);
                if n == self.selected { line.style(Style::default().bg(super::theme::p().selection)) } else if self.hover == Some(n) { line.style(super::theme::hover_style()) } else { line }
            })
            .collect();
        if let Some(text) = &self.naming {
            lines.push(Line::from(vec![Span::styled(" name › ", Style::default().fg(super::theme::p().dim)), Span::styled(text.clone(), Style::default().add_modifier(Modifier::BOLD))]));
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(super::theme::p().accent))
            .title(format!(" Move {name} to… "));
        let inner = block.inner(popup);
        self.list = Rect { height: (self.choices.len() as u16).min(inner.height), ..inner };
        f.render_widget(Paragraph::new(lines).block(block), popup);
        if let Some(text) = &self.naming {
            f.set_cursor_position((inner.x + 8 + text.chars().count() as u16, inner.y + self.choices.len() as u16));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::state::Section;

    #[test]
    fn choices_start_at_the_current_section() {
        let mut ws = Workspace::default();
        ws.sections = vec![
            Section { id: "L1".into(), kind: "channels".into(), ..Default::default() },
            Section { id: "L2".into(), kind: "standard".into(), name: "Work".into(), channels: vec!["C1".into()], ..Default::default() },
            Section { id: "L3".into(), kind: "standard".into(), name: "Fun".into(), ..Default::default() },
        ];
        let p = MovePicker::new(&ws, "C1".into());
        assert_eq!(p.choices.len(), 4, "no section, Work, Fun, new");
        assert_eq!(p.choice(), &Choice::Section { id: "L2".into(), name: "Work".into() });
        assert_eq!(MovePicker::new(&ws, "C9".into()).choice(), &Choice::NoSection);
    }

    #[test]
    fn hover_follows_the_pointer_and_leaves_the_selection() {
        let mut ws = Workspace::default();
        ws.sections = vec![Section { id: "L2".into(), kind: "standard".into(), name: "Work".into(), channels: vec!["C1".into()], ..Default::default() }];
        let mut p = MovePicker::new(&ws, "C1".into());
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 30)).unwrap();
        terminal.draw(|f| p.draw(f, f.area(), &ws)).unwrap();
        let (x, y) = (p.list.x + 2, p.list.y);
        let selected = p.selected;
        p.hover_at(x, y + 2);
        assert_eq!((p.hover, p.selected), (Some(2), selected));
        p.hover_at(x, y + 3);
        assert_eq!(p.hover, None, "below the last choice");
        p.naming = Some(String::new());
        p.hover_at(x, y);
        assert_eq!(p.hover, None, "no hover while naming");
    }
}

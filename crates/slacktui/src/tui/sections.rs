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
}

impl MovePicker {
    pub fn new(ws: &Workspace, channel: String) -> MovePicker {
        let current = ws.section_of(&channel).map(|s| s.id.clone());
        let mut choices = vec![Choice::NoSection];
        choices.extend(ws.sections.iter().filter(|s| s.kind == "standard").map(|s| Choice::Section { id: s.id.clone(), name: s.name.clone() }));
        choices.push(Choice::New);
        let selected = choices.iter().position(|c| matches!(c, Choice::Section { id, .. } if Some(id) == current.as_ref())).unwrap_or(0);
        MovePicker { channel, choices, selected, naming: None }
    }

    pub fn move_by(&mut self, delta: isize) {
        self.selected = (self.selected as isize + delta).clamp(0, self.choices.len() as isize - 1) as usize;
    }

    pub fn choice(&self) -> &Choice {
        &self.choices[self.selected]
    }

    pub fn draw(&self, f: &mut Frame, area: Rect, ws: &Workspace) {
        let rows = self.choices.len() as u16 + 2 + u16::from(self.naming.is_some());
        let [popup] = Layout::horizontal([Constraint::Length(44.min(area.width))]).flex(Flex::Center).areas(area);
        let [popup] = Layout::vertical([Constraint::Length(rows.min(area.height))]).flex(Flex::Center).areas(popup);
        super::media::clear_popup(f, popup);
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
                if n == self.selected { line.style(Style::default().bg(super::theme::p().selection)) } else { line }
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
}

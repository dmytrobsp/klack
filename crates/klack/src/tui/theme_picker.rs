//! The theme picker (Enter on a theme in the preferences): type to filter, ↑/↓ show each
//! theme on the whole screen at once, Enter keeps it, Esc goes back to the one before.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

/// The workspace's entry for "no theme of its own".
pub const COLOUR_THEME: &str = "(colour theme)";

#[derive(Debug, Default)]
pub struct ThemePicker {
    /// Picking the workspace's own theme ([`COLOUR_THEME`] first) rather than the colour theme.
    pub workspace: bool,
    names: Vec<String>,
    /// What has been typed: themes whose name contains it (ignoring case).
    pub filter: String,
    /// Index among the shown themes.
    pub selected: usize,
    /// Why the highlighted theme cannot be shown.
    pub problem: Option<String>,
}

impl ThemePicker {
    /// A picker over `names`, starting at `current` (None: the workspace has no theme of its own).
    pub fn new(workspace: bool, names: Vec<String>, current: Option<&str>) -> ThemePicker {
        let names: Vec<String> = workspace.then(|| COLOUR_THEME.to_string()).into_iter().chain(names).collect();
        let selected = names.iter().position(|n| Some(n.as_str()) == current.or(workspace.then_some(COLOUR_THEME))).unwrap_or(0);
        ThemePicker { workspace, names, selected, ..Default::default() }
    }

    pub fn shown(&self) -> Vec<&str> {
        let filter = self.filter.to_lowercase();
        self.names.iter().map(String::as_str).filter(|n| n.to_lowercase().contains(&filter)).collect()
    }

    /// The highlighted theme (None: nothing matches the filter).
    pub fn highlighted(&self) -> Option<&str> {
        self.shown().get(self.selected).copied()
    }

    pub fn move_by(&mut self, delta: isize) {
        let n = self.shown().len();
        if n > 0 {
            self.selected = (self.selected as isize + delta).clamp(0, n as isize - 1) as usize;
        }
    }

    pub fn type_char(&mut self, c: char) {
        self.filter.push(c);
        self.selected = 0;
    }

    pub fn erase(&mut self) {
        self.filter.pop();
        self.selected = 0;
    }

    /// Show the highlighted theme everywhere (`base`, the colour theme, for [`COLOUR_THEME`]);
    /// one that does not load leaves the screen as it is and says why.
    pub fn preview(&mut self, base: &str) {
        let Some(name) = self.highlighted().map(|n| if n == COLOUR_THEME { base.to_string() } else { n.to_string() }) else { return };
        self.problem = super::theme::set(&name).err();
    }

    pub fn draw(&self, f: &mut Frame, area: Rect, base: &str) {
        let shown = self.shown();
        let rows = (shown.len().max(1) + 4 + usize::from(self.problem.is_some())) as u16;
        let [popup] = Layout::horizontal([Constraint::Length(40.min(area.width))]).flex(Flex::Center).areas(area);
        let [popup] = Layout::vertical([Constraint::Length(rows.min(area.height))]).flex(Flex::Center).areas(popup);
        super::media::clear_popup(f, popup);
        let p = super::theme::p();
        let mut lines = vec![Line::from(vec![Span::styled(" › ", Style::default().fg(p.dim)), Span::styled(self.filter.clone(), Style::default().add_modifier(Modifier::BOLD))]), Line::raw("")];
        if shown.is_empty() {
            lines.push(Line::styled(" no theme matches", Style::default().fg(p.dim)));
        }
        let width = popup.width.saturating_sub(2) as usize;
        for (n, name) in shown.iter().enumerate() {
            // A few of the theme's own colours beside its name.
            let palette = super::theme::palette(if *name == COLOUR_THEME { base } else { name }).ok();
            let swatch: Vec<Span> = palette.map(|t| [t.accent, t.own, t.mention, t.good, t.danger].map(|c| Span::styled("█", Style::default().fg(c))).into()).unwrap_or_else(|| vec![Span::styled("can't load", Style::default().fg(p.danger))]);
            let swatch_w: usize = swatch.iter().map(|s| s.content.chars().count()).sum();
            let label: String = format!(" {name}").chars().take(width.saturating_sub(swatch_w + 1)).collect();
            let pad = width.saturating_sub(label.chars().count() + swatch_w + 1);
            let mut spans = vec![Span::raw(label), Span::raw(" ".repeat(pad))];
            spans.extend(swatch);
            let line = Line::from(spans);
            lines.push(if n == self.selected { line.style(Style::default().bg(p.selection)) } else { line });
        }
        if let Some(problem) = &self.problem {
            lines.push(Line::styled(format!(" {problem}"), Style::default().fg(p.danger)));
        }
        let title = format!(" {} ", super::i18n::tr(if self.workspace { "Theme for this workspace" } else { "Colour theme" }));
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(p.accent))
            .title(title)
            .title_bottom(Line::styled(format!(" {} ", super::i18n::tr("type to filter")), Style::default().fg(p.dim)).right_aligned());
        let inner = block.inner(popup);
        f.render_widget(Paragraph::new(lines).block(block), popup);
        f.set_cursor_position((inner.x + 3 + self.filter.chars().count() as u16, inner.y));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Vec<String> {
        ["dark", "light", "terminal", "sunset"].map(String::from).into()
    }

    #[test]
    fn filtering_and_moving() {
        let mut p = ThemePicker::new(false, names(), Some("terminal"));
        assert_eq!(p.highlighted(), Some("terminal"), "starts at the theme in use");
        p.type_char('S');
        assert_eq!(p.shown(), ["sunset"], "ignoring case");
        p.erase();
        p.type_char('t');
        assert_eq!(p.shown(), ["light", "terminal", "sunset"]);
        p.move_by(1);
        p.move_by(5);
        assert_eq!(p.highlighted(), Some("sunset"), "stops at the end");
        p.type_char('x');
        assert_eq!((p.shown().len(), p.highlighted()), (0, None));
        p.move_by(1);
        assert_eq!(p.highlighted(), None, "nothing to move to");
        // A workspace's own theme: the colour theme's first, where it starts with none set.
        let p = ThemePicker::new(true, names(), None);
        assert_eq!((p.shown()[0], p.highlighted()), (COLOUR_THEME, Some(COLOUR_THEME)));
        assert_eq!(ThemePicker::new(true, names(), Some("light")).highlighted(), Some("light"));
    }
}

//! Key reference overlay (F1 or `?`), drawn from the key bindings in effect.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::i18n::tr;
use super::keymap::{ACTIONS, Context, Keymap};

/// Slash commands typed into a composer (not key bindings).
pub const COMMANDS: &[(&str, &str)] = &[
    ("/upload <path> [text]", "upload a file"),
    ("/snippet [lang] [title]", "post the following lines as a snippet"),
    ("/join [#channel] · /leave", "join (default: the previewed channel) · leave this channel"),
    ("/create[-private] <name>", "create a channel"),
    ("/dm @a [@b…]", "open a DM, or a group DM with several people"),
    ("/star · /section rename|delete", "star this conversation · rename / delete its section"),
    ("/status [:e:] [text] [for 30m|today]", "set your status · /status clear"),
    ("/away · /active · /dnd 30m|off", "appear away / active · pause notifications"),
    ("/workspace [name]", "list the profile's workspaces or switch"),
    ("/prefs", "preferences (also F6)"),
];

#[derive(Debug, Default)]
pub struct Help {
    pub scroll: usize,
    /// Rows shown on the last draw (for paging and clamping).
    pub height: usize,
    lines: usize,
}

impl Help {
    pub fn scroll_by(&mut self, delta: isize) {
        let max = self.lines.saturating_sub(self.height);
        self.scroll = (self.scroll as isize + delta).clamp(0, max as isize) as usize;
    }

    pub fn page(&self) -> isize {
        self.height.saturating_sub(2).max(1) as isize
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, keys: &Keymap) {
        let width = 92.min(area.width);
        let [popup] = Layout::horizontal([Constraint::Length(width)]).flex(Flex::Center).areas(area);
        let [popup] = Layout::vertical([Constraint::Length(area.height.saturating_sub(2).max(5))]).flex(Flex::Center).areas(popup);
        super::media::clear_popup(f, popup);
        let lines = lines(keys);
        self.lines = lines.len();
        self.height = popup.height.saturating_sub(2) as usize;
        self.scroll_by(0);
        let close = keys.keys_label(Context::Help, "close");
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(super::theme::p().accent))
            .title(format!(" {} ", super::i18n::tr("Keys")))
            .title_bottom(Line::styled(format!(" {close} {} · {} ", tr("close"), tr("rebind in keys.toml")), Style::default().fg(super::theme::p().dim)).right_aligned());
        f.render_widget(Paragraph::new(lines).scroll((self.scroll as u16, 0)).block(block), popup);
        super::scroll::indicator(f, popup, self.scroll, self.height, self.lines, Style::default().fg(super::theme::p().accent));
    }
}

fn row(keys: String, what: &str) -> Line<'static> {
    Line::from(vec![Span::styled(format!("  {keys:<30} "), Style::default().fg(super::theme::p().accent)), Span::raw(what.to_string())])
}

/// Every context's bindings, then the slash commands.
pub fn lines(keys: &Keymap) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for context in Context::ALL {
        out.push(Line::styled(tr(context.title()).into_owned(), Style::default().add_modifier(Modifier::BOLD)));
        for info in ACTIONS.iter().filter(|a| a.context == context) {
            // Workspaces 2…9 are listed as one row.
            if info.name.starts_with("workspace_") && info.name != "workspace_1" {
                continue;
            }
            let mut label = keys.keys_label(context, info.name);
            if info.name == "workspace_1" {
                label = format!("{label} … {}", keys.keys_label(context, "workspace_9"));
            }
            let label = if label.is_empty() { "(unbound)".to_string() } else { label };
            let what = if info.name == "workspace_1" { "switch to the profile's workspace 1 … 9" } else { info.description };
            out.push(row(label, &tr(what)));
        }
        out.push(Line::raw(""));
    }
    out.push(Line::styled(tr("Commands (type in a composer)").into_owned(), Style::default().add_modifier(Modifier::BOLD)));
    out.extend(COMMANDS.iter().map(|(c, what)| row(c.to_string(), &tr(what))));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.spans.iter().map(|s| s.content.to_string()).collect::<String>()).collect()
    }

    #[test]
    fn help_follows_the_bindings() {
        let (map, _) = Keymap::load("[messages]\nreact = \"R\"\npin = []\n");
        let rows = text(&lines(&map));
        let react = rows.iter().find(|r| r.contains("add a reaction")).unwrap();
        assert!(react.trim_start().starts_with("R "), "{react}");
        assert!(rows.iter().any(|r| r.contains("(unbound)") && r.contains("pin")));
        assert!(rows.iter().any(|r| r.contains("Alt+1 … Alt+9")));
        assert!(!rows.iter().any(|r| r.contains("workspace 2")), "one row for all workspaces");
    }
}

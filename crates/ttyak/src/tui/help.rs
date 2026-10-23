//! Key reference overlay (F1 or `?`), drawn from the key bindings in effect.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::i18n::tr;
use super::keymap::{ACTIONS, Context, Keymap, tree};
use unicode_width::UnicodeWidthStr;

/// Slash commands typed into a composer (not key bindings).
pub const COMMANDS: &[(&str, &str)] = &[
    ("/upload <path> [text]", "upload a file"),
    ("/snippet [lang] [title]", "post the following lines as a snippet"),
    ("/join [#channel] · /leave", "join (default: the previewed channel) · leave this channel"),
    ("/create[-private] <name>", "create a channel"),
    ("/dm @a [@b…]", "open a DM, or a group DM with several people"),
    ("/star · /section rename|delete", "star this conversation · rename / delete its section"),
    ("/status [:e:] [text] [for 30m|today]", "set your status · /status clear"),
    ("/away · /active · /dnd [30m|off]", "appear away / active · pause notifications (alone: 30 min, or resume)"),
    ("/workspace [name]", "list the profile's workspaces or switch"),
    ("/prefs", "preferences (also F6)"),
    ("/remind, /giphy, … any other", "run by Slack (its answer only you see); a leading space sends it as text"),
];

#[derive(Debug, Default)]
pub struct Help {
    pub scroll: usize,
    /// Rows shown on the last draw (for paging and clamping).
    pub height: usize,
    lines: usize,
    /// Last drawn area, for clicks.
    pub area: Rect,
}

impl Help {
    pub fn scroll_by(&mut self, delta: isize) {
        let max = self.lines.saturating_sub(self.height);
        self.scroll = (self.scroll as isize + delta).clamp(0, max as isize) as usize;
    }

    pub fn page(&self) -> isize {
        self.height.saturating_sub(2).max(1) as isize
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, keys: &Keymap, icons: bool) {
        let width = 92.min(area.width);
        let [popup] = Layout::horizontal([Constraint::Length(width)]).flex(Flex::Center).areas(area);
        let [popup] = Layout::vertical([Constraint::Length(area.height.saturating_sub(2).max(5))]).flex(Flex::Center).areas(popup);
        self.area = popup;
        super::media::clear_modal(f, popup);
        let lines = lines(keys, icons);
        self.lines = lines.len();
        self.height = popup.height.saturating_sub(2) as usize;
        self.scroll_by(0);
        let close = keys.keys_label(Context::Help, "close");
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(super::theme::p().accent))
            .title(format!(" {} ", super::i18n::tr("Keys")))
            .title_bottom(Line::styled(format!(" {close} {} · {} ", tr("close"), tr("F6 → Keys · ttyak ctl keys-template")), Style::default().fg(super::theme::p().dim)).right_aligned());
        f.render_widget(Paragraph::new(lines).scroll((self.scroll as u16, 0)).block(block), popup);
        super::scroll::indicator(f, popup, self.scroll, self.height, self.lines, Style::default().fg(super::theme::p().accent));
    }
}

fn row(keys: String, what: &str) -> Line<'static> {
    Line::from(vec![Span::styled(format!("  {keys:<30} "), Style::default().fg(super::theme::p().accent)), Span::raw(what.to_string())])
}

/// `text` padded with spaces to `width` cells.
fn pad(text: &str, width: usize) -> String {
    format!("{text}{}", " ".repeat(width.saturating_sub(UnicodeWidthStr::width(text))))
}

/// The tree of bindings (see [`tree`]) with `keys → icon label` and the description dim,
/// then the Ctrl+Space note and the slash commands.
pub fn lines(keys: &Keymap, icons: bool) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let sections = tree(keys);
    let width = sections.iter().flat_map(|s| &s.rows).map(|r| UnicodeWidthStr::width(r.keys.as_str())).max().unwrap_or(0).clamp(8, 26);
    for section in sections {
        out.push(Line::styled(section.title, Style::default().add_modifier(Modifier::BOLD)));
        for r in section.rows {
            let info = &ACTIONS[r.info];
            let what = if info.name == "workspace_1" { "switch to the profile's workspace 1 … 9" } else { info.description };
            let icon = if icons { pad(&r.icon, 3) } else { String::new() };
            out.push(Line::from(vec![
                Span::styled(format!("  {} ", pad(&r.keys, width)), Style::default().fg(super::theme::p().accent)),
                Span::raw(format!("{icon}{} ", pad(&r.label, 16))),
                Span::styled(tr(what).into_owned(), Style::default().fg(super::theme::p().dim)),
            ]));
        }
        out.push(Line::raw(""));
    }
    if let Some(leader) = keys.insert_leader {
        out.push(Line::raw(format!("  {}: {}", leader.label(), tr("the leader while typing or in a dialog"))));
        out.push(Line::styled(format!("  {}", tr("if your system takes it (macOS input source, IME), rebind it in F6 → Keys")), Style::default().fg(super::theme::p().dim)));
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
        let rows = text(&lines(&map, true));
        let react = rows.iter().find(|r| r.contains("add a reaction")).unwrap();
        assert!(react.trim_start().starts_with("R "), "{react}");
        assert!(rows.iter().any(|r| r.contains("pin / unpin")));
        assert!(rows.iter().any(|r| r.contains("Alt+1 … Alt+9")));
        assert!(!rows.iter().any(|r| r.contains("workspace 2")), "one row for all workspaces");
        // The defaults: the message's actions behind the leader, the card's behind Ctrl+Space.
        let rows = text(&lines(&Keymap::load("").0, true));
        assert!(rows.iter().any(|r| r.trim_start().starts_with("Space r ") && r.contains("add a reaction")), "{rows:#?}");
        assert!(rows.iter().any(|r| r.trim_start().starts_with("Ctrl+Space h p ") && r.contains("start a huddle with them")));
        assert!(rows.iter().any(|r| r.trim_start().starts_with("Space h h ") && r.contains("join or leave the huddle")));
    }

    fn titles_of(map: &Keymap) -> Vec<String> {
        tree(map).into_iter().map(|s| s.title).collect()
    }

    fn keys_of(map: &Keymap, section: &str, label: &str) -> Option<String> {
        tree(map).into_iter().find(|s| s.title == section)?.rows.into_iter().find(|r| r.label == label).map(|r| r.keys)
    }

    #[test]
    fn help_follows_the_tree() {
        let map = Keymap::load("").0;
        let titles = titles_of(&map);
        let want = ["Direct keys — everywhere", "Direct keys — sidebar", "Direct keys — messages and thread", "Direct keys — huddle view", "␣ on a message", "␣ on a conversation", "␣c conversation", "␣h huddle", "␣n new", "␣a availability", "␣f find", "Typing (INSERT)"];
        assert_eq!(titles[..want.len()], want, "{titles:?}");
        assert_eq!(titles.last().map(String::as_str), Some("Update screen"), "{titles:?}");
        assert_eq!(keys_of(&map, "Update screen", "update now").as_deref(), Some("u"));
        assert!(titles.contains(&"Profile card".to_string()) && !titles.contains(&"Unbound".to_string()));
        assert_eq!(keys_of(&map, "␣ on a message", "react").as_deref(), Some("Space r"));
        assert_eq!(keys_of(&map, "␣h huddle", "mute").as_deref(), Some("Space h m"));
        // Workspaces are one row, with the direct keys (their first keys are Alt+digits).
        let workspaces: Vec<_> = tree(&map).into_iter().flat_map(|s| s.rows).filter(|r| r.label.starts_with("workspace")).collect();
        assert_eq!(workspaces.len(), 1, "{workspaces:?}");
        assert_eq!(keys_of(&map, "Direct keys — everywhere", "workspace 1…9").as_deref(), Some("Alt+1 … Alt+9 / Space w 1 … Space w 9"));
        // A plain key moves the action to the direct keys.
        let map = Keymap::load("[messages]\nreact = \"r\"\n").0;
        assert_eq!(keys_of(&map, "Direct keys — messages and thread", "react").as_deref(), Some("r"));
        assert_eq!(keys_of(&map, "␣ on a message", "react"), None);
        // No keys: under Unbound, last.
        let map = Keymap::load("[messages]\npin = []\n").0;
        let titles = titles_of(&map);
        assert_eq!(titles.last().map(String::as_str), Some("Unbound"));
        assert_eq!(keys_of(&map, "Unbound", "pin / unpin").as_deref(), Some(""));
        // A user group has a section of its own, after the default ones.
        let map = Keymap::load("[global]\njump_to = \"<leader> z j\"\n[groups]\n\"<leader> z\" = \"zoo\"\n").0;
        let titles = titles_of(&map);
        let (zoo, typing) = (titles.iter().position(|t| t == "␣z zoo").unwrap(), titles.iter().position(|t| t == "Typing (INSERT)").unwrap());
        assert!(titles.iter().position(|t| t == "␣f find").unwrap() < zoo && zoo < typing, "{titles:?}");
    }

    #[test]
    fn help_draws_the_tree() {
        let rows = text(&lines(&Keymap::load("").0, true));
        assert!(rows.iter().any(|r| r == "Direct keys — everywhere"), "{rows:#?}");
        let react = rows.iter().find(|r| r.contains("add a reaction")).unwrap();
        assert!(react.trim_start().starts_with("Space r ") && react.contains("😀 react"), "{react}");
        let plain = text(&lines(&Keymap::load("").0, false));
        assert!(!plain.iter().any(|r| r.contains("😀")));
        assert!(rows.iter().any(|r| r.contains("Ctrl+Space") && r.contains("leader")));
        assert!(rows.iter().any(|r| r.contains("Commands (type in a composer)")));
    }
}

//! Key reference overlay (F1 or `?`).

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use super::text::DIM;

/// (section, [(keys, what they do)])
pub const KEYS: &[(&str, &[(&str, &str)])] = &[
    ("Everywhere", &[
        ("Tab / Shift+Tab", "move focus between panes"),
        ("Alt+↑ / Alt+↓", "previous / next conversation"),
        ("Ctrl+K", "jump to a conversation or person (# alone: every channel)"),
        ("Ctrl+F", "search messages"),
        ("F5", "activity: mentions, reactions, threads (Enter opens)"),
        ("F2", "conversation details: about, members (Enter: profile), pins"),
        ("F3 / F4", "join or leave the huddle / mute or unmute"),
        ("PgUp / PgDn", "scroll messages"),
        ("F1 or ?", "this help"),
        ("Ctrl+Q", "quit"),
    ]),
    ("Sidebar", &[
        ("↑↓ j k g G · Enter", "choose a conversation · type in it"),
        ("*", "star / unstar"),
        ("m", "move to a section (or a new one)"),
    ]),
    ("Messages and thread", &[
        ("↑↓ j k g G", "select a message"),
        ("Enter", "open its thread"),
        ("r", "add a reaction"),
        ("e / d", "edit / delete your message"),
        ("y / Y", "copy text / link"),
        ("o", "open image, play video, or view a snippet"),
        ("s", "save attachments"),
        ("P", "pin / unpin"),
        ("u", "profile card: author, ←/→ people mentioned; Enter message, h huddle"),
        ("i", "type (in the thread: reply)"),
        ("Esc", "back to the composer (in the thread: close it)"),
    ]),
    ("Composer", &[
        ("Enter", "send (Alt/Shift+Enter or Ctrl+J: new line)"),
        ("↑ in an empty composer", "edit your last message"),
        ("@ # :", "mention, channel, emoji (Tab/Enter to insert)"),
        ("Alt+B / I / S / C", "bold, italic, strike, code"),
        ("Ctrl+W / U / A / E", "delete word / line, line start / end"),
        ("/upload <path> [text]", "upload a file"),
        ("/join [#channel]", "join a channel (default: the one previewed)"),
        ("/leave", "leave the current channel"),
        ("/dm @a [@b…]", "open a DM, or a group DM with several people"),
        ("/snippet [lang] [title]", "post the following lines as a snippet"),
        ("/star · /section rename|delete", "star this conversation · rename / delete its section"),
        ("/status [:e:] [text] [for 30m|today]", "set your status · /status clear"),
        ("/away · /active · /dnd 30m|off", "appear away / active · pause notifications"),
        ("/create[-private] <name>", "create a channel"),
        ("Alt+A (reply box)", "also send the reply to the channel"),
        ("Esc", "cancel an edit, else leave the composer"),
    ]),
    ("Viewer and player", &[("← → h l", "previous / next image"), ("Space / p", "pause video"), ("↑↓ PgUp PgDn g G · y", "scroll / copy a snippet"), ("Esc / q", "close")]),
];

pub fn draw(f: &mut Frame, area: Rect) {
    let width = 76.min(area.width);
    let rows: u16 = KEYS.iter().map(|(_, k)| k.len() as u16 + 2).sum::<u16>() + 2;
    let [popup] = Layout::horizontal([Constraint::Length(width)]).flex(Flex::Center).areas(area);
    let [popup] = Layout::vertical([Constraint::Length(rows.min(area.height))]).flex(Flex::Center).areas(popup);
    f.render_widget(Clear, popup);
    let mut lines = Vec::new();
    for (section, keys) in KEYS {
        lines.push(Line::styled(*section, Style::default().add_modifier(Modifier::BOLD)));
        for (k, what) in *keys {
            lines.push(Line::from(vec![Span::styled(format!("  {k:<24}"), Style::default().fg(Color::Rgb(0x61, 0xaf, 0xef))), Span::raw(*what)]));
        }
        lines.push(Line::raw(""));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(0x61, 0xaf, 0xef)))
        .title(" Keys ")
        .title_bottom(Line::styled(" any key closes ", Style::default().fg(DIM)).right_aligned());
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), popup);
}

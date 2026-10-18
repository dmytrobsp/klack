//! Right-click context menus for messages and sidebar entries. Items run the same actions
//! as their keys, which are shown beside them.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use super::keymap::{ACTIONS, Action, Context, Keymap};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A message in the message pane, or the thread pane.
    Message { thread: bool },
    /// The selected sidebar conversation.
    Conversation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Do(Action),
    /// Conversation details (F2) for the clicked conversation.
    Details,
    /// Leave the clicked channel.
    Leave,
}

pub struct Menu {
    pub target: Target,
    pub items: Vec<(Item, String, String)>,
    pub selected: usize,
    /// Where it was opened (top-left corner of the menu).
    pub at: (u16, u16),
    /// Last drawn area, for clicks.
    pub area: Rect,
}

/// Messages actions that make sense on one message (not movement or focus).
const MESSAGE_ACTIONS: &[Action] = &[
    Action::OpenThread,
    Action::React,
    Action::Edit,
    Action::Delete,
    Action::Copy,
    Action::CopyLink,
    Action::Open,
    Action::Save,
    Action::Pin,
    Action::Profile,
    Action::ThreadHuddle,
];

impl Menu {
    /// `own`: the message is yours (edit/delete are offered); `channel`: the conversation
    /// is a channel (leave is offered).
    pub fn new(target: Target, at: (u16, u16), keys: &Keymap, own: bool, channel: bool, starred: bool) -> Menu {
        let key = |c: Context, a: Action| keys.first_key(c, a).unwrap_or_default();
        let describe = |c: Context, a: Action| ACTIONS.iter().find(|i| i.context == c && i.action == a).map_or(String::new(), |i| super::i18n::tr(i.description).into_owned());
        let items = match &target {
            Target::Message { thread } => MESSAGE_ACTIONS
                .iter()
                .filter(|a| own || !matches!(a, Action::Edit | Action::Delete))
                .filter(|a| !(*thread && **a == Action::OpenThread))
                .map(|a| (Item::Do(*a), describe(Context::Messages, *a), key(Context::Messages, *a)))
                .collect(),
            Target::Conversation => {
                let mut items = vec![
                    (Item::Do(Action::Open), super::i18n::tr("open").into_owned(), key(Context::Sidebar, Action::Open)),
                    (Item::Do(Action::Star), super::i18n::tr(if starred { "unstar" } else { "star" }).into_owned(), key(Context::Sidebar, Action::Star)),
                    (Item::Do(Action::MoveToSection), describe(Context::Sidebar, Action::MoveToSection), key(Context::Sidebar, Action::MoveToSection)),
                    (Item::Details, describe(Context::Global, Action::Details), key(Context::Global, Action::Details)),
                ];
                if channel {
                    items.push((Item::Leave, super::i18n::tr("leave the channel").into_owned(), "/leave".into()));
                }
                items
            }
        };
        Menu { target, items, selected: 0, at, area: Rect::default() }
    }

    pub fn move_by(&mut self, delta: isize) {
        self.selected = (self.selected as isize + delta).clamp(0, self.items.len() as isize - 1) as usize;
    }

    /// The item drawn at a terminal position.
    pub fn item_at(&self, column: u16, row: u16) -> Option<usize> {
        let inner = self.area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
        inner.contains((column, row).into()).then(|| (row - inner.y) as usize).filter(|n| *n < self.items.len())
    }

    pub fn chosen(&self) -> Option<Item> {
        self.items.get(self.selected).map(|i| i.0)
    }

    pub fn draw(&mut self, f: &mut Frame, screen: Rect) {
        let p = super::theme::p();
        let label_w = self.items.iter().map(|i| i.1.chars().count()).max().unwrap_or(0);
        let key_w = self.items.iter().map(|i| i.2.chars().count()).max().unwrap_or(0);
        let width = ((label_w + key_w + 5) as u16).min(screen.width);
        let height = (self.items.len() as u16 + 2).min(screen.height);
        // Open at the click, kept on screen.
        let x = self.at.0.min(screen.right().saturating_sub(width));
        let y = self.at.1.min(screen.bottom().saturating_sub(height));
        self.area = Rect { x, y, width, height };
        f.render_widget(Clear, self.area);
        let lines: Vec<Line> = self
            .items
            .iter()
            .enumerate()
            .map(|(n, (_, label, key))| {
                let line = Line::from(vec![Span::raw(format!(" {label:<label_w$} ")), Span::styled(format!("{key:>key_w$} "), Style::default().fg(p.dim))]);
                if n == self.selected { line.style(Style::default().bg(p.selection).add_modifier(Modifier::BOLD)) } else { line }
            })
            .collect();
        f.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(p.accent))), self.area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_fit_the_target() {
        let keys = Keymap::load("").0;
        let theirs = Menu::new(Target::Message { thread: false }, (0, 0), &keys, false, true, false);
        assert!(!theirs.items.iter().any(|i| matches!(i.0, Item::Do(Action::Edit | Action::Delete))), "not on others' messages");
        assert_eq!(theirs.items[0], (Item::Do(Action::OpenThread), "open the message's thread".into(), "Enter".into()));
        let mine_in_thread = Menu::new(Target::Message { thread: true }, (0, 0), &keys, true, true, false);
        assert!(mine_in_thread.items.iter().any(|i| i.0 == Item::Do(Action::Delete)));
        assert!(!mine_in_thread.items.iter().any(|i| i.0 == Item::Do(Action::OpenThread)));
        let dm = Menu::new(Target::Conversation, (0, 0), &keys, false, false, true);
        assert!(dm.items.iter().any(|i| i.1 == "unstar") && !dm.items.iter().any(|i| i.0 == Item::Leave));
    }

    #[test]
    fn stays_on_screen_and_hits_items() {
        let keys = Keymap::load("").0;
        let mut m = Menu::new(Target::Conversation, (78, 22), &keys, false, true, false);
        m.area = Rect { x: 50, y: 10, width: 30, height: 7 };
        assert_eq!(m.item_at(55, 11), Some(0));
        assert_eq!(m.item_at(55, 15), Some(4));
        assert_eq!(m.item_at(55, 16), None, "the border");
        assert_eq!(m.item_at(10, 11), None);
    }
}

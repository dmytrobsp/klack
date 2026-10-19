//! Right-click context menus for messages and sidebar entries. Items run the same actions
//! as their keys, which are shown beside them.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::keymap::{ACTIONS, Action, Context, Keymap};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A message in the message pane, or the thread pane.
    Message { thread: bool },
    /// The selected sidebar conversation.
    Conversation,
    /// What message `ts` holds, to open one (`o` with several).
    Opener { thread: bool, ts: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Do(Action),
    /// Conversation details (F2) for the clicked conversation.
    Details,
    /// Leave the clicked channel.
    Leave,
    /// Open the menu's `n`th thing.
    Thing(usize),
}

pub struct Menu {
    pub target: Target,
    pub items: Vec<(Item, String, String)>,
    pub selected: usize,
    /// Where it was opened (top-left corner of the menu).
    pub at: (u16, u16),
    /// Last drawn area, for clicks.
    pub area: Rect,
    /// An opener's things, by `Item::Thing` number.
    pub things: Vec<super::open::Thing>,
}

/// Longest line of an opener (a long address is cut).
const OPENER_WIDTH: usize = 72;

/// What a message offers, so its menu shows only what applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offers {
    /// Yours: it can be edited and deleted.
    pub own: bool,
    /// It holds a link, picture, video or snippet.
    pub open: bool,
    /// It has files.
    pub save: bool,
    /// It has text to copy.
    pub text: bool,
}

impl Offers {
    pub fn of(m: &slack_core::types::Message, self_id: &str) -> Offers {
        Offers { own: m.user.as_deref() == Some(self_id), open: !super::open::things(m).is_empty(), save: !m.files.is_empty(), text: !m.text.trim().is_empty() }
    }
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
    /// `offers`: what the message allows (only that is offered); `channel`: the conversation
    /// is a channel (leave is offered).
    pub fn new(target: Target, at: (u16, u16), keys: &Keymap, offers: Offers, channel: bool, starred: bool) -> Menu {
        let key = |c: Context, a: Action| keys.first_key(c, a).unwrap_or_default();
        let describe = |c: Context, a: Action| ACTIONS.iter().find(|i| i.context == c && i.action == a).map_or(String::new(), |i| super::i18n::tr(i.description).into_owned());
        let items = match &target {
            Target::Message { thread } => MESSAGE_ACTIONS
                .iter()
                .filter(|a| match a {
                    Action::Edit | Action::Delete => offers.own,
                    Action::Open => offers.open,
                    Action::Save => offers.save,
                    Action::Copy => offers.text,
                    _ => true,
                })
                .filter(|a| !(*thread && **a == Action::OpenThread))
                .map(|a| (Item::Do(*a), describe(Context::Messages, *a), key(Context::Messages, *a)))
                .collect(),
            Target::Opener { .. } => Vec::new(),
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
        Menu { target, items, selected: 0, at, area: Rect::default(), things: Vec::new() }
    }

    /// A list of what message `ts` holds (`labels` beside `things`), to open one.
    pub fn opener(thread: bool, ts: String, things: Vec<super::open::Thing>, labels: Vec<String>, at: (u16, u16)) -> Menu {
        let items = labels
            .into_iter()
            .enumerate()
            .map(|(n, l)| {
                let l = if l.chars().count() > OPENER_WIDTH { l.chars().take(OPENER_WIDTH - 1).chain(['…']).collect() } else { l };
                (Item::Thing(n), l, String::new())
            })
            .collect();
        Menu { target: Target::Opener { thread, ts }, items, selected: 0, at, area: Rect::default(), things }
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
        let label_w = self.items.iter().map(|i| unicode_width::UnicodeWidthStr::width(i.1.as_str())).max().unwrap_or(0);
        let key_w = self.items.iter().map(|i| i.2.chars().count()).max().unwrap_or(0);
        let width = ((label_w + key_w + 5) as u16).min(screen.width);
        let height = (self.items.len() as u16 + 2).min(screen.height);
        // Open at the click, kept on screen.
        let x = self.at.0.min(screen.right().saturating_sub(width));
        let y = self.at.1.min(screen.bottom().saturating_sub(height));
        self.area = Rect { x, y, width, height };
        super::media::clear_modal(f, self.area);
        let lines: Vec<Line> = self
            .items
            .iter()
            .enumerate()
            .map(|(n, (_, label, key))| {
                let pad = " ".repeat(label_w - unicode_width::UnicodeWidthStr::width(label.as_str()));
                let line = Line::from(vec![Span::raw(format!(" {label}{pad} ")), Span::styled(format!("{key:>key_w$} "), Style::default().fg(p.dim))]);
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
        let all = Offers { own: true, open: true, save: true, text: true };
        let theirs = Menu::new(Target::Message { thread: false }, (0, 0), &keys, Offers { own: false, ..all }, true, false);
        assert!(!theirs.items.iter().any(|i| matches!(i.0, Item::Do(Action::Edit | Action::Delete))), "not on others' messages");
        assert_eq!(theirs.items[0], (Item::Do(Action::OpenThread), "open the message's thread".into(), "Enter".into()));
        let mine_in_thread = Menu::new(Target::Message { thread: true }, (0, 0), &keys, all, true, false);
        assert!(mine_in_thread.items.iter().any(|i| i.0 == Item::Do(Action::Delete)));
        assert!(!mine_in_thread.items.iter().any(|i| i.0 == Item::Do(Action::OpenThread)));
        let plain = Menu::new(Target::Message { thread: false }, (0, 0), &keys, Offers { own: false, open: false, save: false, text: true }, true, false);
        assert!(!plain.items.iter().any(|i| matches!(i.0, Item::Do(Action::Open | Action::Save))), "nothing to open or save: not offered");
        let picture_only = Menu::new(Target::Message { thread: false }, (0, 0), &keys, Offers { own: false, open: true, save: true, text: false }, true, false);
        assert!(!picture_only.items.iter().any(|i| i.0 == Item::Do(Action::Copy)), "no text to copy");
        assert!(picture_only.items.iter().any(|i| i.0 == Item::Do(Action::Open)) && picture_only.items.iter().any(|i| i.0 == Item::Do(Action::Save)));
        let dm = Menu::new(Target::Conversation, (0, 0), &keys, all, false, true);
        assert!(dm.items.iter().any(|i| i.1 == "unstar") && !dm.items.iter().any(|i| i.0 == Item::Leave));
    }

    #[test]
    fn stays_on_screen_and_hits_items() {
        let keys = Keymap::load("").0;
        let mut m = Menu::new(Target::Conversation, (78, 22), &keys, Offers { own: false, open: false, save: false, text: false }, true, false);
        m.area = Rect { x: 50, y: 10, width: 30, height: 7 };
        assert_eq!(m.item_at(55, 11), Some(0));
        assert_eq!(m.item_at(55, 15), Some(4));
        assert_eq!(m.item_at(55, 16), None, "the border");
        assert_eq!(m.item_at(10, 11), None);
    }
}

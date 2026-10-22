//! Right-click context menus for messages and sidebar entries. Items run the same actions
//! as their keys, which are shown beside them.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::keymap::{ACTIONS, Action, Context, Key, Keymap, Mode, Resolved};

/// Where a message is shown: what its actions find it by, and where its menu opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shown {
    /// The conversation's messages pane.
    Messages,
    /// The thread pane.
    Thread,
    /// The Threads view's feed.
    Feed,
    /// The Activity view.
    Activity,
}

/// A message acted on: its conversation, its ts, and where it is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageTarget {
    pub channel: String,
    pub ts: String,
    pub shown: Shown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A message, where it is shown.
    Message(MessageTarget),
    /// The selected sidebar conversation.
    Conversation,
    /// What a message holds, to open one (`o` with several).
    Opener(MessageTarget),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Do(Action),
    /// Conversation details for the clicked conversation.
    Details,
    /// Leave the clicked channel.
    Leave,
    /// Open the menu's `n`th thing.
    Thing(usize),
    /// A title between groups of an opener's rows, which nothing selects.
    Heading,
}

pub struct Menu {
    pub target: Target,
    pub items: Vec<(Item, String, String)>,
    /// The key that runs each item in the open menu: the last of its first sequence (`r` for
    /// `␣r`), unless the menu uses that key itself.
    pub run_keys: Vec<Option<Key>>,
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

/// The open menu's headings: links into klack, other links, files.
pub const OPENER_HEADINGS: [&str; 3] = ["In klack", "In the browser", "Files"];

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
    /// It is a reply in a thread.
    pub reply: bool,
}

impl Offers {
    pub fn of(m: &slack_core::types::Message, self_id: &str) -> Offers {
        Offers { own: m.user.as_deref() == Some(self_id), open: !super::open::things(m, None).is_empty(), save: !m.files.is_empty(), text: !m.text.trim().is_empty(), reply: m.is_reply() }
    }

    /// Whether a messages action makes sense on the message (edit / delete yours, open, save
    /// and copy what it holds).
    pub fn allows(&self, action: Action) -> bool {
        match action {
            Action::Edit | Action::Delete => self.own,
            Action::Open => self.open,
            Action::Save => self.save,
            Action::Copy => self.text,
            _ => true,
        }
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
    Action::MarkUnread,
    Action::Profile,
    Action::ThreadHuddle,
];

impl Menu {
    /// `offers`: what the message allows (only that is offered); `channel`: the conversation
    /// is a channel (leave is offered).
    pub fn new(target: Target, at: (u16, u16), keys: &Keymap, offers: Offers, channel: bool, starred: bool, muted: bool) -> Menu {
        let key = |c: Context, a: Action| keys.first_shown(c, a).unwrap_or_default();
        let describe = |c: Context, a: Action| ACTIONS.iter().find(|i| i.context == c && i.action == a).map_or(String::new(), |i| super::i18n::tr(i.description).into_owned());
        let items = match &target {
            // (The thread pane is in its thread; a view's Enter opens it.)
            Target::Message(t) => MESSAGE_ACTIONS
                .iter()
                .filter(|a| offers.allows(**a))
                .filter(|a| !(t.shown == Shown::Thread && matches!(**a, Action::OpenThread | Action::MarkUnread)))
                .filter(|a| !(matches!(t.shown, Shown::Feed | Shown::Activity) && **a == Action::OpenThread))
                // (A reply there is in a thread, which is not marked unread.)
                .filter(|a| !(matches!(t.shown, Shown::Feed | Shown::Activity) && offers.reply && **a == Action::MarkUnread))
                .map(|a| (Item::Do(*a), describe(Context::Messages, *a), key(Context::Messages, *a)))
                .collect(),
            Target::Opener(_) => Vec::new(),
            Target::Conversation => {
                let mut items = vec![
                    (Item::Do(Action::Open), super::i18n::tr("open").into_owned(), key(Context::Sidebar, Action::Open)),
                    (Item::Do(Action::Star), super::i18n::tr(if starred { "unstar" } else { "star" }).into_owned(), key(Context::Sidebar, Action::Star)),
                    (Item::Do(Action::MoveToSection), describe(Context::Sidebar, Action::MoveToSection), key(Context::Sidebar, Action::MoveToSection)),
                    (Item::Details, describe(Context::Global, Action::Details), key(Context::Sidebar, Action::Details)),
                    (Item::Do(Action::MuteConversation), super::i18n::tr(if muted { "unmute" } else { "mute" }).into_owned(), key(Context::Sidebar, Action::MuteConversation)),
                    (Item::Do(Action::MarkUnread), super::i18n::tr("mark unread").into_owned(), key(Context::Global, Action::MarkUnread)),
                    (Item::Do(Action::CopyConversationLink), super::i18n::tr("copy link").into_owned(), key(Context::Sidebar, Action::CopyConversationLink)),
                ];
                if channel {
                    items.push((Item::Leave, super::i18n::tr("leave the channel").into_owned(), key(Context::Sidebar, Action::LeaveConversation)));
                }
                items
            }
        };
        // Each item's key as its pane binds it, run here by its last key.
        let pane = |item: &Item| match (&target, item) {
            (Target::Message(_), Item::Do(a)) => Some((Context::Messages, *a)),
            (Target::Conversation, Item::Do(a)) => Some((Context::Sidebar, *a)),
            (Target::Conversation, Item::Details) => Some((Context::Sidebar, Action::Details)),
            (Target::Conversation, Item::Leave) => Some((Context::Sidebar, Action::LeaveConversation)),
            _ => None,
        };
        let run_keys = items
            .iter()
            .map(|(item, ..)| {
                let (context, action) = pane(item)?;
                let last = *keys.sequences(context, ACTIONS.iter().find(|i| i.context == context && i.action == action)?.name).first()?.last()?;
                (keys.resolve(&[Context::Menu], Mode::Dialog, &[], last) == Resolved::None).then_some(last)
            })
            .collect();
        Menu { target, items, run_keys, selected: 0, at, area: Rect::default(), things: Vec::new() }
    }

    /// A list of what message `message` holds (`labels` beside `things`), to open one.
    pub fn opener(message: MessageTarget, things: Vec<super::open::Thing>, labels: Vec<String>, at: (u16, u16)) -> Menu {
        // With links into klack among the things (they come first): "In klack", then "In the browser", then "Files"
        // (they open in klack's viewers).
        let grouped = things.iter().any(super::open::Thing::in_app);
        let heading = |t: &super::open::Thing| match t {
            super::open::Thing::Link { app: Some(_), .. } => OPENER_HEADINGS[0],
            super::open::Thing::Link { .. } => OPENER_HEADINGS[1],
            _ => OPENER_HEADINGS[2],
        };
        let mut items = Vec::new();
        for (n, l) in labels.into_iter().enumerate() {
            if grouped && (n == 0 || heading(&things[n - 1]) != heading(&things[n])) {
                items.push((Item::Heading, super::i18n::tr(heading(&things[n])).into_owned(), String::new()));
            }
            let l = if l.chars().count() > OPENER_WIDTH { l.chars().take(OPENER_WIDTH - 1).chain(['…']).collect() } else { l };
            items.push((Item::Thing(n), l, String::new()));
        }
        let selected = items.iter().position(|i| i.0 != Item::Heading).unwrap_or(0);
        Menu { target: Target::Opener(message), run_keys: vec![None; items.len()], items, selected, at, area: Rect::default(), things }
    }

    /// The item `key` runs in the open menu, if any.
    pub fn item_for_key(&self, key: Key) -> Option<Item> {
        self.run_keys.iter().position(|k| *k == Some(key)).and_then(|n| self.items.get(n)).map(|i| i.0)
    }

    pub fn move_by(&mut self, delta: isize) {
        let target = (self.selected as isize + delta).clamp(0, self.items.len() as isize - 1);
        // A heading is stepped over: on to the next row that way, or back to the nearest one when the end is a heading.
        let row = |n: &isize| self.items.get(*n as usize).is_some_and(|i| n >= &0 && i.0 != Item::Heading);
        let dir = if delta < 0 { -1 } else { 1 };
        if let Some(next) = (0..self.items.len() as isize).map(|k| target + dir * k).find(row).or_else(|| (0..self.items.len() as isize).map(|k| target - dir * k).find(row)) {
            self.selected = next as usize;
        }
    }

    /// The item drawn at a terminal position.
    pub fn item_at(&self, column: u16, row: u16) -> Option<usize> {
        let inner = self.area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
        inner.contains((column, row).into()).then(|| (row - inner.y) as usize).filter(|n| self.items.get(*n).is_some_and(|i| i.0 != Item::Heading))
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
            .map(|(n, (item, label, key))| {
                if *item == Item::Heading {
                    return Line::from(Span::styled(format!(" {label}"), Style::default().fg(p.dim).add_modifier(Modifier::BOLD)));
                }
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

    /// A message shown at `shown`.
    fn at(shown: Shown) -> MessageTarget {
        MessageTarget { channel: "C1".into(), ts: "1.0".into(), shown }
    }

    #[test]
    fn items_fit_the_target() {
        let keys = Keymap::load("").0;
        let all = Offers { own: true, open: true, save: true, text: true, reply: false };
        let theirs = Menu::new(Target::Message(at(Shown::Messages)), (0, 0), &keys, Offers { own: false, ..all }, true, false, false);
        assert!(!theirs.items.iter().any(|i| matches!(i.0, Item::Do(Action::Edit | Action::Delete))), "not on others' messages");
        assert_eq!(theirs.items[0], (Item::Do(Action::OpenThread), "open the message's thread".into(), "Enter".into()));
        let mine_in_thread = Menu::new(Target::Message(at(Shown::Thread)), (0, 0), &keys, all, true, false, false);
        assert!(mine_in_thread.items.iter().any(|i| i.0 == Item::Do(Action::Delete)));
        assert!(!mine_in_thread.items.iter().any(|i| i.0 == Item::Do(Action::OpenThread)));
        let plain = Menu::new(Target::Message(at(Shown::Messages)), (0, 0), &keys, Offers { own: false, open: false, save: false, text: true, reply: false }, true, false, false);
        assert!(!plain.items.iter().any(|i| matches!(i.0, Item::Do(Action::Open | Action::Save))), "nothing to open or save: not offered");
        let picture_only = Menu::new(Target::Message(at(Shown::Messages)), (0, 0), &keys, Offers { own: false, open: true, save: true, text: false, reply: false }, true, false, false);
        assert!(!picture_only.items.iter().any(|i| i.0 == Item::Do(Action::Copy)), "no text to copy");
        assert!(picture_only.items.iter().any(|i| i.0 == Item::Do(Action::Open)) && picture_only.items.iter().any(|i| i.0 == Item::Do(Action::Save)));
        let dm = Menu::new(Target::Conversation, (0, 0), &keys, all, false, true, false);
        assert!(dm.items.iter().any(|i| i.1 == "unstar") && !dm.items.iter().any(|i| i.0 == Item::Leave));
    }

    #[test]
    fn a_views_message_has_the_messages_keys() {
        let keys = Keymap::load("").0;
        let all = Offers { own: true, open: true, save: true, text: true, reply: false };
        for shown in [Shown::Feed, Shown::Activity] {
            let m = Menu::new(Target::Message(at(shown)), (0, 0), &keys, all, false, false, false);
            assert!(!m.items.iter().any(|i| i.0 == Item::Do(Action::OpenThread)), "Enter is the view's: {shown:?}");
            let open = m.items.iter().find(|i| i.0 == Item::Do(Action::Open)).unwrap();
            assert_eq!((open.1.as_str(), open.2.as_str()), ("open a link, picture, video or snippet (a list when there are several)", "␣o"), "the messages' own: {shown:?}");
            let key = |s: &str| super::super::keymap::Key::parse(s).unwrap();
            assert_eq!((m.item_for_key(key("r")), m.item_for_key(key("o")), m.item_for_key(key("U"))), (Some(Item::Do(Action::React)), Some(Item::Do(Action::Open)), Some(Item::Do(Action::MarkUnread))));
            // A reply there is not marked unread.
            let reply = Menu::new(Target::Message(at(shown)), (0, 0), &keys, Offers { reply: true, ..all }, false, false, false);
            assert!(!reply.items.iter().any(|i| i.0 == Item::Do(Action::MarkUnread)), "{shown:?}");
        }
        // In a conversation's messages a reply sent to the channel too is.
        let broadcast = Menu::new(Target::Message(at(Shown::Messages)), (0, 0), &keys, Offers { reply: true, ..all }, false, false, false);
        assert!(broadcast.items.iter().any(|i| i.0 == Item::Do(Action::MarkUnread)));
    }

    #[test]
    fn item_keys_run_items() {
        let keys = Keymap::load("").0;
        let all = Offers { own: true, open: true, save: true, text: true, reply: false };
        let m = Menu::new(Target::Message(at(Shown::Messages)), (0, 0), &keys, all, false, false, false);
        let react = m.items.iter().find(|i| i.0 == Item::Do(Action::React)).unwrap();
        assert_eq!(react.2, "␣r", "the key column shows the sequence");
        let key = |s: &str| super::super::keymap::Key::parse(s).unwrap();
        assert_eq!(m.item_for_key(key("r")), Some(Item::Do(Action::React)));
        assert_eq!(m.item_for_key(key("T")), Some(Item::Do(Action::ThreadHuddle)), "␣hT: its last key");
        assert_eq!(m.item_for_key(key("x")), None);
        // The menu's own keys keep their menu meaning (Enter chooses, though it opens the thread).
        for own in ["up", "down", "enter", "space", "esc", "q", "j", "k"] {
            assert_eq!(m.item_for_key(key(own)), None, "{own}");
        }
        let conversation = Menu::new(Target::Conversation, (0, 0), &keys, all, true, false, false);
        let shown: Vec<&str> = conversation.items.iter().map(|i| i.2.as_str()).collect();
        assert_eq!(shown, ["Enter", "␣s", "␣m", "␣d", "␣M", "␣cu", "␣y", "␣l"]);
        assert_eq!((conversation.item_for_key(key("d")), conversation.item_for_key(key("l"))), (Some(Item::Details), Some(Item::Leave)));
    }

    #[test]
    fn stays_on_screen_and_hits_items() {
        let keys = Keymap::load("").0;
        let mut m = Menu::new(Target::Conversation, (78, 22), &keys, Offers { own: false, open: false, save: false, text: false, reply: false }, true, false, false);
        m.area = Rect { x: 50, y: 10, width: 30, height: 7 };
        assert_eq!(m.item_at(55, 11), Some(0));
        assert_eq!(m.item_at(55, 15), Some(4));
        assert_eq!(m.item_at(55, 16), None, "the border");
        assert_eq!(m.item_at(10, 11), None);
    }
}

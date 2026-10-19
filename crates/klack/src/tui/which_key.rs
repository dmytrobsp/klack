//! The which-key panel: while a key sequence is under way, the keys that can come next, in
//! aligned columns of `key → icon label` in a corner above the status line.

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::i18n::tr;
use super::keymap::{short, Child, Context, Key, Keymap, Pending};
use super::settings::WhichKey;

/// Words the panel shows besides labels and group names (for translation templates).
pub const WORDS: &[&str] = &["more", "previous", "message", "conversation", "actions"];

/// The panel's title: the keys so far and what they lead to (`␣h  huddle`); at the leader alone,
/// what the pane's own keys act on (`␣  message`).
pub fn title(keys: &Keymap, contexts: &[Context], pending: &Pending) -> String {
    let name = match (keys.group(pending.keys()), pending.keys()) {
        (Some((name, _)), _) => name,
        (None, [_]) => tr(match contexts.iter().find(|c| **c != Context::Completion) {
            Some(Context::Messages) => "message",
            Some(Context::Sidebar) => "conversation",
            _ => "actions",
        })
        .into_owned(),
        (None, _) => String::new(),
    };
    format!("{}  {name}", keys.show(pending.typed())).trim_end().to_string()
}

/// A key while a sequence is under way: Esc cancels it, Backspace goes back a key, Ctrl+D /
/// Ctrl+U turn the pages of the open panel. True when the key was one of these.
pub fn handle_key(pending: &mut Pending, panel: Option<&mut Panel>, key: Key) -> bool {
    if pending.keys().is_empty() {
        return false;
    }
    let ctrl = |c| key == Key::new(KeyCode::Char(c), KeyModifiers::CONTROL);
    if key == Key::new(KeyCode::Esc, KeyModifiers::NONE) {
        pending.clear();
    } else if key == Key::new(KeyCode::Backspace, KeyModifiers::NONE) {
        pending.pop();
    } else if let Some(panel) = panel.filter(|_| ctrl('d') || ctrl('u')) {
        panel.page = if ctrl('d') { panel.page + 1 } else { panel.page.saturating_sub(1) };
    } else {
        return false;
    }
    true
}

/// Cells between two columns.
const GAP: usize = 2;
/// The widest a label gets; longer ones are cut with `…`.
const MAX_LABEL: usize = 24;

/// The open panel.
#[derive(Debug, Clone, Default)]
pub struct Panel {
    /// The page shown (Ctrl+D / Ctrl+U), from 0.
    pub page: usize,
}

/// How the entries fit: columns and rows of a page, pages, and the panel's width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub columns: usize,
    pub rows: usize,
    pub pages: usize,
    pub width: u16,
}

/// The cells of one entry: `key → icon label`, the key and the label padded to these widths
/// (the icon is 2 cells and a space, or nothing).
struct Widths {
    key: usize,
    icon: usize,
    label: usize,
}

impl Widths {
    /// The widths for `children` in `room` cells at most.
    fn of(children: &[Child], room: usize, icons: bool) -> Widths {
        let key = children.iter().map(|c| short(c.key).width()).max().unwrap_or(1).max(1);
        let icon = if icons { 3 } else { 0 };
        let label = children.iter().map(|c| label(c).width()).max().unwrap_or(0).min(MAX_LABEL);
        Widths { key, icon, label: label.min(room.saturating_sub(key + 3 + icon)) }
    }

    fn entry(&self) -> usize {
        self.key + 3 + self.icon + self.label
    }
}

/// A group's label starts with `+`.
fn label(c: &Child) -> String {
    if c.group && !c.label.starts_with('+') { format!("+{}", c.label) } else { c.label.clone() }
}

/// The bottom row: how to close or go back, and the page when there are several.
fn footer(page: usize, pages: usize) -> String {
    let keys = format!("Esc {} · Backspace {}", tr("close"), tr("back"));
    match pages {
        0 | 1 => keys,
        _ if page + 1 < pages => format!("{}/{pages} · Ctrl+D {} · {keys}", page + 1, tr("more")),
        _ => format!("{pages}/{pages} · Ctrl+U {} · {keys}", tr("previous")),
    }
}

/// Columns first, then pages: as many rows as `max_height_percent` of `screen` leaves (less
/// the border and the footer), as many columns as the width takes.
pub fn layout(children: &[Child], screen: Rect, max_height_percent: u16, icons: bool) -> Layout {
    let room = (screen.width as usize).saturating_sub(4);
    let entry = Widths::of(children, room, icons).entry();
    let max_columns = ((room + GAP) / (entry + GAP)).max(1);
    let max_rows = (screen.height as usize * max_height_percent as usize / 100).saturating_sub(3).max(1);
    let n = children.len().max(1);
    let columns = n.div_ceil(max_rows).clamp(1, max_columns);
    let rows = n.div_ceil(columns).min(max_rows);
    let pages = n.div_ceil(columns * rows);
    let entries = columns * entry + (columns - 1) * GAP;
    let width = (entries.max(footer(0, pages).width()) + 4).min(screen.width as usize) as u16;
    Layout { columns, rows, pages, width }
}

/// `s` at (`x`, `y`) in at most `room` cells, if that is on the buffer.
fn put(buf: &mut Buffer, x: u16, y: u16, s: &str, room: usize, style: Style) {
    if buf.area.contains((x, y).into()) && room > 0 {
        let room = room.min((buf.area.right() - x) as usize);
        buf.set_stringn(x, y, s, room, style);
    }
}

/// The panel in `settings.corner` of `screen`, `title` in its top border.
pub fn draw(f: &mut Frame, screen: Rect, title: &str, children: &[Child], panel: &Panel, settings: &WhichKey) {
    let l = layout(children, screen, settings.max_height_percent, settings.icons);
    let title = format!(" {title} ");
    let width = (l.width as usize).max(title.width() + 2).min(screen.width as usize) as u16;
    let height = (l.rows as u16 + 3).min(screen.height);
    if width < 3 || height < 2 {
        return;
    }
    let x = if settings.corner.ends_with("left") { screen.x } else { screen.right() - width };
    let y = if settings.corner.starts_with("top") { screen.y } else { screen.bottom() - height };
    let area = Rect::new(x, y, width, height);
    super::media::clear_popup(f, area);
    let p = super::theme::p();
    let block = Block::default().borders(Borders::ALL).border_style(Style::default().fg(p.accent)).title(Span::styled(title, Style::default().add_modifier(Modifier::BOLD)));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 {
        return;
    }
    let w = Widths::of(children, (screen.width as usize).saturating_sub(4), settings.icons);
    let page = panel.page.min(l.pages.saturating_sub(1));
    let per_page = l.columns * l.rows;
    let (left, right) = (inner.x + 1, inner.right().saturating_sub(1));
    let rows = inner.height as usize - 1;
    let buf = f.buffer_mut();
    for (i, c) in children.iter().skip(page * per_page).take(per_page).enumerate() {
        let (column, row) = (i / l.rows, i % l.rows);
        let x = left as usize + column * (w.entry() + GAP);
        if row >= rows || x >= right as usize {
            continue;
        }
        let (x, y) = (x as u16, inner.y + row as u16);
        let room = |at: usize| (right as usize).saturating_sub(x as usize + at);
        put(buf, x, y, &short(c.key), w.key.min(room(0)), Style::default().fg(p.pin).add_modifier(Modifier::BOLD));
        put(buf, x + w.key as u16 + 1, y, "→", room(w.key + 1), Style::default().fg(p.dim));
        if settings.icons {
            put(buf, x + w.key as u16 + 3, y, &c.icon, 2.min(room(w.key + 3)), Style::default());
        }
        let at = w.key + 3 + w.icon;
        let style = if c.group { Style::default().fg(p.accent) } else { Style::default() };
        put(buf, x + at as u16, y, &super::status::cut(&label(c), w.label), w.label.min(room(at)), style);
    }
    let footer = footer(page, l.pages);
    let room = right.saturating_sub(left) as usize;
    put(buf, left, inner.bottom() - 1, &super::status::cut(&footer, room), room, Style::default().fg(p.dim));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::keymap::{Mode, LEADER};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn child(key: &str, icon: &str, label: &str) -> Child {
        Child { key: Key::parse(key).unwrap(), label: label.into(), icon: icon.into(), group: false, action: None }
    }

    fn many(n: usize) -> Vec<Child> {
        (0..n).map(|i| child(&((b'a' + i as u8) as char).to_string(), "😀", &format!("label {i}"))).collect()
    }

    /// The panel drawn on a `w`×`h` screen: each row's cells.
    fn cells(w: u16, h: u16, children: &[Child], panel: &Panel, settings: &WhichKey) -> Vec<Vec<String>> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, f.area(), "␣  message", children, panel, settings)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..h).map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect()).collect()
    }

    /// The screen as text.
    fn render(w: u16, h: u16, children: &[Child], panel: &Panel, settings: &WhichKey) -> String {
        cells(w, h, children, panel, settings).iter().map(|r| r.concat()).collect::<Vec<_>>().join("\n")
    }

    /// The cell column of `needle` in a row.
    fn cell_of(row: &[String], needle: &str) -> Option<usize> {
        row.iter().position(|s| s == needle)
    }

    #[test]
    fn keys_while_pending() {
        let k = |s| Key::parse(s).unwrap();
        let mut pending = Pending::default();
        assert!(!handle_key(&mut pending, None, k("esc")), "nothing under way: Esc is Esc");
        pending.push(LEADER, Mode::Insert);
        pending.push(k("h"), Mode::Insert);
        assert!(handle_key(&mut pending, None, k("backspace")));
        assert_eq!(pending.keys(), &[LEADER], "one key back, nothing erased");
        assert!(!handle_key(&mut pending, None, k("ctrl+d")), "no panel: Ctrl+D is itself");
        let mut panel = Panel::default();
        assert!(handle_key(&mut pending, Some(&mut panel), k("ctrl+d")) && handle_key(&mut pending, Some(&mut panel), k("ctrl+d")));
        assert!(handle_key(&mut pending, Some(&mut panel), k("ctrl+u")));
        assert_eq!(panel.page, 1);
        assert!(!handle_key(&mut pending, Some(&mut panel), k("m")), "other keys go on to the keymap");
        assert!(handle_key(&mut pending, Some(&mut panel), k("esc")));
        assert!(pending.keys().is_empty(), "Esc cancels (and does nothing else)");
    }

    #[test]
    fn title_names_the_group() {
        let (keys, _) = Keymap::load("[global]\nmute = \"<leader> h m\"\n[groups]\n\"<leader> h\" = \"calls\"\n");
        let mut pending = Pending::default();
        pending.push_typed(LEADER, Key::parse("space").unwrap(), Mode::Normal);
        assert_eq!(title(&keys, &[Context::Messages, Context::Global], &pending), "␣  message");
        assert_eq!(title(&keys, &[Context::Composer, Context::Global], &pending), "␣  actions");
        pending.push(Key::parse("h").unwrap(), Mode::Normal);
        assert_eq!(title(&keys, &[Context::Messages, Context::Global], &pending), "␣h  calls");
    }

    #[test]
    fn columns_align_by_display_width() {
        let children = [child("r", "😀", "react"), child("t", "🧵", "open the thread"), child("Y", "😀", "copy link")];
        let screen = cells(60, 20, &children, &Panel::default(), &WhichKey::default());
        let arrows: Vec<usize> = screen.iter().filter_map(|row| cell_of(row, "→")).collect();
        assert_eq!(arrows.len(), 3, "{screen:?}");
        assert!(arrows.iter().all(|x| *x == arrows[0]), "{arrows:?}");
        // The labels start in one column too, after the 2-cell icon.
        for (row, first) in screen.iter().filter(|r| cell_of(r, "→").is_some()).zip(["r", "o", "c"]) {
            assert_eq!(row[arrows[0] + 5], first, "{row:?}");
        }
    }

    #[test]
    fn grows_columns_then_pages() {
        let children = many(18);
        let tall = layout(&children, Rect::new(0, 0, 80, 40), 50, true);
        assert_eq!((tall.columns, tall.pages), (2, 1), "{tall:?}");
        let short = layout(&children, Rect::new(0, 0, 80, 10), 50, true);
        assert!(short.pages > 1, "{short:?}");
        assert!(short.rows <= 5 && short.columns >= 2, "{short:?}");
        let text = render(80, 10, &children, &Panel::default(), &WhichKey::default());
        assert!(text.contains("1/") && text.contains("Ctrl+D"), "{text}");
        let second = render(80, 10, &children, &Panel { page: 1 }, &WhichKey::default());
        assert!(second.contains("2/"), "{second}");
    }

    #[test]
    fn six_row_screen_does_not_panic() {
        for (w, h) in [(80, 6), (20, 6), (5, 3), (1, 1), (0, 0)] {
            let _ = layout(&many(18), Rect::new(0, 0, w, h), 50, true);
            if w > 0 && h > 0 {
                render(w, h, &many(18), &Panel { page: 7 }, &WhichKey::default());
            }
        }
        let l = layout(&many(18), Rect::new(0, 0, 80, 6), 50, true);
        assert!(l.rows >= 1 && l.pages >= 1, "{l:?}");
    }

    #[test]
    fn icons_off_drops_the_column() {
        let children = [child("r", "😀", "react")];
        let on = layout(&children, Rect::new(0, 0, 80, 40), 50, true);
        let off = layout(&children, Rect::new(0, 0, 80, 40), 50, false);
        assert!(off.width <= on.width);
        let settings = WhichKey { icons: false, ..Default::default() };
        let screen = cells(60, 20, &children, &Panel::default(), &settings);
        assert!(!screen.concat().contains(&"😀".to_string()), "{screen:?}");
        let row = screen.iter().find(|r| cell_of(r, "→").is_some()).unwrap();
        let x = cell_of(row, "→").unwrap();
        assert_eq!(row[x + 2], "r", "the label right after the arrow: {row:?}");
    }

    #[test]
    fn long_labels_are_cut() {
        let long = "a label far too long to show in a which-key column at all";
        let children = [child("r", "😀", long), child("s", "😀", "save")];
        let screen = cells(80, 20, &children, &Panel::default(), &WhichKey::default());
        let row = screen.iter().find(|r| r.concat().contains("a label")).unwrap().concat();
        assert!(!row.contains(long) && row.contains('…'), "{row}");
        let narrow = cells(24, 20, &children, &Panel::default(), &WhichKey::default());
        let row = narrow.iter().find(|r| r.concat().contains("a la")).unwrap().concat();
        assert!(row.contains('…'), "{row}");
    }
}

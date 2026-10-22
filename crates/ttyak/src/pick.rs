//! Plain-terminal questions asked before the full-screen TUI starts: a list to choose from and a one-line prompt.
//!
//! Both draw inline below the cursor (no alternate screen) and erase themselves when done, so only what the caller prints
//! afterwards stays in the scrollback. The key handling is a pure state machine ([`Picker`]); [`choose`] and [`ask`] are
//! thin drivers around it.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::io::{self, IsTerminal, Stdout, Write};

use anyhow::Result;
use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use crossterm::terminal::{self, Clear, ClearType, disable_raw_mode, enable_raw_mode};
use crossterm::{cursor, execute, queue, style};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{Terminal, TerminalOptions, Viewport};
use unicode_width::UnicodeWidthStr;

/// One choosable line. `detail` follows the text dimly, `note` ends the line dimly (`as Dmytro`, `already signed in`).
/// A disabled item is drawn dim and the cursor never lands on it.
pub struct Item {
    pub text: String,
    pub detail: String,
    pub note: String,
    pub enabled: bool,
}

/// A heading names a group of items; one without a title is drawn all dim (a note such as why a group is empty, or a
/// blank separating line when its detail is empty too).
pub enum Row {
    Heading { title: String, detail: String },
    Item(Item),
}

pub enum Outcome {
    /// Indices into the rows given to the picker, in row order.
    Picked(Vec<usize>),
    Cancelled,
}

/// The state of a list question: cursor, marks, filter and scroll position.
pub struct Picker {
    rows: Vec<Row>,
    multi: bool,
    cursor: Option<usize>,
    marks: BTreeSet<usize>,
    filter: String,
    /// Typed letters go to the filter (after `/`, until Enter or Esc).
    filtering: bool,
    /// First shown position of the last `visible` window (what a click's row number counts from).
    scroll: Cell<usize>,
    outcome: Option<Outcome>,
}

impl Picker {
    pub fn new(rows: Vec<Row>, multi: bool) -> Picker {
        Picker::with_marks(rows, multi, &[])
    }

    /// A picker whose rows `marked` start marked (multi-choice only); indices that are not items that can be picked are
    /// left out.
    pub fn with_marks(rows: Vec<Row>, multi: bool, marked: &[usize]) -> Picker {
        let mut p = Picker { rows, multi, cursor: None, marks: BTreeSet::new(), filter: String::new(), filtering: false, scroll: Cell::new(0), outcome: None };
        if multi {
            p.marks = marked.iter().copied().filter(|&i| i < p.rows.len() && p.selectable(i)).collect();
        }
        p.fix_cursor();
        p
    }

    /// The outcome a click produced (a click on a single-choice list picks it); `key` returns the others.
    pub fn done(&mut self) -> Option<Outcome> {
        self.outcome.take()
    }

    fn selectable(&self, i: usize) -> bool {
        matches!(&self.rows[i], Row::Item(it) if it.enabled)
    }

    /// The title and detail of the heading above row `i`.
    fn heading_of(&self, i: usize) -> Option<(&str, &str)> {
        self.rows[..i].iter().rev().find_map(|r| match r {
            Row::Heading { title, detail } => Some((title.as_str(), detail.as_str())),
            Row::Item(_) => None,
        })
    }

    fn matches(&self, i: usize) -> bool {
        let Row::Item(it) = &self.rows[i] else { return false };
        let needle = self.filter.to_lowercase();
        let (ht, hd) = self.heading_of(i).unwrap_or(("", ""));
        [&it.text, &it.detail, &it.note, ht, hd].iter().any(|s| s.to_lowercase().contains(&needle))
    }

    /// Row indices that pass the filter: matching items, and a heading while any of its items is shown.
    fn shown(&self) -> Vec<usize> {
        if self.filter.is_empty() {
            return (0..self.rows.len()).collect();
        }
        let mut out = Vec::new();
        let mut heading: Option<usize> = None;
        for i in 0..self.rows.len() {
            match &self.rows[i] {
                Row::Heading { .. } => heading = Some(i),
                Row::Item(_) if self.matches(i) => {
                    if let Some(h) = heading.take() {
                        out.push(h);
                    }
                    out.push(i);
                }
                Row::Item(_) => {}
            }
        }
        out
    }

    /// Keeps the cursor on a shown, selectable row (the first one when it was filtered away).
    fn fix_cursor(&mut self) {
        let shown = self.shown();
        if self.cursor.is_some_and(|c| shown.contains(&c)) {
            return;
        }
        self.cursor = shown.into_iter().find(|&i| self.selectable(i));
    }

    fn step(&mut self, down: bool) {
        let shown = self.shown();
        let Some(pos) = self.cursor.and_then(|c| shown.iter().position(|&i| i == c)) else { return };
        let next = if down { shown[pos + 1..].iter().find(|&&i| self.selectable(i)) } else { shown[..pos].iter().rev().find(|&&i| self.selectable(i)) };
        if let Some(&n) = next {
            self.cursor = Some(n);
        }
    }

    fn accept(&mut self) -> Option<Outcome> {
        if self.multi && !self.marks.is_empty() {
            return Some(Outcome::Picked(self.marks.iter().copied().collect()));
        }
        self.cursor.map(|c| Outcome::Picked(vec![c]))
    }

    pub fn key(&mut self, k: KeyEvent) -> Option<Outcome> {
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
            return Some(Outcome::Cancelled);
        }
        match k.code {
            KeyCode::Up => self.step(false),
            KeyCode::Down => self.step(true),
            KeyCode::Esc if !self.filter.is_empty() || self.filtering => {
                self.filter.clear();
                self.filtering = false;
                self.fix_cursor();
            }
            KeyCode::Esc => return Some(Outcome::Cancelled),
            KeyCode::Enter if self.filtering => self.filtering = false,
            KeyCode::Enter => return self.accept(),
            KeyCode::Backspace if self.filtering => {
                self.filter.pop();
                self.fix_cursor();
            }
            KeyCode::Char(c) if self.filtering && !k.modifiers.contains(KeyModifiers::CONTROL) => {
                self.filter.push(c);
                self.fix_cursor();
            }
            KeyCode::Char('/') => self.filtering = true,
            KeyCode::Char('j') => self.step(true),
            KeyCode::Char('k') => self.step(false),
            KeyCode::Char(' ') if self.multi => {
                if let Some(c) = self.cursor
                    && !self.marks.remove(&c)
                {
                    self.marks.insert(c);
                }
            }
            _ => {}
        }
        None
    }

    /// A left click on the `visible_row`-th line of the last `visible` window: moves the cursor there and toggles its mark
    /// (picks it when the list is single choice). Headings and dim items do nothing.
    pub fn click(&mut self, visible_row: usize) {
        let Some(&i) = self.shown().get(self.scroll.get() + visible_row) else { return };
        if !self.selectable(i) {
            return;
        }
        self.cursor = Some(i);
        if !self.multi {
            self.outcome = Some(Outcome::Picked(vec![i]));
        } else if !self.marks.remove(&i) {
            self.marks.insert(i);
        }
    }

    /// The lines to draw in a window of `height` lines: (row index, cursor is here, marked). The window scrolls to keep the
    /// cursor in view (and the heading above it when it is the first item of its group).
    pub fn visible(&self, height: usize) -> Vec<(usize, bool, bool)> {
        let shown = self.shown();
        let mut start = self.scroll.get().min(shown.len().saturating_sub(height));
        if let Some(pos) = self.cursor.and_then(|c| shown.iter().position(|&i| i == c)) {
            let pos_top = if pos > 0 && matches!(self.rows[shown[pos - 1]], Row::Heading { .. }) { pos - 1 } else { pos };
            if pos_top < start {
                start = pos_top;
            } else if pos >= start + height {
                start = pos + 1 - height;
            }
        }
        self.scroll.set(start);
        shown.into_iter().skip(start).take(height).map(|i| (i, self.cursor == Some(i), self.marks.contains(&i))).collect()
    }

    fn hint(&self) -> Line<'static> {
        let dim = Style::new().add_modifier(Modifier::DIM);
        if self.filtering {
            Line::from(vec![Span::raw(format!("/{}", self.filter)), Span::styled("  (Enter keeps, Esc drops)", dim)])
        } else if !self.filter.is_empty() {
            Line::from(Span::styled(format!("filter: {}  (Esc drops it)", self.filter), dim))
        } else {
            let mark = if self.multi { " · Space marks" } else { "" };
            Line::from(Span::styled(format!("↑↓ move{mark} · Enter ok · / filter · Esc cancel"), dim))
        }
    }

    fn lines(&self, question: &str, height: usize) -> Vec<Line<'static>> {
        let dim = Style::new().add_modifier(Modifier::DIM);
        let bold = Style::new().add_modifier(Modifier::BOLD);
        let mut out = vec![Line::from(Span::styled(question.to_string(), bold))];
        for (i, cursor, marked) in self.visible(height) {
            match &self.rows[i] {
                Row::Heading { title, detail } if title.is_empty() => out.push(Line::from(Span::styled(detail.clone(), dim))),
                Row::Heading { title, detail } => {
                    let mut spans = vec![Span::styled(title.clone(), bold)];
                    if !detail.is_empty() {
                        spans.push(Span::styled(format!("  {detail}"), dim));
                    }
                    out.push(Line::from(spans));
                }
                Row::Item(it) => {
                    let base = if it.enabled { Style::new() } else { dim };
                    let mut spans = vec![Span::styled(if cursor { "› " } else { "  " }, bold)];
                    if self.multi {
                        spans.push(Span::styled(if marked { "[x] " } else { "[ ] " }, base));
                    }
                    spans.push(Span::styled(it.text.clone(), if cursor { base.add_modifier(Modifier::BOLD) } else { base }));
                    for extra in [&it.detail, &it.note] {
                        if !extra.is_empty() {
                            spans.push(Span::styled(format!("  {extra}"), dim));
                        }
                    }
                    out.push(Line::from(spans));
                }
            }
        }
        out.push(self.hint());
        out
    }
}

/// True when both stdin and stdout are terminals, so a question can be asked.
pub fn interactive() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

/// Raw mode, mouse capture and the inline viewport for as long as it lives; `Drop` puts the terminal back and erases the
/// viewport on every exit path (also an error or a panic).
struct Session {
    term: Terminal<CrosstermBackend<Stdout>>,
}

impl Session {
    fn start(viewport_height: u16) -> Result<Session> {
        enable_raw_mode()?;
        let setup = || -> Result<Terminal<CrosstermBackend<Stdout>>> {
            execute!(io::stdout(), EnableMouseCapture)?;
            Ok(Terminal::with_options(CrosstermBackend::new(io::stdout()), TerminalOptions { viewport: Viewport::Inline(viewport_height) })?)
        };
        match setup() {
            Ok(term) => Ok(Session { term }),
            Err(e) => {
                let _ = execute!(io::stdout(), DisableMouseCapture);
                let _ = disable_raw_mode();
                Err(e)
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // `clear` puts the cursor back where the last drawing left it (inside the erased list): what is printed next
        // goes where the list started.
        let _ = self.term.clear();
        let top = self.term.get_frame().area().as_position();
        let _ = self.term.set_cursor_position(top);
        let _ = self.term.show_cursor();
        let _ = execute!(io::stdout(), DisableMouseCapture);
        let _ = disable_raw_mode();
    }
}

/// Throws away input typed before the question was drawn (keys pressed while ttyak was looking for something): an
/// Enter typed then would otherwise pick the first row the moment the list appears. Raw mode must be on, so a typed
/// but unfinished line is given up as well.
fn drain_input() -> Result<()> {
    while event::poll(std::time::Duration::ZERO)? {
        let _ = event::read()?;
    }
    Ok(())
}

/// Asks a list question inline and erases it when answered. Ctrl+C and Esc (without a filter) give `Cancelled`.
pub fn choose(question: &str, rows: Vec<Row>, multi: bool) -> Result<Outcome> {
    run(question, Picker::new(rows, multi))
}

/// [`choose`] with several to pick, the rows `marked` marked at the start.
pub fn choose_marked(question: &str, rows: Vec<Row>, marked: &[usize]) -> Result<Outcome> {
    run(question, Picker::with_marks(rows, true, marked))
}

/// The driver of [`choose`] and [`choose_marked`].
fn run(question: &str, mut picker: Picker) -> Result<Outcome> {
    let term_h = terminal::size()?.1 as usize;
    let list_h = picker.rows.len().min(term_h.saturating_sub(2)).max(1);
    let mut session = Session::start((list_h + 2) as u16)?;
    drain_input()?;
    let mut area = Rect::default();
    loop {
        session.term.draw(|f| {
            area = f.area();
            let lines = picker.lines(question, list_h);
            f.render_widget(Paragraph::new(lines), area);
        })?;
        match event::read()? {
            Event::Key(k) if k.kind != KeyEventKind::Release => {
                if let Some(o) = picker.key(k) {
                    return Ok(o);
                }
            }
            Event::Mouse(m) if m.kind == MouseEventKind::Down(MouseButton::Left) => {
                if m.row > area.y && (m.row - area.y) as usize <= list_h {
                    picker.click((m.row - area.y - 1) as usize);
                    if let Some(o) = picker.done() {
                        return Ok(o);
                    }
                }
            }
            _ => {}
        }
    }
}

/// The parts of the prompt line: the prompt, the typed text, and the dim placeholder that follows it.
fn prompt_line(prompt: &str, typed: &str, placeholder: &dyn Fn(&str) -> String) -> (String, String, String) {
    (prompt.to_string(), typed.to_string(), placeholder(typed))
}

/// The parts of a secret's prompt line: nothing of what is typed is drawn, only how much (dim).
fn secret_line(prompt: &str, typed: &str) -> (String, String, String) {
    let dim = if typed.is_empty() { "paste it, then Enter".to_string() } else { format!("{} characters", typed.chars().count()) };
    (prompt.to_string(), String::new(), dim)
}

fn draw_prompt((p, t, dim): (String, String, String)) -> Result<()> {
    let mut out = io::stdout();
    queue!(out, cursor::MoveToColumn(0), Clear(ClearType::CurrentLine), style::Print(&p), style::Print(&t))?;
    queue!(out, style::SetAttribute(style::Attribute::Dim), style::Print(&dim), style::SetAttribute(style::Attribute::Reset))?;
    let col = (p.as_str().width() + t.as_str().width()).min(u16::MAX as usize) as u16;
    queue!(out, cursor::MoveToColumn(col))?;
    out.flush()?;
    Ok(())
}

/// A one-line text prompt. `placeholder(typed)` is drawn dim after the cursor. Enter gives the text; Esc and Ctrl+C give
/// `None`. The line is erased afterwards.
pub fn ask(prompt: &str, placeholder: impl Fn(&str) -> String) -> Result<Option<String>> {
    ask_with(prompt, "", placeholder)
}

/// [`ask`] with `initial` already typed (Enter keeps it; Backspace takes it away a character at a time).
pub fn ask_with(prompt: &str, initial: &str, placeholder: impl Fn(&str) -> String) -> Result<Option<String>> {
    read_line(&|typed| prompt_line(prompt, typed, &placeholder), initial, &|t: &str| t.to_string(), false, &mut |_, _| Ok(()))
}

/// The prompt for a sign-in code: `___-___` filled in as it is typed, the hyphen put in by itself. Only letters and
/// digits are taken (capitalised, six at most), so a typed or pasted hyphen or space does nothing. Enter gives the
/// letters and digits typed (maybe fewer than six); Esc and Ctrl+C give `None`. `resized(width, height)` is called
/// with the cursor at the start of the cleared prompt line when the terminal's size is new (also once at the start,
/// for a resize while nothing was asked); the prompt is drawn again after it, with what was typed.
pub fn ask_code(prompt: &str, resized: &mut dyn FnMut(u16, u16) -> Result<()>) -> Result<Option<String>> {
    read_line(&|typed| {
        let (shown, rest) = code_mask(typed);
        (prompt.to_string(), shown, rest)
    }, "", &code_chars, true, resized)
}

/// What the code prompt keeps of `typed`: its letters and digits, capitalised, six at most.
fn code_chars(typed: &str) -> String {
    typed.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_uppercase()).take(6).collect()
}

/// The code `typed` (from [`code_chars`]) in its mask `___-___`: what is typed, with the hyphen once three are, and
/// the rest of the mask (drawn dim).
fn code_mask(typed: &str) -> (String, String) {
    const MASK: &str = "___-___";
    let mut shown = String::new();
    for (i, c) in typed.chars().enumerate() {
        shown.push(c);
        if i == 2 {
            shown.push('-');
        }
    }
    let rest = MASK.chars().skip(shown.chars().count()).collect();
    (shown, rest)
}

/// A one-line prompt for a secret (a cookie): what is typed or pasted is never drawn, only how many characters it has.
/// Enter gives the text; Esc and Ctrl+C give `None`.
pub fn ask_secret(prompt: &str) -> Result<Option<String>> {
    read_line(&|typed| secret_line(prompt, typed), "", &|t: &str| t.to_string(), false, &mut |_, _| Ok(()))
}

/// The prompt loop of [`ask_with`], [`ask_code`] and [`ask_secret`]: `line(typed)` gives what to draw, `initial` is
/// typed already, `keep(typed)` is what is kept of the text after each key or paste, `paste` turns on bracketed
/// paste (a paste then comes as one piece; without it, or where the terminal has none, as keys), and `resized` is
/// told the terminal's size at the start and on each resize (see [`ask_code`]).
fn read_line(line: &dyn Fn(&str) -> (String, String, String), initial: &str, keep: &dyn Fn(&str) -> String, paste: bool, resized: &mut dyn FnMut(u16, u16) -> Result<()>) -> Result<Option<String>> {
    struct Raw(bool);
    impl Drop for Raw {
        fn drop(&mut self) {
            if self.0 {
                let _ = execute!(io::stdout(), event::DisableBracketedPaste);
            }
            let _ = execute!(io::stdout(), cursor::MoveToColumn(0), Clear(ClearType::CurrentLine));
            let _ = disable_raw_mode();
        }
    }
    enable_raw_mode()?;
    let _raw = Raw(paste);
    if paste {
        execute!(io::stdout(), event::EnableBracketedPaste)?;
    }
    // A resize while ttyak was busy is thrown away with the keys: the size is told once here instead.
    drain_input()?;
    let (w, h) = terminal::size()?;
    resized(w, h)?;
    let mut typed = initial.to_string();
    loop {
        draw_prompt(line(&typed))?;
        let ev = event::read()?;
        if let Event::Resize(w, h) = ev {
            execute!(io::stdout(), cursor::MoveToColumn(0), Clear(ClearType::CurrentLine))?;
            resized(w, h)?;
            continue;
        }
        if let Some(enter) = apply(&mut typed, &ev, keep) {
            return Ok(enter.then_some(typed));
        }
    }
}

/// What a key or a paste does to the prompt's text `typed` (`keep` says what is kept of it): `Some(true)` for Enter,
/// `Some(false)` for Esc and Ctrl+C, `None` to go on asking.
fn apply(typed: &mut String, ev: &Event, keep: &dyn Fn(&str) -> String) -> Option<bool> {
    match ev {
        Event::Paste(text) => {
            typed.push_str(text);
            *typed = keep(typed);
        }
        Event::Key(k) if k.kind != KeyEventKind::Release => {
            let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
            match k.code {
                KeyCode::Char('c') if ctrl => return Some(false),
                KeyCode::Esc => return Some(false),
                KeyCode::Enter => return Some(true),
                KeyCode::Backspace => {
                    typed.pop();
                }
                KeyCode::Char(c) if !ctrl && !k.modifiers.contains(KeyModifiers::ALT) => {
                    typed.push(c);
                    *typed = keep(typed);
                }
                _ => {}
            }
        }
        _ => {}
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn k(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }
    fn ch(c: char) -> KeyEvent {
        k(KeyCode::Char(c))
    }
    fn item(text: &str, enabled: bool) -> Row {
        Row::Item(Item { text: text.into(), detail: String::new(), note: String::new(), enabled })
    }
    fn heading(title: &str) -> Row {
        Row::Heading { title: title.into(), detail: String::new() }
    }
    /// Heading, a, dim b, Heading, c.
    fn sample(multi: bool) -> Picker {
        Picker::new(vec![heading("One"), item("a", true), item("b", false), heading("Two"), item("c", true)], multi)
    }
    fn cursor_row(p: &Picker, height: usize) -> Option<usize> {
        p.visible(height).into_iter().find(|v| v.1).map(|v| v.0)
    }

    #[test]
    fn the_cursor_skips_headings_and_dim_items() {
        let mut p = sample(false);
        assert_eq!(cursor_row(&p, 10), Some(1));
        p.key(k(KeyCode::Down));
        assert_eq!(cursor_row(&p, 10), Some(4));
        p.key(k(KeyCode::Down)); // no wrap
        assert_eq!(cursor_row(&p, 10), Some(4));
        p.key(ch('k'));
        assert_eq!(cursor_row(&p, 10), Some(1));
        p.key(k(KeyCode::Up));
        assert_eq!(cursor_row(&p, 10), Some(1));
    }

    #[test]
    fn space_marks_and_enter_returns_the_marked() {
        let mut p = sample(true);
        p.key(ch(' '));
        p.key(ch('j'));
        p.key(ch(' '));
        assert_eq!(p.visible(10).iter().filter(|v| v.2).count(), 2);
        assert!(matches!(p.key(k(KeyCode::Enter)), Some(Outcome::Picked(v)) if v == vec![1, 4]));
        let mut p = sample(true);
        p.key(ch(' '));
        p.key(ch(' ')); // toggled off again
        assert!(matches!(p.key(k(KeyCode::Enter)), Some(Outcome::Picked(v)) if v == vec![1]));
    }

    #[test]
    fn enter_without_marks_takes_the_cursor() {
        let mut p = sample(true);
        p.key(ch('j'));
        assert!(matches!(p.key(k(KeyCode::Enter)), Some(Outcome::Picked(v)) if v == vec![4]));
    }

    #[test]
    fn marked_rows_start_marked() {
        // Row 0 is a heading and row 2 is dim: only row 4 can start marked.
        let mut p = Picker::with_marks(vec![heading("One"), item("a", true), item("b", false), heading("Two"), item("c", true)], true, &[4, 0, 2]);
        assert_eq!(p.visible(10).iter().filter(|v| v.2).map(|v| v.0).collect::<Vec<_>>(), vec![4]);
        assert!(matches!(p.key(k(KeyCode::Enter)), Some(Outcome::Picked(v)) if v == vec![4]));
    }

    #[test]
    fn single_choice_ignores_space() {
        let mut p = sample(false);
        assert!(p.key(ch(' ')).is_none());
        assert!(p.visible(10).iter().all(|v| !v.2));
        assert!(matches!(p.key(k(KeyCode::Enter)), Some(Outcome::Picked(v)) if v == vec![1]));
    }

    #[test]
    fn slash_filters_esc_drops_it_then_cancels() {
        let mut p = sample(false);
        p.key(ch('/'));
        p.key(ch('C'));
        // heading "Two" and item c remain
        assert_eq!(p.visible(10).iter().map(|v| v.0).collect::<Vec<_>>(), vec![3, 4]);
        assert_eq!(cursor_row(&p, 10), Some(4));
        p.key(k(KeyCode::Backspace));
        assert_eq!(p.visible(10).len(), 5);
        p.key(ch('c'));
        p.key(k(KeyCode::Enter)); // keeps the filter, back to moving
        assert_eq!(p.visible(10).len(), 2);
        assert!(p.key(k(KeyCode::Esc)).is_none()); // drops it
        assert_eq!(p.visible(10).len(), 5);
        assert!(matches!(p.key(k(KeyCode::Esc)), Some(Outcome::Cancelled)));
    }

    #[test]
    fn esc_inside_a_filter_drops_it_without_cancelling() {
        let mut p = sample(false);
        p.key(ch('/'));
        p.key(ch('c'));
        assert!(p.key(k(KeyCode::Esc)).is_none());
        assert_eq!(p.visible(10).len(), 5);
    }

    #[test]
    fn j_and_k_never_filter() {
        let mut p = sample(false);
        p.key(ch('j'));
        p.key(ch('k'));
        assert_eq!(p.visible(10).len(), 5);
        p.key(ch('/'));
        p.key(ch('j')); // now it is filter text
        assert!(p.visible(10).is_empty());
    }

    #[test]
    fn a_click_moves_and_marks() {
        let mut p = sample(true);
        p.click(4);
        assert_eq!(cursor_row(&p, 10), Some(4));
        assert_eq!(p.visible(10).iter().find(|v| v.0 == 4).map(|v| v.2), Some(true));
        p.click(4);
        assert_eq!(p.visible(10).iter().find(|v| v.0 == 4).map(|v| v.2), Some(false));
        p.click(0); // a heading does nothing
        p.click(2); // a dim item does nothing
        assert_eq!(cursor_row(&p, 10), Some(4));
        assert!(p.done().is_none());
    }

    #[test]
    fn a_click_picks() {
        let mut p = sample(false);
        p.click(4);
        assert!(matches!(p.done(), Some(Outcome::Picked(v)) if v == vec![4]));
    }

    #[test]
    fn a_long_list_scrolls_with_the_cursor() {
        let rows = (0..30).map(|i| item(&format!("row {i}"), true)).collect();
        let mut p = Picker::new(rows, false);
        for _ in 0..20 {
            p.key(k(KeyCode::Down));
            let v = p.visible(8);
            assert_eq!(v.len(), 8);
            assert!(v.iter().any(|x| x.1));
        }
        assert_eq!(cursor_row(&p, 8), Some(20));
        for _ in 0..20 {
            p.key(k(KeyCode::Up));
            assert!(p.visible(8).iter().any(|x| x.1));
        }
        assert_eq!(p.visible(8)[0].0, 0);
        // a click on the visible row 3 lands on the row shown there
        for _ in 0..12 {
            p.key(k(KeyCode::Down));
        }
        let shown = p.visible(8)[3].0;
        p.click(3);
        assert_eq!(cursor_row(&p, 8), Some(shown));
    }

    #[test]
    fn ctrl_c_cancels() {
        let mut p = sample(true);
        assert!(matches!(p.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)), Some(Outcome::Cancelled)));
        let mut p = sample(true);
        p.key(ch('/'));
        assert!(matches!(p.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)), Some(Outcome::Cancelled)));
    }

    #[test]
    fn a_secret_is_never_drawn() {
        let (p, shown, dim) = secret_line("d cookie: ", "xoxd-abc%2Fdef");
        assert_eq!((p.as_str(), shown.as_str()), ("d cookie: ", ""));
        assert_eq!(dim, "14 characters");
        assert_eq!(secret_line("d cookie: ", "").2, "paste it, then Enter");
    }

    /// The code prompt's text after `events`, from nothing typed.
    fn code_after(events: &[Event]) -> String {
        let mut typed = String::new();
        for e in events {
            assert!(apply(&mut typed, e, &code_chars).is_none());
        }
        typed
    }
    fn keys(text: &str) -> Vec<Event> {
        text.chars().map(|c| Event::Key(ch(c))).collect()
    }

    #[test]
    fn the_code_fills_its_mask() {
        assert_eq!(code_mask(""), (String::new(), "___-___".to_string()));
        assert_eq!(code_mask("RE"), ("RE".to_string(), "_-___".to_string()));
        // Three typed: the hyphen is put in, and the next one goes after it.
        assert_eq!(code_mask("REP"), ("REP-".to_string(), "___".to_string()));
        assert_eq!(code_mask("REPO"), ("REP-O".to_string(), "__".to_string()));
        assert_eq!(code_mask("REPOEU"), ("REP-OEU".to_string(), String::new()));
    }

    #[test]
    fn the_code_takes_letters_and_digits_capitalised() {
        assert_eq!(code_chars("a-b c1!"), "ABC1");
        assert_eq!(code_after(&keys("a-b c1!")), "ABC1");
        // At most six; the seventh does nothing.
        assert_eq!(code_after(&keys("rep-oeuX")), "REPOEU");
        // Pasted with a hyphen: as keys (no bracketed paste) or as one paste.
        assert_eq!(code_after(&keys(" REP-OEU ")), "REPOEU");
        assert_eq!(code_after(&[Event::Paste("rep-oeu".into())]), "REPOEU");
        assert_eq!(code_after(&[Event::Paste("rep-oeu-extra".into())]), "REPOEU");
        // Backspace takes the last letter: the hyphen is only drawn, so it is stepped over.
        let mut typed = code_after(&keys("repo"));
        assert!(apply(&mut typed, &Event::Key(k(KeyCode::Backspace)), &code_chars).is_none());
        assert_eq!(code_mask(&typed), ("REP-".to_string(), "___".to_string()));
        assert_eq!(apply(&mut typed, &Event::Key(k(KeyCode::Enter)), &code_chars), Some(true));
        assert_eq!(apply(&mut typed, &Event::Key(k(KeyCode::Esc)), &code_chars), Some(false));
    }

    #[test]
    fn other_prompts_keep_what_is_typed() {
        let same = |t: &str| t.to_string();
        let mut typed = String::new();
        for e in keys("a-b c") {
            apply(&mut typed, &e, &same);
        }
        apply(&mut typed, &Event::Paste("1!".into()), &same);
        assert_eq!(typed, "a-b c1!");
    }

    #[test]
    fn the_placeholder_follows_the_text() {
        let ph = |t: &str| if t.contains('.') { String::new() } else { ".slack.com".to_string() };
        assert_eq!(prompt_line("Workspace: ", "", &ph), ("Workspace: ".to_string(), String::new(), ".slack.com".to_string()));
        assert_eq!(prompt_line("Workspace: ", "acme", &ph).2, ".slack.com");
        assert_eq!(prompt_line("Workspace: ", "acme.example.com", &ph).2, "");
    }
}

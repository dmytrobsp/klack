//! Terminal setup and guaranteed restoration (also on panic).

use std::io::{self, Stdout};

use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste, EnableFocusChange, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement};
use ratatui::Terminal;
use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::{Cell, CellWidth};
use ratatui::layout::{Position, Size};

pub type Tui = Terminal<WideSafe<CrosstermBackend<Stdout>>>;

/// Drops writes into columns that a just-written wide character covers.
///
/// ratatui-core 0.1.2 re-emits the trailing cell of VS16 emoji (❤️) right after the emoji. The
/// crossterm backend treats that cell as adjacent and skips the cursor move, but the terminal
/// has already advanced two columns, so the blank lands one column too far right and shifts
/// the rest of the row (stale characters later). The covered column needs no write anyway.
pub struct WideSafe<B>(pub B);

/// The cells of a diff without those inside a wide character written just before them.
pub fn drop_covered<'a>(cells: impl Iterator<Item = (u16, u16, &'a Cell)>) -> impl Iterator<Item = (u16, u16, &'a Cell)> {
    let mut covered: Option<(u16, u16)> = None; // (row, first free column)
    cells.filter(move |(x, y, cell)| {
        if covered.is_some_and(|(row, end)| *y == row && *x < end) {
            return false;
        }
        let width = cell.symbol().cell_width();
        covered = (width > 1).then_some((*y, x + width));
        true
    })
}

impl<B: Backend> Backend for WideSafe<B> {
    type Error = B::Error;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.0.draw(drop_covered(content))
    }
    fn append_lines(&mut self, n: u16) -> Result<(), Self::Error> {
        self.0.append_lines(n)
    }
    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.0.hide_cursor()
    }
    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.0.show_cursor()
    }
    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        self.0.get_cursor_position()
    }
    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        self.0.set_cursor_position(position)
    }
    fn clear(&mut self) -> Result<(), Self::Error> {
        self.0.clear()
    }
    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        self.0.clear_region(clear_type)
    }
    fn size(&self) -> Result<Size, Self::Error> {
        self.0.size()
    }
    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        self.0.window_size()
    }
    fn flush(&mut self) -> Result<(), Self::Error> {
        self.0.flush()
    }
}

/// Restores the terminal when dropped.
pub struct TerminalGuard {
    keyboard_enhanced: bool,
}

impl TerminalGuard {
    pub fn enter() -> io::Result<(Self, Tui)> {
        enable_raw_mode()?;
        let mut out = io::stdout();
        execute!(out, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste, EnableFocusChange)?;
        // Lets Shift+Enter be told apart from Enter where the terminal supports it.
        let keyboard_enhanced = supports_keyboard_enhancement().unwrap_or(false);
        if keyboard_enhanced {
            execute!(out, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES))?;
        }
        install_panic_hook(keyboard_enhanced);
        let terminal = Terminal::new(WideSafe(CrosstermBackend::new(out)))?;
        Ok((TerminalGuard { keyboard_enhanced }, terminal))
    }
}

fn restore(keyboard_enhanced: bool) {
    let mut out = io::stdout();
    if keyboard_enhanced {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(out, DisableFocusChange, DisableBracketedPaste, DisableMouseCapture, LeaveAlternateScreen);
    let _ = disable_raw_mode();
}

fn install_panic_hook(keyboard_enhanced: bool) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore(keyboard_enhanced);
        previous(info);
    }));
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore(self.keyboard_enhanced);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covered_columns_are_dropped() {
        let cell = |s: &str| {
            let mut c = Cell::default();
            c.set_symbol(s);
            c
        };
        let cells = [cell("["), cell("\u{2764}\u{fe0f}"), cell(" "), cell(" "), cell("1")];
        let diff = [(0, 0, &cells[0]), (1, 0, &cells[1]), (2, 0, &cells[2]), (3, 0, &cells[3]), (4, 0, &cells[4]), (2, 1, &cells[2])];
        let kept: Vec<(u16, u16)> = drop_covered(diff.into_iter()).map(|(x, y, _)| (x, y)).collect();
        assert_eq!(kept, vec![(0, 0), (1, 0), (3, 0), (4, 0), (2, 1)]);
    }
}

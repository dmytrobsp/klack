//! Terminal setup and guaranteed restoration (also on panic).

use std::io::{self, Stdout};

use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste, EnableFocusChange, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

pub type Tui = Terminal<CrosstermBackend<Stdout>>;

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
        let terminal = Terminal::new(CrosstermBackend::new(out))?;
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

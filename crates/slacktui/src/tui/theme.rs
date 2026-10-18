//! Colour themes. Everything drawn takes its colours from the current palette, which the
//! preferences can switch at any time.

use std::sync::RwLock;

use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub name: &'static str,
    /// Focused borders, titles, key names.
    pub accent: Color,
    /// Background of the selected row.
    pub selection: Color,
    /// Background of the row under the mouse pointer.
    pub hover: Color,
    /// Secondary text: times, hints, separators.
    pub dim: Color,
    /// Unread conversations.
    pub strong: Color,
    /// Online dots, values in the preferences.
    pub good: Color,
    /// Errors, delete questions.
    pub danger: Color,
    /// The bar along your own messages; huddles.
    pub own: Color,
    pub mention: Color,
    pub link: Color,
    pub code_fg: Color,
    pub code_bg: Color,
    pub quote_bar: Color,
    /// Mention-count badges.
    pub badge_fg: Color,
    pub badge_bg: Color,
    pub pin: Color,
    /// Background of the call bar.
    pub call_bg: Color,
    /// Author names, picked per user.
    pub authors: [Color; 10],
    /// syntect theme for snippets.
    pub syntax: &'static str,
}

const fn rgb(v: u32) -> Color {
    Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

pub const DARK: Palette = Palette {
    name: "dark",
    accent: rgb(0x61afef),
    selection: rgb(0x2c3e5c),
    hover: rgb(0x262a33),
    dim: rgb(0x8a8a8a),
    strong: Color::White,
    good: rgb(0x98c379),
    danger: rgb(0xe06c75),
    own: rgb(0x2bac76),
    mention: rgb(0x1d9bd1),
    link: rgb(0x5da9f6),
    code_fg: rgb(0xe8912d),
    code_bg: rgb(0x2b2f3a),
    quote_bar: rgb(0x6e6e6e),
    badge_fg: Color::White,
    badge_bg: rgb(0xe01e5a),
    pin: rgb(0xe5c07b),
    call_bg: rgb(0x1b2b22),
    authors: [rgb(0xe06c75), rgb(0x98c379), rgb(0xe5c07b), rgb(0x61afef), rgb(0xc678dd), rgb(0x56b6c2), rgb(0xd19a66), rgb(0xbe5046), rgb(0x7fb98a), rgb(0xa98de0)],
    syntax: "base16-eighties.dark",
};

/// For terminals with a light background.
pub const LIGHT: Palette = Palette {
    name: "light",
    accent: rgb(0x1264a3),
    selection: rgb(0xd4e4f7),
    hover: rgb(0xeef2f7),
    dim: rgb(0x6b6b6b),
    strong: Color::Black,
    good: rgb(0x2e7d32),
    danger: rgb(0xc62828),
    own: rgb(0x007a5a),
    mention: rgb(0x1264a3),
    link: rgb(0x0b4c8c),
    code_fg: rgb(0xb3361e),
    code_bg: rgb(0xeeeeee),
    quote_bar: rgb(0x9e9e9e),
    badge_fg: Color::White,
    badge_bg: rgb(0xc2185b),
    pin: rgb(0x9a6700),
    call_bg: rgb(0xe3f2e8),
    authors: [rgb(0xb3261e), rgb(0x2e7d32), rgb(0x8a6d00), rgb(0x1565c0), rgb(0x7b1fa2), rgb(0x00838f), rgb(0xa0522d), rgb(0x6d4c41), rgb(0x33691e), rgb(0x4527a0)],
    syntax: "InspiredGitHub",
};

/// The terminal's own 16 colours, so a custom terminal palette shows through.
pub const TERMINAL: Palette = Palette {
    name: "terminal",
    accent: Color::Blue,
    selection: Color::DarkGray,
    hover: Color::Reset,
    dim: Color::DarkGray,
    strong: Color::Reset,
    good: Color::Green,
    danger: Color::Red,
    own: Color::Green,
    mention: Color::Cyan,
    link: Color::Blue,
    code_fg: Color::Yellow,
    code_bg: Color::Reset,
    quote_bar: Color::DarkGray,
    badge_fg: Color::White,
    badge_bg: Color::Red,
    pin: Color::Yellow,
    call_bg: Color::Reset,
    authors: [Color::Red, Color::Green, Color::Yellow, Color::Blue, Color::Magenta, Color::Cyan, Color::LightRed, Color::LightGreen, Color::LightBlue, Color::LightMagenta],
    syntax: "base16-ocean.dark",
};

pub const ALL: [Palette; 3] = [DARK, LIGHT, TERMINAL];

static CURRENT: RwLock<Palette> = RwLock::new(DARK);

/// The palette in use.
pub fn p() -> Palette {
    *CURRENT.read().unwrap_or_else(|e| e.into_inner())
}

/// Use the theme called `name` (unknown names: dark).
pub fn set(name: &str) {
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = by_name(name);
}

pub fn by_name(name: &str) -> Palette {
    ALL.into_iter().find(|t| t.name == name).unwrap_or(DARK)
}

/// The theme after `name`, for cycling in the preferences.
pub fn next(name: &str) -> &'static str {
    let i = ALL.iter().position(|t| t.name == name).map_or(0, |i| (i + 1) % ALL.len());
    ALL[i].name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn themes_by_name() {
        assert_eq!(by_name("light").name, "light");
        assert_eq!(by_name("nope").name, "dark");
        assert_eq!((next("dark"), next("light"), next("terminal")), ("light", "terminal", "dark"));
        assert_eq!(DARK.accent, Color::Rgb(0x61, 0xaf, 0xef), "the dark theme keeps the original colours");
    }
}

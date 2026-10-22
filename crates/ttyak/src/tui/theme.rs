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
    /// Read conversations, threads and activity: a step back from the text, so unread ones
    /// stand out.
    pub read: Color,
    /// Online dots, values in the preferences.
    pub good: Color,
    /// Errors, delete questions.
    pub danger: Color,
    /// The bar along your own messages; huddles.
    pub own: Color,
    pub mention: Color,
    pub link: Color,
    /// Links into your own Slack, which ttyak opens itself.
    pub app_link: Color,
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
    read: rgb(0xa5a9b1),
    good: rgb(0x98c379),
    danger: rgb(0xe06c75),
    own: rgb(0x2bac76),
    mention: rgb(0x1d9bd1),
    link: rgb(0x5da9f6),
    app_link: rgb(0x3fc9a5),
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
    read: rgb(0x4f535a),
    good: rgb(0x2e7d32),
    danger: rgb(0xc62828),
    own: rgb(0x007a5a),
    mention: rgb(0x1264a3),
    link: rgb(0x0b4c8c),
    app_link: rgb(0x7b1fa2),
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
    read: Color::Reset,
    good: Color::Green,
    danger: Color::Red,
    own: Color::Green,
    mention: Color::Cyan,
    link: Color::Blue,
    app_link: Color::Green,
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

/// The look of a dialog's list row under the pointer (when it is not the selected one).
pub fn hover_style() -> ratatui::style::Style {
    ratatui::style::Style::default().bg(p().hover)
}

/// The palette in use.
pub fn p() -> Palette {
    *CURRENT.read().unwrap_or_else(|e| e.into_inner())
}

/// Use the theme called `name`: a built-in one or `themes/<name>.toml`. On failure the
/// current theme stays.
pub fn set(name: &str) -> Result<(), String> {
    let palette = palette(name)?;
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = palette;
    super::messages::looks_changed();
    Ok(())
}

/// The theme called `name`, without using it.
pub fn palette(name: &str) -> Result<Palette, String> {
    match ALL.into_iter().find(|t| t.name == name) {
        Some(p) => Ok(p),
        None => load(name),
    }
}

#[cfg(test)]
pub fn by_name(name: &str) -> Palette {
    ALL.into_iter().find(|t| t.name == name).unwrap_or(DARK)
}

/// Where custom themes live: `themes/` next to settings.toml.
pub fn dir() -> Option<std::path::PathBuf> {
    super::keymap::config_path().map(|p| p.with_file_name("themes"))
}

/// Built-in themes, then the custom ones (sorted).
pub fn names() -> Vec<String> {
    let mut custom: Vec<String> = dir()
        .and_then(|d| std::fs::read_dir(d).ok())
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok()?.path().file_name()?.to_str()?.strip_suffix(".toml").map(String::from))
        .filter(|n| !ALL.iter().any(|t| t.name == n))
        .collect();
    custom.sort();
    ALL.iter().map(|t| t.name.to_string()).chain(custom).collect()
}

fn load(name: &str) -> Result<Palette, String> {
    let path = dir().ok_or("no config directory")?.join(format!("{name}.toml"));
    let text = std::fs::read_to_string(&path).map_err(|e| format!("theme {name}: {e}"))?;
    parse(name, &text).map_err(|e| format!("theme {name}: {e}"))
}

/// A theme file: `base = "dark"` (or light/terminal) and any palette entries as colours
/// (`"#61afef"`, ANSI names such as `"light-red"`, or 0–255), `authors` as a list of ten,
/// `syntax` as a syntect theme name.
pub fn parse(name: &str, text: &str) -> Result<Palette, String> {
    let table: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
    let base = table.get("base").and_then(|b| b.as_str()).unwrap_or("dark");
    let mut p = ALL.into_iter().find(|t| t.name == base).ok_or_else(|| format!("unknown base theme '{base}'"))?;
    p.name = Box::leak(name.to_string().into_boxed_str());
    for (key, value) in &table {
        let color = |v: &toml::Value| -> Result<Color, String> {
            let s = v.as_str().map(String::from).or_else(|| v.as_integer().map(|i| i.to_string())).ok_or(format!("{key}: expected a colour"))?;
            s.parse::<Color>().map_err(|_| format!("{key}: '{s}' is not a colour"))
        };
        match key.as_str() {
            "base" => {}
            "syntax" => p.syntax = Box::leak(value.as_str().ok_or("syntax: expected a name")?.to_string().into_boxed_str()),
            "authors" => {
                let list = value.as_array().ok_or("authors: expected a list of colours")?;
                for (slot, v) in p.authors.iter_mut().zip(list) {
                    *slot = color(v)?;
                }
            }
            _ => *field(&mut p, key).ok_or_else(|| format!("unknown entry '{key}'"))? = color(value)?,
        }
    }
    Ok(p)
}

const FIELDS: [&str; 18] = ["accent", "selection", "hover", "dim", "strong", "read", "good", "danger", "own", "mention", "link", "app_link", "code_fg", "code_bg", "quote_bar", "badge_fg", "badge_bg", "pin"];

fn field<'a>(p: &'a mut Palette, key: &str) -> Option<&'a mut Color> {
    Some(match key {
        "accent" => &mut p.accent,
        "selection" => &mut p.selection,
        "hover" => &mut p.hover,
        "dim" => &mut p.dim,
        "strong" => &mut p.strong,
        "read" => &mut p.read,
        "good" => &mut p.good,
        "danger" => &mut p.danger,
        "own" => &mut p.own,
        "mention" => &mut p.mention,
        "link" => &mut p.link,
        "app_link" => &mut p.app_link,
        "code_fg" => &mut p.code_fg,
        "code_bg" => &mut p.code_bg,
        "quote_bar" => &mut p.quote_bar,
        "badge_fg" => &mut p.badge_fg,
        "badge_bg" => &mut p.badge_bg,
        "pin" => &mut p.pin,
        "call_bg" => &mut p.call_bg,
        _ => return None,
    })
}

fn color_text(c: Color) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("\"#{r:02x}{g:02x}{b:02x}\""),
        Color::Indexed(i) => i.to_string(),
        named => format!("\"{}\"", format!("{named:?}").to_lowercase()),
    }
}

/// A complete theme file for `base` (to copy and edit).
pub fn template(base: &str) -> Result<String, String> {
    let mut p = ALL.into_iter().find(|t| t.name == base).ok_or_else(|| format!("unknown base theme '{base}'"))?;
    let mut out = format!("# ttyak theme: save as themes/<name>.toml next to settings.toml; omitted entries come from the base.\nbase = \"{base}\"\nsyntax = \"{}\"\n", p.syntax);
    for key in FIELDS.iter().chain(&["call_bg"]) {
        let c = *field(&mut p, key).expect("known field");
        out += &format!("{key} = {}\n", color_text(c));
    }
    out += &format!("authors = [{}]\n", p.authors.iter().map(|c| color_text(*c)).collect::<Vec<_>>().join(", "));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn themes_by_name() {
        assert_eq!(by_name("light").name, "light");
        assert_eq!(by_name("nope").name, "dark");
        assert_eq!(DARK.accent, Color::Rgb(0x61, 0xaf, 0xef), "the dark theme keeps the original colours");
    }

    #[test]
    fn links_into_your_slack_have_their_own_colour() {
        let rgb3 = |c: Color| match c {
            Color::Rgb(r, g, b) => (r as i32, g as i32, b as i32),
            _ => (0, 0, 0),
        };
        let (r, g, b) = rgb3(DARK.app_link);
        assert!(g > r && g > b, "dark: a green-teal");
        let (r, g, b) = rgb3(LIGHT.app_link);
        assert!(r > g && b > g, "light: a purple");
        for t in ALL {
            assert_ne!(t.app_link, t.link, "{}", t.name);
            assert_ne!(t.app_link, t.accent, "{}", t.name);
        }
        assert_eq!(parse("x", "app_link = \"#102030\"").unwrap().app_link, Color::Rgb(0x10, 0x20, 0x30), "a theme file sets it");
        assert!(template("dark").unwrap().contains("app_link = \"#3fc9a5\""), "and the template lists it");
    }

    #[test]
    fn theme_files() {
        let p = parse("sunset", "base = \"light\"\naccent = \"#ff8800\"\nselection = \"light-blue\"\nhover = 236\nauthors = [\"red\", \"#010203\"]\n").unwrap();
        assert_eq!((p.name, p.accent, p.selection, p.hover), ("sunset", Color::Rgb(0xff, 0x88, 0), Color::LightBlue, Color::Indexed(236)));
        assert_eq!((p.authors[0], p.authors[1], p.authors[2]), (Color::Red, Color::Rgb(1, 2, 3), LIGHT.authors[2]), "unlisted authors from the base");
        assert_eq!(p.code_bg, LIGHT.code_bg, "omitted entries from the base");
        assert!(parse("x", "accent = \"not a colour\"").unwrap_err().contains("accent"));
        assert!(parse("x", "sparkle = \"red\"").unwrap_err().contains("unknown entry"));
        assert!(parse("x", "base = \"nope\"").is_err());
        // A template is a complete file that gives back its base.
        for base in ["dark", "light", "terminal"] {
            let again = parse(base, &template(base).unwrap()).unwrap();
            assert_eq!(again, by_name(base), "{base}");
        }
    }
}

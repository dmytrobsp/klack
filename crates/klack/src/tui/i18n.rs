//! Translation scaffolding: `tr("English text")` looks the text up in the locale chosen in
//! the preferences (`locales/<lang>.toml` next to settings.toml, mapping English text to the
//! translation) and falls back to the English. `ctl locale-template` prints every string
//! that goes through here, as a starting point for a locale file.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::RwLock;

static TABLE: RwLock<Option<HashMap<String, String>>> = RwLock::new(None);

/// `text` in the current language.
pub fn tr(text: &str) -> Cow<'_, str> {
    match TABLE.read().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|t| t.get(text)) {
        Some(t) => Cow::Owned(t.clone()),
        None => Cow::Borrowed(text),
    }
}

pub fn locale_path(lang: &str) -> Option<std::path::PathBuf> {
    super::keymap::config_path().map(|p| p.with_file_name("locales").join(format!("{lang}.toml")))
}

/// Parse a locale file: `"English text" = "translation"` lines.
pub fn parse(text: &str) -> Result<HashMap<String, String>, String> {
    let table: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
    Ok(table.into_iter().filter_map(|(k, v)| v.as_str().filter(|v| !v.is_empty()).map(|v| (k, v.to_string()))).collect())
}

/// Use language `lang` (`en` or empty: English). Returns how many strings were loaded.
pub fn set(lang: &str) -> Result<usize, String> {
    let lang = lang.trim();
    if lang.is_empty() || lang == "en" {
        *TABLE.write().unwrap_or_else(|e| e.into_inner()) = None;
        return Ok(0);
    }
    let path = locale_path(lang).ok_or("no config directory")?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let table = parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let n = table.len();
    *TABLE.write().unwrap_or_else(|e| e.into_inner()) = Some(table);
    Ok(n)
}

/// Every string shown through `tr`, for `ctl locale-template`.
pub fn translatable() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in super::keymap::Context::ALL {
        out.push(c.title().into());
    }
    out.extend(super::keymap::ACTIONS.iter().map(|a| a.description.to_string()));
    out.extend(super::keymap::hint_words().into_iter().map(String::from));
    out.extend(super::prefs::translatable());
    out.extend(super::help::COMMANDS.iter().map(|(_, what)| what.to_string()));
    out.extend(TITLES.iter().map(|t| t.to_string()));
    let mut seen = std::collections::HashSet::new();
    out.retain(|s| !s.is_empty() && seen.insert(s.clone()));
    out
}

/// Overlay titles that go through `tr`.
pub const TITLES: &[&str] = &["Preferences", "Keys", "Jump to…", "Activity", "Threads", "Profile", "Add reaction", "Search messages", "Commands (type in a composer)", "close", "rebind in keys.toml", "type to filter", "keep", "back"];

/// A locale file with every string untranslated (`"text" = ""`).
pub fn template() -> String {
    let mut out = String::from("# klack locale: translate the right-hand sides; empty ones stay English.\n");
    for s in translatable() {
        let key = s.replace('\\', "\\\\").replace('"', "\\\"");
        out += &format!("\"{key}\" = \"\"\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_files() {
        let t = parse("\"Preferences\" = \"Einstellungen\"\n\"quit\" = \"\"\n").unwrap();
        assert_eq!(t.get("Preferences").map(String::as_str), Some("Einstellungen"));
        assert!(!t.contains_key("quit"), "empty translations stay English");
        assert!(parse("not toml =").is_err());
        let template = template();
        assert!(template.contains("\"Preferences\" = \"\"") && template.contains("\"add a reaction\" = \"\""));
        assert!(parse(&template).unwrap().is_empty(), "the template parses and translates nothing");
    }
}

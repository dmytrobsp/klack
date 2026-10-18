//! Local preferences in `settings.toml` (next to the profiles), edited in the preferences
//! menu. Slack-side preferences (notification levels, keywords) live on Slack instead.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub appearance: Appearance,
    pub messages: Messages,
    pub region: Region,
    pub notifications: Notifications,
    pub audio: Audio,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    /// No blank rows between authors.
    pub compact: bool,
    /// Colour theme: dark, light or terminal (the terminal's own colours).
    pub theme: String,
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance { compact: false, theme: "dark".into() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Messages {
    /// Image thumbnails in the message list.
    pub inline_images: bool,
    /// Profile pictures next to names.
    pub avatars: bool,
    /// Show emoji as `:name:` instead of the character.
    pub emoji_as_names: bool,
}

impl Default for Messages {
    fn default() -> Self {
        Messages { inline_images: true, avatars: true, emoji_as_names: false }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Region {
    /// 24-hour clock (else 12-hour with am/pm).
    pub clock_24h: bool,
    /// Day separators, as a strftime pattern.
    pub date_format: String,
    /// IANA time zone for shown times (e.g. `Europe/Kyiv`); empty: the system's.
    pub timezone: String,
    /// Interface language: `en`, or the code of a file in `locales/`.
    pub language: String,
}

impl Default for Region {
    fn default() -> Self {
        Region { clock_24h: true, date_format: "%A, %B %-d".into(), timezone: String::new(), language: "en".into() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Notifications {
    /// Desktop notifications at all.
    pub desktop: bool,
}

impl Default for Notifications {
    fn default() -> Self {
        Notifications { desktop: true }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Audio {
    /// Microphone and speaker (part of a device id or name); `SLACKTUI_AUDIO_INPUT` /
    /// `SLACKTUI_AUDIO_OUTPUT` win over these.
    pub input: Option<String>,
    pub output: Option<String>,
}

impl Settings {
    /// strftime pattern for times of day.
    pub fn time_format(&self) -> &'static str {
        if self.region.clock_24h { "%H:%M" } else { "%-I:%M %p" }
    }

    /// The device to use for `env` (the environment variable wins over the setting).
    pub fn device(env: &str, setting: &Option<String>) -> Option<String> {
        std::env::var(env).ok().filter(|v| !v.is_empty()).or_else(|| setting.clone().filter(|v| !v.is_empty()))
    }

    pub fn parse(text: &str) -> Result<Settings, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }
}

pub fn config_path() -> Option<std::path::PathBuf> {
    super::keymap::config_path().map(|p| p.with_file_name("settings.toml"))
}

/// The saved settings, or the defaults and why the file could not be used.
pub fn load() -> (Settings, Option<String>) {
    let Some(path) = config_path() else { return (Settings::default(), None) };
    match std::fs::read_to_string(&path) {
        Ok(text) => match Settings::parse(&text) {
            Ok(s) => (s, None),
            Err(e) => (Settings::default(), Some(format!("settings.toml: {e}"))),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Settings::default(), None),
        Err(e) => (Settings::default(), Some(format!("cannot read settings.toml: {e}"))),
    }
}

pub fn save(settings: &Settings) -> Result<std::path::PathBuf, String> {
    let path = config_path().ok_or("no config directory")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, settings.to_toml()).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_files_keep_the_other_defaults() {
        let s = Settings::parse("[region]\nclock_24h = false\n[messages]\navatars = false\n").unwrap();
        assert!(!s.region.clock_24h && !s.messages.avatars);
        assert!(s.messages.inline_images && s.notifications.desktop && s.region.date_format == "%A, %B %-d");
        assert_eq!(s.time_format(), "%-I:%M %p");
        assert_eq!(Settings::parse("").unwrap(), Settings::default());
        assert!(Settings::parse("[region]\nclock_24h = \"yes\"").is_err());
    }

    #[test]
    fn round_trip() {
        let mut s = Settings::default();
        s.appearance.compact = true;
        s.audio.output = Some("USB".into());
        assert_eq!(Settings::parse(&s.to_toml()).unwrap(), s);
    }
}

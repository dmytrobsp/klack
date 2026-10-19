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
    pub video: Video,
    pub huddle: Huddle,
}

/// Sounds for huddle events: a name from `sound::HUDDLE_SOUNDS`, "off" for none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Huddle {
    /// When you join (once the huddle is connected).
    pub join_sound: String,
    /// While you are in it: when someone else joins, and when someone leaves.
    pub someone_joined: String,
    pub someone_left: String,
    /// A huddle invitation rings with this (from `sound::RING_SOUNDS`) until you answer,
    /// for at most `ring_timeout_secs`; then, with `auto_decline`, it is declined on Slack.
    pub ring_sound: String,
    pub ring_timeout_secs: u32,
    pub auto_decline: bool,
    /// When you have been the only one in a huddle for `alone_secs`, a quiet melody (from
    /// `sound::ALONE_SOUNDS`) loops until someone joins or you leave.
    pub alone_sound: String,
    pub alone_secs: u32,
}

impl Default for Huddle {
    fn default() -> Self {
        Huddle { join_sound: "rise".into(), someone_joined: "pop".into(), someone_left: "fall".into(), ring_sound: "ring".into(), ring_timeout_secs: 30, auto_decline: false, alone_sound: "lullaby".into(), alone_secs: 300 }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Video {
    /// The camera for huddles (part of its name or path); none chosen: the camera stays off.
    pub camera: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    /// No blank rows between authors.
    pub compact: bool,
    /// Colour theme: dark, light or terminal (the terminal's own colours).
    pub theme: String,
    /// Channels and DMs without a message for this many days leave the sidebar (starred, unread
    /// and open ones stay; Ctrl+K still finds them); 0 keeps them all.
    pub hide_inactive_days: u32,
    /// A theme of its own for some workspaces (team id → theme), to tell them apart.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub workspace_themes: std::collections::BTreeMap<String, String>,
}

impl Appearance {
    /// The theme for the workspace `team`.
    pub fn theme_for(&self, team: &str) -> &str {
        self.workspace_themes.get(team).unwrap_or(&self.theme)
    }
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance { compact: false, theme: "dark".into(), hide_inactive_days: 30, workspace_themes: Default::default() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Messages {
    /// Image thumbnails in the message list.
    pub inline_images: bool,
    /// Profile pictures next to names.
    pub avatars: bool,
    /// Profile pictures two rows tall (beside the name and the first line), the text indented.
    pub large_avatars: bool,
    /// A previewed channel (one you are not in gets no live events) is fetched again this
    /// often, in seconds; 0 turns it off.
    pub preview_refresh_secs: u32,
    /// Show emoji as `:name:` instead of the character.
    pub emoji_as_names: bool,
    /// Draw the workspace's own (picture) emoji as pictures in messages and reactions, where
    /// the terminal draws pictures; else they show as `:name:`.
    pub custom_emoji_pictures: bool,
    /// How pictures are drawn: "auto" (asked from the terminal) or one of
    /// [`super::media::GRAPHICS`]; `KLACK_GRAPHICS` wins over it.
    pub graphics: String,
}

impl Default for Messages {
    fn default() -> Self {
        Messages { inline_images: true, avatars: true, large_avatars: false, preview_refresh_secs: 15, emoji_as_names: false, custom_emoji_pictures: true, graphics: "auto".into() }
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
    /// Sound with each notification: off, chime, ping, knock, or system (the desktop's).
    pub sound: String,
}

impl Default for Notifications {
    fn default() -> Self {
        Notifications { desktop: true, sound: "off".into() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Audio {
    /// Microphone and speaker (part of a device id or name); `KLACK_AUDIO_INPUT` /
    /// `KLACK_AUDIO_OUTPUT` win over these.
    pub input: Option<String>,
    pub output: Option<String>,
    /// Huddle playback volume, in percent.
    pub huddle_volume: u32,
    /// Notification sound volume, in percent.
    pub sound_volume: u32,
    /// Microphone gain, in percent.
    pub mic_gain: u32,
    /// The microphone sends silence while quieter than this (dBFS); unset: always on.
    pub speaking_threshold_db: Option<i32>,
    /// RNNoise noise suppression on the microphone.
    pub noise_suppression: bool,
    /// Remove what the speaker plays from the microphone (AEC3).
    pub echo_cancellation: bool,
}

impl Default for Audio {
    fn default() -> Self {
        Audio { input: None, output: None, huddle_volume: 100, sound_volume: 100, mic_gain: 100, speaking_threshold_db: None, noise_suppression: false, echo_cancellation: true }
    }
}

impl Audio {
    /// How the microphone is processed in a huddle.
    pub fn mic_config(&self) -> huddle::devices::MicConfig {
        huddle::devices::MicConfig { gain: self.mic_gain as f32 / 100.0, gate_db: self.speaking_threshold_db.map(|d| d as f32), suppress_noise: self.noise_suppression, cancel_echo: self.echo_cancellation }
    }
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
        s.audio.speaking_threshold_db = Some(-45);
        assert_eq!(Settings::parse(&s.to_toml()).unwrap(), s);
    }

    #[test]
    fn audio_defaults_and_mic_config() {
        let s = Settings::parse("[audio]\nmic_gain = 150\n").unwrap();
        assert_eq!((s.audio.huddle_volume, s.audio.sound_volume, s.audio.speaking_threshold_db, s.audio.noise_suppression), (100, 100, None, false));
        let cfg = s.audio.mic_config();
        assert_eq!((cfg.gain, cfg.gate_db, cfg.suppress_noise, cfg.cancel_echo), (1.5, None, false, true), "echo cancellation is on by default");
        let s = Settings::parse("[audio]\nspeaking_threshold_db = -40\nnoise_suppression = true\necho_cancellation = false\n").unwrap();
        assert_eq!(s.audio.mic_config(), huddle::devices::MicConfig { gain: 1.0, gate_db: Some(-40.0), suppress_noise: true, cancel_echo: false });
    }
}

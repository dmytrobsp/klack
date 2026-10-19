//! Preferences (F6): local settings by category; a change applies at once and is saved
//! to settings.toml.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::keymap::{ACTIONS, Keymap};
use super::settings::Settings;
use super::state::{LEVELS, Workspace, level_label};

/// Day-separator formats offered (strftime).
pub const DATE_FORMATS: &[&str] = &["%A, %B %-d", "%a %-d %b", "%d.%m.%Y", "%Y-%m-%d", "%m/%d/%Y"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Theme,
    WorkspaceTheme,
    /// The leader key for chords.
    Leader,
    /// An action's keys (index into `keymap::ACTIONS`).
    Binding(usize),
    Compact,
    InlineImages,
    Avatars,
    LargeAvatars,
    PreviewRefresh,
    Graphics,
    EmojiAsNames,
    Clock24,
    DateFormat,
    TimeZone,
    Language,
    Desktop,
    Sound,
    /// Slack: the level for the conversation on screen.
    ConversationLevel,
    /// Slack: the default level for channels.
    ChannelDefault,
    /// Slack: words that notify like a mention.
    Keywords,
    /// Slack: your VIPs (changed with v on a profile card).
    Vips,
    /// Slack: your status (a text field: `:emoji: text`).
    Status,
    /// Slack: appear away.
    Away,
    /// Slack: pause notifications.
    Pause,
    Input,
    Output,
    HuddleVolume,
    SoundVolume,
    MicGain,
    SpeakingThreshold,
    NoiseSuppression,
    EchoCancellation,
    /// The camera for huddles (one of the cameras found; never a default).
    Camera,
    JoinSound,
    SomeoneJoined,
    SomeoneLeft,
    RingSound,
    RingTimeout,
    AutoDecline,
    AloneSound,
    AloneSecs,
}

/// How long you may be alone in a huddle before the reminder, in seconds.
const ALONE_SECS: &[u32] = &[30, 60, 120, 180, 300, 600, 900];

/// The alone reminder plays at this share (percent) of the notification sound volume.
pub const ALONE_VOLUME: u32 = 40;

/// How long an invitation may ring, in seconds.
const RING_TIMEOUTS: &[u32] = &[15, 30, 45, 60, 90, 120];
/// Seconds between fetches of a previewed channel (0: off).
const PREVIEW_REFRESH: &[u32] = &[0, 5, 10, 15, 30, 60, 120];

/// Pause lengths offered, in minutes (0: not paused).
pub const PAUSES: &[u64] = &[0, 30, 60, 120, 480];

pub const CATEGORIES: &[(&str, &[Field])] = &[
    ("Availability", &[Field::Status, Field::Away, Field::Pause]),
    ("Appearance", &[Field::Theme, Field::WorkspaceTheme, Field::Compact]),
    ("Messages & media", &[Field::InlineImages, Field::Avatars, Field::LargeAvatars, Field::PreviewRefresh, Field::Graphics, Field::EmojiAsNames]),
    ("Language & region", &[Field::Clock24, Field::DateFormat, Field::TimeZone, Field::Language]),
    ("Notifications", &[Field::Desktop, Field::Sound, Field::ConversationLevel, Field::ChannelDefault, Field::Keywords, Field::Vips]),
    ("Audio & video", &[Field::Input, Field::Output, Field::HuddleVolume, Field::SoundVolume, Field::MicGain, Field::SpeakingThreshold, Field::NoiseSuppression, Field::EchoCancellation, Field::Camera]),
    ("Huddles", &[Field::JoinSound, Field::SomeoneJoined, Field::SomeoneLeft, Field::RingSound, Field::RingTimeout, Field::AutoDecline, Field::AloneSound, Field::AloneSecs]),
];

/// Volumes offered, in percent.
pub const VOLUMES: &[u32] = &[0, 10, 25, 50, 75, 100, 125, 150, 200];
/// Microphone gains offered, in percent.
pub const GAINS: &[u32] = &[25, 50, 75, 100, 125, 150, 200, 300, 400];
/// Speaking thresholds offered, in dBFS (None: off).
pub const THRESHOLDS: &[Option<i32>] = &[None, Some(-60), Some(-55), Some(-50), Some(-45), Some(-40), Some(-35), Some(-30), Some(-25), Some(-20)];

/// The next value up (or down) in `list` from `current`, which need not be in it; at the
/// end it wraps to the start if `wrap`, else stays.
fn step<T: PartialOrd + Copy>(list: &[T], current: T, up: bool, wrap: bool) -> T {
    let found = if up { list.iter().find(|v| **v > current) } else { list.iter().rev().find(|v| **v < current) };
    match found {
        Some(v) => *v,
        None if wrap && up => list[0],
        None => current,
    }
}

impl Field {
    pub fn label(self) -> String {
        match self {
            Field::Leader => super::i18n::tr(LEADER_LABEL).into_owned(),
            Field::Binding(i) => format!("{} · {}", super::i18n::tr(ACTIONS[i].context.title()), super::i18n::tr(ACTIONS[i].description)),
            other => super::i18n::tr(other.fixed_label()).into_owned(),
        }
    }

    fn fixed_label(self) -> &'static str {
        match self {
            Field::Leader | Field::Binding(_) => "",
            Field::Theme => "Colour theme",
            Field::WorkspaceTheme => "Theme for this workspace",
            Field::Compact => "Compact (no gaps between authors or sidebar sections)",
            Field::InlineImages => "Show image thumbnails",
            Field::Avatars => "Show profile pictures",
            Field::LargeAvatars => "Two-row profile pictures",
            Field::PreviewRefresh => "Refresh previewed channels",
            Field::Graphics => "Draw pictures with",
            Field::EmojiAsNames => "Show emoji as :names:",
            Field::Clock24 => "24-hour clock",
            Field::DateFormat => "Date format",
            Field::TimeZone => "Time zone",
            Field::Language => "Language (locales/<code>.toml)",
            Field::Desktop => "Desktop notifications",
            Field::Sound => "Sound",
            Field::ConversationLevel => "This conversation notifies about",
            Field::ChannelDefault => "Channels notify about (default)",
            Field::Keywords => "Keywords",
            Field::Vips => "VIPs (v on a profile card)",
            Field::Status => "Status",
            Field::Away => "Appear away",
            Field::Pause => "Pause notifications",
            Field::Input => "Microphone",
            Field::Output => "Speaker",
            Field::HuddleVolume => "Huddle volume",
            Field::SoundVolume => "Notification sound volume",
            Field::MicGain => "Microphone gain",
            Field::SpeakingThreshold => "Speaking threshold",
            Field::NoiseSuppression => "Noise suppression",
            Field::EchoCancellation => "Echo cancellation",
            Field::Camera => "Camera (F9 in a huddle)",
            Field::JoinSound => "Sound when you join",
            Field::SomeoneJoined => "Sound when someone joins",
            Field::SomeoneLeft => "Sound when someone leaves",
            Field::RingSound => "An invitation rings with",
            Field::RingTimeout => "Stop ringing after",
            Field::AutoDecline => "Then decline it on Slack",
            Field::AloneSound => "Alone in a huddle, loop",
            Field::AloneSecs => "Start it after",
        }
    }

    pub fn value(self, ws: &Workspace, current: Option<&str>, keys: &Keymap) -> String {
        let s = &ws.settings;
        match self {
            Field::Leader => return keys.leader.map(|k| k.label()).unwrap_or_else(|| "none".into()),
            Field::Binding(i) => {
                let label = keys.keys_label(ACTIONS[i].context, ACTIONS[i].name);
                return if label.is_empty() { "(unbound)".into() } else { label };
            }
            _ => {}
        }
        let yes = |b: bool| if b { "on" } else { "off" }.to_string();
        let device = |d: &Option<String>| d.clone().unwrap_or_else(|| "system default".into());
        match self {
            Field::Leader | Field::Binding(_) => String::new(),
            Field::Theme => s.appearance.theme.clone(),
            Field::WorkspaceTheme => s.appearance.workspace_themes.get(&ws.team_id).cloned().unwrap_or_else(|| "(colour theme)".into()),
            Field::Compact => yes(s.appearance.compact),
            Field::InlineImages => yes(s.messages.inline_images),
            Field::Avatars => yes(s.messages.avatars),
            Field::LargeAvatars => yes(s.messages.large_avatars),
            Field::PreviewRefresh => match s.messages.preview_refresh_secs {
                0 => "off".into(),
                n => format!("every {n} s"),
            },
            Field::Graphics => s.messages.graphics.clone(),
            Field::EmojiAsNames => yes(s.messages.emoji_as_names),
            Field::Clock24 => yes(s.region.clock_24h),
            Field::DateFormat => super::clock::now().format(&s.region.date_format).to_string(),
            Field::Language if s.region.language.is_empty() || s.region.language == "en" => "en (built in)".into(),
            Field::Language => s.region.language.clone(),
            Field::TimeZone if s.region.timezone.is_empty() => format!("system ({})", super::clock::now().format("%:z")),
            Field::TimeZone => format!("{} ({})", s.region.timezone, super::clock::now().format("%H:%M")),
            Field::Desktop => yes(s.notifications.desktop),
            Field::Sound => s.notifications.sound.clone(),
            Field::ConversationLevel => match current {
                Some(c) => {
                    let mpim = ws.conversations.get(c).is_some_and(|c| c.is_mpim);
                    let own = ws.notify.channels.contains_key(c);
                    format!("{}{}", level_label(ws.notify.level(c, mpim)), if own { "" } else { " (default)" })
                }
                None => "no conversation open".into(),
            },
            Field::ChannelDefault => level_label(&ws.notify.global).into(),
            Field::Keywords if ws.notify.keywords.is_empty() => "none".into(),
            Field::Keywords => ws.notify.keywords.join(", "),
            Field::Status => {
                let p = ws.users.get(&ws.self_id).map(|u| &u.profile);
                match p.map(|p| (p.status_emoji.trim_matches(':'), p.status_text.as_str())) {
                    Some((e, t)) if !e.is_empty() || !t.is_empty() => format!("{} {t}", if e.is_empty() { String::new() } else { super::text::emoji(ws, e, None) }).trim().to_string(),
                    _ => "none".into(),
                }
            }
            Field::Away => yes(ws.presence.get(&ws.self_id) == Some(&false)),
            Field::Pause => {
                let now = super::clock::now().timestamp() as f64;
                match ws.dnd.snooze_until.filter(|t| *t > now) {
                    Some(t) => super::clock::at(t as i64).map(|d| format!("until {}", d.format(s.time_format()))).unwrap_or_default(),
                    None => "no".into(),
                }
            }
            Field::Vips if ws.vip.is_empty() => "none".into(),
            Field::Vips => {
                let mut names: Vec<String> = ws.vip.iter().map(|u| ws.user_name(u)).collect();
                names.sort();
                names.join(", ")
            }
            Field::Input => device(&s.audio.input),
            Field::Output => device(&s.audio.output),
            Field::HuddleVolume => format!("{}%", s.audio.huddle_volume),
            Field::SoundVolume => format!("{}%", s.audio.sound_volume),
            Field::MicGain => format!("{}%", s.audio.mic_gain),
            Field::SpeakingThreshold => s.audio.speaking_threshold_db.map_or("off".into(), |d| format!("{d} dB")),
            Field::NoiseSuppression => yes(s.audio.noise_suppression),
            Field::EchoCancellation => yes(s.audio.echo_cancellation),
            Field::Camera => s.video.camera.clone().unwrap_or_else(|| "none (the camera stays off)".into()),
            Field::JoinSound => s.huddle.join_sound.clone(),
            Field::SomeoneJoined => s.huddle.someone_joined.clone(),
            Field::SomeoneLeft => s.huddle.someone_left.clone(),
            Field::RingSound => s.huddle.ring_sound.clone(),
            Field::RingTimeout => format!("{} s", s.huddle.ring_timeout_secs),
            Field::AutoDecline => yes(s.huddle.auto_decline),
            Field::AloneSound => s.huddle.alone_sound.clone(),
            Field::AloneSecs => format!("{} s alone", s.huddle.alone_secs),
        }
    }

    /// Raise or lower a value with steps (volumes, gain, threshold); false for other fields.
    /// `wrap`: past the top, start again at the bottom (Enter cycles that way).
    pub fn adjust(self, s: &mut Settings, up: bool, wrap: bool) -> bool {
        let a = &mut s.audio;
        match self {
            Field::HuddleVolume => a.huddle_volume = step(VOLUMES, a.huddle_volume, up, wrap),
            Field::SoundVolume => a.sound_volume = step(VOLUMES, a.sound_volume, up, wrap),
            Field::RingTimeout => s.huddle.ring_timeout_secs = step(RING_TIMEOUTS, s.huddle.ring_timeout_secs, up, wrap),
            Field::AloneSecs => s.huddle.alone_secs = step(ALONE_SECS, s.huddle.alone_secs, up, wrap),
            Field::PreviewRefresh => s.messages.preview_refresh_secs = step(PREVIEW_REFRESH, s.messages.preview_refresh_secs, up, wrap),
            Field::MicGain => a.mic_gain = step(GAINS, a.mic_gain, up, wrap),
            Field::SpeakingThreshold => a.speaking_threshold_db = step(THRESHOLDS, a.speaking_threshold_db, up, wrap),
            _ => return false,
        }
        true
    }

    /// Toggle, or step to the next choice; `devices` are (inputs, outputs) as device ids.
    pub fn change(self, s: &mut Settings, devices: &(Vec<String>, Vec<String>)) {
        let next = |current: &Option<String>, list: &[String]| -> Option<String> {
            // None (system default), then each device, then back to None.
            let pos = current.as_ref().and_then(|c| list.iter().position(|d| d == c));
            match pos {
                None if current.is_none() => list.first().cloned(),
                None => None,
                Some(i) => list.get(i + 1).cloned(),
            }
        };
        match self {
            // Needs the workspace: in `Prefs::change`.
            Field::TimeZone | Field::Language | Field::WorkspaceTheme => {}
            // Chosen in a picker (`Prefs::change`).
            Field::Theme => {}
            Field::Compact => s.appearance.compact ^= true,
            Field::InlineImages => s.messages.inline_images ^= true,
            Field::Avatars => s.messages.avatars ^= true,
            Field::LargeAvatars => s.messages.large_avatars ^= true,
            Field::PreviewRefresh => {
                self.adjust(s, true, true);
            }
            Field::Graphics => {
                let i = super::media::GRAPHICS.iter().position(|g| *g == s.messages.graphics).map_or(0, |i| (i + 1) % super::media::GRAPHICS.len());
                s.messages.graphics = super::media::GRAPHICS[i].into();
            }
            Field::EmojiAsNames => s.messages.emoji_as_names ^= true,
            Field::Clock24 => s.region.clock_24h ^= true,
            Field::DateFormat => {
                let i = DATE_FORMATS.iter().position(|f| *f == s.region.date_format).map_or(0, |i| (i + 1) % DATE_FORMATS.len());
                s.region.date_format = DATE_FORMATS[i].into();
            }
            Field::Desktop => s.notifications.desktop ^= true,
            Field::JoinSound | Field::SomeoneJoined | Field::SomeoneLeft => {
                let h = &mut s.huddle;
                let slot = match self {
                    Field::JoinSound => &mut h.join_sound,
                    Field::SomeoneJoined => &mut h.someone_joined,
                    _ => &mut h.someone_left,
                };
                *slot = next_sound(slot);
                let name = slot.clone();
                super::sound::play(&name, Settings::device(huddle::devices::OUTPUT_ENV, &s.audio.output), s.audio.sound_volume);
            }
            Field::RingSound => {
                let sounds = super::sound::RING_SOUNDS;
                s.huddle.ring_sound = sounds[sounds.iter().position(|x| *x == s.huddle.ring_sound).map_or(0, |i| (i + 1) % sounds.len())].into();
                super::sound::play(&s.huddle.ring_sound, Settings::device(huddle::devices::OUTPUT_ENV, &s.audio.output), s.audio.sound_volume);
            }
            Field::RingTimeout => {
                self.adjust(s, true, true);
            }
            Field::AutoDecline => s.huddle.auto_decline ^= true,
            Field::AloneSound => {
                let sounds = super::sound::ALONE_SOUNDS;
                s.huddle.alone_sound = sounds[sounds.iter().position(|x| *x == s.huddle.alone_sound).map_or(0, |i| (i + 1) % sounds.len())].into();
                super::sound::play(&s.huddle.alone_sound, Settings::device(huddle::devices::OUTPUT_ENV, &s.audio.output), s.audio.sound_volume * ALONE_VOLUME / 100);
            }
            Field::AloneSecs => {
                self.adjust(s, true, true);
            }
            Field::Sound => {
                let sounds = super::sound::SOUNDS;
                let i = sounds.iter().position(|x| *x == s.notifications.sound).map_or(0, |i| (i + 1) % sounds.len());
                s.notifications.sound = sounds[i].into();
                // Let the user hear it.
                super::sound::play(sounds[i], Settings::device(huddle::devices::OUTPUT_ENV, &s.audio.output), s.audio.sound_volume);
            }
            // Slack-side: see `Prefs::change`.
            Field::ConversationLevel | Field::ChannelDefault | Field::Keywords | Field::Vips | Field::Status | Field::Away | Field::Pause | Field::Leader | Field::Binding(_) => {}
            Field::Input => s.audio.input = next(&s.audio.input, &devices.0),
            Field::Output => s.audio.output = next(&s.audio.output, &devices.1),
            Field::NoiseSuppression => s.audio.noise_suppression ^= true,
            Field::EchoCancellation => s.audio.echo_cancellation ^= true,
            // Needs the cameras found: see `Prefs::change`.
            Field::Camera => {}
            Field::HuddleVolume | Field::SoundVolume | Field::MicGain | Field::SpeakingThreshold => {
                self.adjust(s, true, true);
            }
        }
    }
}

/// A change to make on Slack (applied locally at once by the caller).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlackChange {
    ConversationLevel { channel: String, level: String },
    ChannelDefault(String),
    Keywords(String),
    /// Run this slash command (status, away, pause).
    Command(String),
}

/// A conversation's own level cycles through the three and back to "default" (no override).
fn next_conversation_level(own: Option<&str>) -> String {
    match own {
        None => LEVELS[0].to_string(),
        Some(l) if l == LEVELS[LEVELS.len() - 1] => "default".to_string(),
        Some(l) => next_level(l),
    }
}

/// The huddle sound after `current` (then back to "off").
fn next_sound(current: &str) -> String {
    let sounds = super::sound::HUDDLE_SOUNDS;
    sounds[sounds.iter().position(|x| *x == current).map_or(0, |i| (i + 1) % sounds.len())].to_string()
}

fn next_level(level: &str) -> String {
    let i = LEVELS.iter().position(|l| *l == level).map_or(0, |i| (i + 1) % LEVELS.len());
    LEVELS[i].to_string()
}

const LEADER_LABEL: &str = "Leader key (for chords such as Leader k)";

/// Category names and fixed labels, for translation templates.
pub fn translatable() -> Vec<String> {
    let all = [
        Field::Theme, Field::WorkspaceTheme, Field::Compact, Field::InlineImages, Field::Avatars, Field::LargeAvatars, Field::PreviewRefresh, Field::Graphics, Field::EmojiAsNames, Field::Clock24, Field::DateFormat, Field::TimeZone, Field::Language, Field::Desktop, Field::Sound,
        Field::ConversationLevel, Field::ChannelDefault, Field::Keywords, Field::Vips, Field::Status, Field::Away, Field::Pause, Field::Input, Field::Output,
        Field::HuddleVolume, Field::SoundVolume, Field::MicGain, Field::SpeakingThreshold, Field::NoiseSuppression, Field::EchoCancellation, Field::Camera, Field::JoinSound, Field::SomeoneJoined, Field::SomeoneLeft, Field::RingSound, Field::RingTimeout, Field::AutoDecline, Field::AloneSound, Field::AloneSecs,
    ];
    CATEGORIES.iter().map(|c| c.0).chain(["Keys", LEADER_LABEL]).chain(all.iter().map(|f| f.fixed_label())).map(String::from).collect()
}

#[derive(Debug, Default)]
pub struct Prefs {
    /// Text being typed into a field (keywords).
    pub editing: Option<String>,
    /// Waiting for the key(s) to bind to the selected Keys entry.
    pub capturing: Option<Vec<super::keymap::Key>>,
    /// First row shown in the settings list.
    scroll: usize,
    pub category: usize,
    pub selected: usize,
    /// Audio devices as ids (listed when the menu opens; only names, nothing is opened).
    pub devices: (Vec<String>, Vec<String>),
    /// Camera names (listed when the menu opens; nothing is opened).
    pub cameras: Vec<String>,
    /// Where the settings were saved, or why not.
    pub saved: Option<Result<String, String>>,
    /// Choosing a theme (the colour theme, or the workspace's own).
    pub picker: Option<super::theme_picker::ThemePicker>,
}

impl Prefs {
    pub fn new() -> Prefs {
        let id = |d: &String| d.split(" — ").next().unwrap_or(d).to_string();
        let devices = huddle::devices::list_devices().map(|(i, o)| (i.iter().map(id).collect(), o.iter().map(id).collect())).unwrap_or_default();
        let cameras = huddle::camera::list_cameras().into_iter().map(|c| c.name).collect();
        Prefs { devices, cameras, ..Default::default() }
    }

    pub fn fields(&self) -> Vec<Field> {
        match CATEGORIES.get(self.category) {
            Some((_, fields)) => fields.to_vec(),
            // The last category: key bindings.
            None => std::iter::once(Field::Leader).chain((0..ACTIONS.len()).map(Field::Binding)).collect(),
        }
    }

    pub fn category_name(&self) -> &'static str {
        CATEGORIES.get(self.category).map_or("Keys", |c| c.0)
    }

    pub fn switch_category(&mut self, delta: isize) {
        self.category = (self.category as isize + delta).rem_euclid(CATEGORIES.len() as isize + 1) as usize;
        self.selected = 0;
        self.scroll = 0;
    }

    pub fn selected_field(&self) -> Option<Field> {
        self.fields().get(self.selected).copied()
    }

    pub fn move_by(&mut self, delta: isize) {
        self.selected = (self.selected as isize + delta).clamp(0, self.fields().len() as isize - 1) as usize;
    }

    /// Change the selected setting: local ones in `ws.settings` (the caller saves them),
    /// Slack-side ones returned for the caller to send. A text field starts editing first.
    pub fn change(&mut self, ws: &mut Workspace, current: Option<&str>) -> Option<SlackChange> {
        let field = self.selected_field()?;
        match field {
            Field::Leader | Field::Binding(_) => {
                self.capturing = Some(Vec::new());
                None
            }
            Field::Language => {
                match self.editing.take() {
                    Some(text) => match super::i18n::set(&text) {
                        Ok(_) => ws.settings.region.language = text.trim().to_string(),
                        Err(e) => self.saved = Some(Err(e)),
                    },
                    None => self.editing = Some(ws.settings.region.language.clone()),
                }
                None
            }
            // A text field: Enter starts editing, Enter again applies (the caller saves).
            // The picker shows each theme as you go; the caller keeps the chosen one.
            Field::Theme => {
                self.picker = Some(super::theme_picker::ThemePicker::new(false, super::theme::names(), Some(&ws.settings.appearance.theme)));
                None
            }
            Field::WorkspaceTheme => {
                let own = ws.settings.appearance.workspace_themes.get(&ws.team_id).map(String::as_str);
                self.picker = Some(super::theme_picker::ThemePicker::new(true, super::theme::names(), own));
                None
            }
            Field::Camera => {
                // None, then each camera found, then None again.
                let c = &mut ws.settings.video.camera;
                let pos = c.as_ref().and_then(|c| self.cameras.iter().position(|n| n == c));
                *c = match pos {
                    None if c.is_none() => self.cameras.first().cloned(),
                    None => None,
                    Some(i) => self.cameras.get(i + 1).cloned(),
                };
                if self.cameras.is_empty() {
                    self.saved = Some(Err("no camera found".into()));
                }
                None
            }
            Field::TimeZone => {
                match self.editing.take() {
                    Some(text) => match super::clock::set(&text) {
                        Ok(()) => ws.settings.region.timezone = text.trim().to_string(),
                        Err(e) => self.saved = Some(Err(e)),
                    },
                    None => self.editing = Some(ws.settings.region.timezone.clone()),
                }
                None
            }
            Field::ConversationLevel => {
                let channel = current?.to_string();
                let level = next_conversation_level(ws.notify.channels.get(&channel).map(String::as_str));
                Some(SlackChange::ConversationLevel { channel, level })
            }
            Field::ChannelDefault => Some(SlackChange::ChannelDefault(next_level(&ws.notify.global))),
            Field::Vips => None,
            Field::Status => match self.editing.take() {
                Some(text) if text.trim().is_empty() => Some(SlackChange::Command("/status clear".into())),
                Some(text) => Some(SlackChange::Command(format!("/status {}", text.trim()))),
                None => {
                    let p = ws.users.get(&ws.self_id).map(|u| &u.profile);
                    self.editing = Some(p.map(|p| format!("{} {}", p.status_emoji, p.status_text).trim().to_string()).unwrap_or_default());
                    None
                }
            },
            Field::Away => Some(SlackChange::Command(if ws.presence.get(&ws.self_id) == Some(&false) { "/active" } else { "/away" }.into())),
            Field::Pause => {
                // Step from the pause in effect (rounded to an offered length) to the next.
                let now = super::clock::now().timestamp() as f64;
                let left = ws.dnd.snooze_until.filter(|t| *t > now).map_or(0, |t| ((t - now) / 60.0).round() as u64);
                let i = PAUSES.iter().position(|m| *m >= left && (*m == 0) == (left == 0)).unwrap_or(0);
                Some(SlackChange::Command(match PAUSES[(i + 1) % PAUSES.len()] {
                    0 => "/dnd off".into(),
                    m => format!("/dnd {m}m"),
                }))
            }
            Field::Keywords => match self.editing.take() {
                Some(text) => Some(SlackChange::Keywords(text.split(',').map(str::trim).filter(|k| !k.is_empty()).collect::<Vec<_>>().join(","))),
                None => {
                    self.editing = Some(ws.notify.keywords.join(", "));
                    None
                }
            },
            f => {
                f.change(&mut ws.settings, &self.devices);
                None
            }
        }
    }

    /// Whether the selected field is kept on Slack (nothing to save locally).
    pub fn on_slack(&self) -> bool {
        matches!(self.selected_field(), Some(Field::Leader | Field::Binding(_) | Field::ConversationLevel | Field::ChannelDefault | Field::Keywords | Field::Vips | Field::Status | Field::Away | Field::Pause))
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, ws: &Workspace, current: Option<&str>, keys: &Keymap) {
        let [popup] = Layout::horizontal([Constraint::Length(84.min(area.width))]).flex(Flex::Center).areas(area);
        let [popup] = Layout::vertical([Constraint::Length(22.min(area.height))]).flex(Flex::Center).areas(popup);
        super::media::clear_modal(f, popup);
        let saved = match &self.saved {
            Some(Ok(path)) => format!(" saved to {path} "),
            Some(Err(e)) => format!(" not saved: {e} "),
            None => String::new(),
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(super::theme::p().accent))
            .title(format!(" {} ", super::i18n::tr("Preferences")))
            .title_bottom(Line::styled(saved, Style::default().fg(super::theme::p().dim)));
        let inner = block.inner(popup);
        f.render_widget(block, popup);
        let [left, right] = Layout::horizontal([Constraint::Length(22), Constraint::Min(10)]).areas(inner);
        let highlight = Style::default().bg(super::theme::p().selection);
        let cats: Vec<Line> = CATEGORIES
            .iter()
            .map(|c| c.0)
            .chain(["Keys"])
            .enumerate()
            .map(|(n, name)| {
                let line = Line::raw(format!(" {}", super::i18n::tr(name)));
                if n == self.category { line.style(highlight.add_modifier(Modifier::BOLD)) } else { line }
            })
            .collect();
        f.render_widget(Paragraph::new(cats), left);
        let mut rows: Vec<Line> = Vec::new();
        let fields = self.fields();
        let label_w = (fields.iter().map(|f| f.label().chars().count()).max().unwrap_or(0) + 2).min(right.width as usize / 2 + 8);
        // Keep the selection in view (the Keys list is long).
        let room = right.height.saturating_sub(5).max(1) as usize;
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + room {
            self.scroll = self.selected + 1 - room;
        }
        // Each setting keeps to one row: a long value is cut (from the left while typing, so the
        // cursor stays in view).
        let value_w = (right.width as usize).saturating_sub(label_w + 1);
        for (n, field) in fields.iter().enumerate().skip(self.scroll).take(room) {
            let typing = self.editing.is_some() && n == self.selected;
            let value = match (&self.editing, &self.capturing, n == self.selected) {
                (Some(text), _, true) => format!("{text}▏"),
                (_, Some(seq), true) if seq.is_empty() => "press the new key…".into(),
                (_, Some(seq), true) => format!("{} …", super::keymap::sequence_label(seq)),
                _ => field.value(ws, current, keys),
            };
            let value = fit(&value, value_w, typing);
            let label = fit(&field.label(), label_w.saturating_sub(1), false);
            let line = Line::from(vec![Span::raw(format!(" {label:<label_w$}")), Span::styled(value, Style::default().fg(super::theme::p().good))]);
            rows.push(if n == self.selected { line.style(highlight) } else { line });
        }
        let hints: &[&str] = match self.category_name() {
            "Keys" => &["Enter, then press the key (Leader first for a chord). Delete unbinds, r resets."],
            "Availability" => &["Status: \":emoji: text for 30m\" (or today); empty clears."],
            "Notifications" => &["All but the first are kept on Slack. Keywords: comma-separated."],
            "Audio & video" => &[
                "+ / - (or Enter) change the levels; a huddle you are in follows at once.",
                "Below the speaking threshold the microphone sends silence.",
                "KLACK_AUDIO_INPUT / _OUTPUT, when set, win over the devices.",
            ],
            _ => &[],
        };
        let shown = rows.len() as u16;
        f.render_widget(Paragraph::new(rows), right);
        let list = Rect { height: (room as u16).min(right.height), ..right };
        super::scroll::indicator_beside(f, popup, list, self.scroll, room, fields.len(), Style::default().fg(super::theme::p().accent));
        // The hints under a blank row, wrapped at words and indented like the rows.
        let below = Rect { x: right.x + 1, y: right.y + shown + 1, width: right.width.saturating_sub(1), height: right.height.saturating_sub(shown + 1) };
        if !hints.is_empty() && below.height > 0 {
            let dim = Style::default().fg(super::theme::p().dim);
            f.render_widget(Paragraph::new(hints.iter().map(|h| Line::styled(*h, dim)).collect::<Vec<_>>()).wrap(Wrap { trim: true }), below);
        }
        if let Some(picker) = &self.picker {
            picker.draw(f, area, &ws.settings.appearance.theme);
        }
    }
}

/// `text` cut to `room` cells with "…": its start kept, or with `tail` its end.
fn fit(text: &str, room: usize, tail: bool) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    if text.width() <= room {
        return text.to_string();
    }
    let mut kept = Vec::new();
    let mut used = 1;
    let chars: Box<dyn Iterator<Item = char>> = if tail { Box::new(text.chars().rev()) } else { Box::new(text.chars()) };
    for c in chars {
        let w = c.width().unwrap_or(0);
        if used + w > room {
            break;
        }
        used += w;
        kept.push(c);
    }
    if tail {
        std::iter::once('…').chain(kept.into_iter().rev()).collect()
    } else {
        kept.into_iter().chain(['…']).take(room).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_values_fit_one_row() {
        let device = "alsa_output.usb-HP__Inc_HyperX_Cloud_Alpha_Wireless_00000001-00.analog-stereo";
        assert_eq!(fit(device, 20, false), "alsa_output.usb-HP_…");
        assert_eq!(fit("short", 20, false), "short");
        assert_eq!(fit(&format!("{device}▏"), 12, true), "…log-stereo▏", "typing: the end with the cursor");
        assert_eq!(fit("日本語のテキスト", 7, false), "日本語…", "wide characters count two cells");
        assert_eq!(fit("abc", 0, false), "");
        assert_eq!(fit("abcdef", 1, false), "…");
    }

    #[test]
    fn changing_values() {
        let mut s = Settings::default();
        Field::Compact.change(&mut s, &Default::default());
        assert!(s.appearance.compact);
        Field::DateFormat.change(&mut s, &Default::default());
        assert_eq!(s.region.date_format, DATE_FORMATS[1]);
        s.region.date_format = "%custom".into();
        Field::DateFormat.change(&mut s, &Default::default());
        assert_eq!(s.region.date_format, DATE_FORMATS[0], "an unknown format starts the list again");
        let devices = (vec![], vec!["hw:0".to_string(), "hw:1".to_string()]);
        let steps: Vec<Option<String>> = (0..3)
            .map(|_| {
                Field::Output.change(&mut s, &devices);
                s.audio.output.clone()
            })
            .collect();
        assert_eq!(steps, vec![Some("hw:0".into()), Some("hw:1".into()), None], "default → each device → default");
    }

    #[test]
    fn audio_levels_step() {
        let mut s = Settings::default();
        assert!(Field::HuddleVolume.adjust(&mut s, true, false));
        assert_eq!(s.audio.huddle_volume, 125);
        s.audio.huddle_volume = 200;
        Field::HuddleVolume.adjust(&mut s, true, false);
        assert_eq!(s.audio.huddle_volume, 200, "+ stops at the top");
        Field::HuddleVolume.change(&mut s, &Default::default());
        assert_eq!(s.audio.huddle_volume, 0, "Enter wraps around");
        Field::HuddleVolume.adjust(&mut s, false, false);
        assert_eq!(s.audio.huddle_volume, 0, "- stops at the bottom");
        s.audio.mic_gain = 90;
        Field::MicGain.adjust(&mut s, false, false);
        assert_eq!(s.audio.mic_gain, 75, "a hand-edited value steps to its neighbour");
        Field::SpeakingThreshold.adjust(&mut s, true, false);
        assert_eq!(s.audio.speaking_threshold_db, Some(-60), "off → the lowest threshold");
        Field::SpeakingThreshold.adjust(&mut s, false, false);
        assert_eq!(s.audio.speaking_threshold_db, None);
        Field::NoiseSuppression.change(&mut s, &Default::default());
        assert!(s.audio.noise_suppression);
        Field::EchoCancellation.change(&mut s, &Default::default());
        assert!(!s.audio.echo_cancellation, "on by default, so the first change turns it off");
        assert!(!Field::Compact.adjust(&mut s, true, false), "not a level");
    }

    #[test]
    fn huddle_sounds_cycle() {
        assert_eq!(Settings::default().huddle.join_sound, "rise", "on by default");
        let mut s = "off".to_string();
        let mut seen = Vec::new();
        for _ in 0..super::super::sound::HUDDLE_SOUNDS.len() {
            s = next_sound(&s);
            seen.push(s.clone());
        }
        assert_eq!(seen, ["rise", "pop", "fall", "chime", "ping", "knock", "off"]);
        let h = Settings::default().huddle;
        assert_eq!((h.someone_joined.as_str(), h.someone_left.as_str()), ("pop", "fall"));
        assert_eq!((h.ring_sound.as_str(), h.ring_timeout_secs, h.auto_decline), ("ring", 30, false), "rings 30 s, no auto-decline by default");
        let mut s = Settings::default();
        assert!(Field::RingTimeout.adjust(&mut s, true, false));
        assert_eq!(s.huddle.ring_timeout_secs, 45);
        s.huddle.ring_timeout_secs = 120;
        Field::RingTimeout.adjust(&mut s, true, true);
        assert_eq!(s.huddle.ring_timeout_secs, 15, "Enter wraps around");
        Field::AutoDecline.change(&mut s, &Default::default());
        assert!(s.huddle.auto_decline);
        assert_eq!(next_sound("unknown"), "off", "a hand-edited name starts over");
    }

    #[test]
    fn choosing_a_camera() {
        let mut ws = Workspace::default();
        let mut p = Prefs { category: 5, cameras: vec!["cam A".into(), "cam B".into()], ..Default::default() };
        assert_eq!(p.category_name(), "Audio & video");
        p.selected = p.fields().iter().position(|f| *f == Field::Camera).unwrap();
        let steps: Vec<Option<String>> = (0..3)
            .map(|_| {
                p.change(&mut ws, None);
                ws.settings.video.camera.clone()
            })
            .collect();
        assert_eq!(steps, vec![Some("cam A".into()), Some("cam B".into()), None], "none → each camera → none");
        assert!(!p.on_slack());
    }

    #[test]
    fn slack_side_changes() {
        let mut ws = Workspace::default();
        let mut p = Prefs { category: 4, ..Default::default() };
        p.selected = 2;
        assert_eq!(p.change(&mut ws, Some("C1")), Some(SlackChange::ConversationLevel { channel: "C1".into(), level: "everything".into() }), "default → everything");
        let mut cycle = Vec::new();
        for _ in 0..4 {
            let Some(SlackChange::ConversationLevel { level, .. }) = p.change(&mut ws, Some("C1")) else { panic!() };
            if level == "default" { ws.notify.channels.remove("C1") } else { ws.notify.channels.insert("C1".into(), level.clone()) };
            cycle.push(level);
        }
        assert_eq!(cycle, ["everything", "mentions_dms", "nothing", "default"], "back to the default after nothing");
        assert_eq!(p.change(&mut ws, None), None, "no conversation, no change");
        p.selected = 3;
        assert_eq!(p.change(&mut ws, None), Some(SlackChange::ChannelDefault("nothing".into())));
        p.selected = 4;
        ws.notify.keywords = vec!["a".into()];
        assert_eq!(p.change(&mut ws, None), None, "Enter starts editing");
        assert_eq!(p.editing.as_deref(), Some("a"));
        p.editing = Some(" deploy ,, release notes ".into());
        assert_eq!(p.change(&mut ws, None), Some(SlackChange::Keywords("deploy,release notes".into())));
        assert!(p.editing.is_none() && p.on_slack());
    }

    #[test]
    fn availability_becomes_commands() {
        let mut ws = Workspace { self_id: "UME".into(), ..Default::default() };
        let mut p = Prefs::default();
        assert_eq!(CATEGORIES[0].0, "Availability");
        p.selected = 1;
        assert_eq!(p.change(&mut ws, None), Some(SlackChange::Command("/away".into())));
        ws.presence.insert("UME".into(), false);
        assert_eq!(p.change(&mut ws, None), Some(SlackChange::Command("/active".into())));
        p.selected = 2;
        assert_eq!(p.change(&mut ws, None), Some(SlackChange::Command("/dnd 30m".into())), "not paused → 30 minutes");
        ws.dnd.snooze_until = Some(crate::tui::clock::now().timestamp() as f64 + 29.6 * 60.0);
        assert_eq!(p.change(&mut ws, None), Some(SlackChange::Command("/dnd 60m".into())), "30 min left → an hour");
        ws.dnd.snooze_until = Some(crate::tui::clock::now().timestamp() as f64 + 470.0 * 60.0);
        assert_eq!(p.change(&mut ws, None), Some(SlackChange::Command("/dnd off".into())), "the longest → off");
        p.selected = 0;
        assert_eq!(p.change(&mut ws, None), None, "Enter edits the status");
        p.editing = Some(":coffee: break for 30m".into());
        assert_eq!(p.change(&mut ws, None), Some(SlackChange::Command("/status :coffee: break for 30m".into())));
        p.editing = Some("  ".into());
        assert_eq!(p.change(&mut ws, None), Some(SlackChange::Command("/status clear".into())));
    }

    #[test]
    fn categories_cycle() {
        let mut p = Prefs::default();
        p.switch_category(-1);
        assert_eq!((p.category, p.category_name()), (CATEGORIES.len(), "Keys"));
        assert_eq!(p.fields().len(), ACTIONS.len() + 1);
        p.switch_category(-1);
        p.move_by(10);
        assert_eq!(p.selected, p.fields().len() - 1);
    }

    #[test]
    fn workspace_themes() {
        let mut ws = Workspace { team_id: "T1".into(), ..Default::default() };
        let p = Prefs { category: 1, selected: 1, ..Default::default() };
        assert_eq!(p.selected_field(), Some(Field::WorkspaceTheme));
        assert_eq!(Field::WorkspaceTheme.value(&ws, None, &Keymap::default()), "(colour theme)");
        ws.settings.appearance.workspace_themes.insert("T1".into(), "light".into());
        assert_eq!(Field::WorkspaceTheme.value(&ws, None, &Keymap::default()), "light");
        assert_eq!((ws.settings.appearance.theme_for("T1"), ws.settings.appearance.theme_for("T2")), ("light", "dark"));
        let text = toml::to_string(&ws.settings).unwrap();
        assert!(text.contains("[appearance.workspace_themes]\nT1 = \"light\""), "{text}");
        assert_eq!(Settings::parse(&text).unwrap().appearance.theme_for("T1"), "light");
        assert!(!toml::to_string(&Settings::default()).unwrap().contains("workspace_themes"), "none: not written");
    }
}

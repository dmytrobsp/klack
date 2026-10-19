//! Preferences (F6): local settings by category; a change applies at once and is saved
//! to settings.toml.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use std::time::{Duration, Instant};

use super::keymap::{ACTIONS, Action, Key, Keymap};
use super::settings::Settings;
use super::state::{LEVELS, Workspace, level_label};
use slack_core::dnd::{DayHours, ScheduleDays, ScheduleEdit};

/// Day-separator formats offered (strftime).
pub const DATE_FORMATS: &[&str] = &["%A, %B %-d", "%a %-d %b", "%d.%m.%Y", "%Y-%m-%d", "%m/%d/%Y"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Theme,
    WorkspaceTheme,
    HideInactive,
    /// which-key: on or off.
    WhichKeyShow,
    /// which-key: the corner of the screen it sits in.
    WhichKeyCorner,
    /// which-key: how long after the last key it opens.
    WhichKeyDelay,
    /// which-key: its tallest, in percent of the screen.
    WhichKeyHeight,
    /// Icons in which-key, help and the Keys list.
    WhichKeyIcons,
    /// The leader key for chords.
    Leader,
    /// The leader while typing and in dialogs.
    InsertLeader,
    /// A section of the key tree (index into `keymap::keys_tree`); a group's can be renamed.
    GroupHeader(usize),
    /// An action's keys (index into `keymap::ACTIONS`).
    Binding(usize),
    Compact,
    InlineImages,
    Avatars,
    LargeAvatars,
    PreviewRefresh,
    Graphics,
    EmojiAsNames,
    CustomEmojiPictures,
    SaveDir,
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
    /// Slack: your VIPs (changed on a profile card).
    Vips,
    /// Slack: your status (a text field: `:emoji: text`).
    Status,
    /// Slack: appear away.
    Away,
    /// Slack: notifications on, or paused until …
    Notifications,
    /// Slack: the notification schedule (off / every day / weekdays / custom).
    Schedule,
    /// Slack: every day / weekdays, the hours notifications come through.
    ScheduleHours,
    /// Slack: a custom schedule's day (0 = Monday): its hours, all day or none.
    ScheduleDay(usize),
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
/// Days without a message before a conversation leaves the sidebar (0: never).
const HIDE_INACTIVE: &[u32] = &[0, 1, 7, 14, 30, 60, 90];

/// Pause lengths + and - step through, in minutes.
const PAUSES: &[u64] = &[30, 60, 120, 480];

/// The schedule's kinds, in the order Enter goes through them (from off: every day).
const KINDS: [ScheduleDays; 4] = [ScheduleDays::Off, ScheduleDays::EveryDay, ScheduleDays::Weekdays, ScheduleDays::Custom];

/// Shown for a schedule (or a day) with a value klack doesn't know; it is never written.
const UNKNOWN_SCHEDULE: &str = "unknown to klack — change it in Slack";

pub const CATEGORIES: &[(&str, &[Field])] = &[
    ("Availability", &[Field::Status, Field::Away, Field::Notifications, Field::Schedule]),
    ("Appearance", &[Field::Theme, Field::WorkspaceTheme, Field::Compact, Field::HideInactive]),
    ("Messages & media", &[Field::InlineImages, Field::Avatars, Field::LargeAvatars, Field::PreviewRefresh, Field::Graphics, Field::EmojiAsNames, Field::CustomEmojiPictures, Field::SaveDir]),
    ("Language & region", &[Field::Clock24, Field::DateFormat, Field::TimeZone, Field::Language]),
    ("Notifications", &[Field::Desktop, Field::Sound, Field::ConversationLevel, Field::ChannelDefault, Field::Keywords, Field::Vips]),
    ("Audio & video", &[Field::Input, Field::Output, Field::HuddleVolume, Field::SoundVolume, Field::MicGain, Field::SpeakingThreshold, Field::NoiseSuppression, Field::EchoCancellation, Field::Camera]),
    ("Huddles", &[Field::JoinSound, Field::SomeoneJoined, Field::SomeoneLeft, Field::RingSound, Field::RingTimeout, Field::AutoDecline, Field::AloneSound, Field::AloneSecs]),
];

/// The Keys category's settings above the bindings.
const KEYS_SETTINGS: &[Field] = &[Field::WhichKeyShow, Field::WhichKeyCorner, Field::WhichKeyDelay, Field::WhichKeyHeight, Field::WhichKeyIcons, Field::Leader, Field::InsertLeader];

/// which-key's corners, in the order Enter goes through them.
const CORNERS: &[&str] = &["bottom-right", "bottom-left", "top-left", "top-right"];

/// A recording of keys ends after this long without a key.
pub const RECORD_PAUSE: Duration = Duration::from_secs(1);

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
            Field::InsertLeader => super::i18n::tr(INSERT_LEADER_LABEL).into_owned(),
            // Drawn from the tree (see `Prefs::draw`).
            Field::GroupHeader(_) => String::new(),
            Field::Binding(i) => format!("{} · {}", super::i18n::tr(ACTIONS[i].context.title()), super::i18n::tr(ACTIONS[i].description)),
            other => super::i18n::tr(other.fixed_label()).into_owned(),
        }
    }

    fn fixed_label(self) -> &'static str {
        match self {
            Field::Leader | Field::InsertLeader | Field::GroupHeader(_) | Field::Binding(_) => "",
            Field::WhichKeyShow => "Show which-key",
            Field::WhichKeyCorner => "which-key corner",
            Field::WhichKeyDelay => "which-key opens after",
            Field::WhichKeyHeight => "which-key max height",
            Field::WhichKeyIcons => "Icons (which-key, help, here)",
            Field::Theme => "Colour theme",
            Field::WorkspaceTheme => "Theme for this workspace",
            Field::HideInactive => "Hide conversations quiet for",
            Field::Compact => "Compact (no gaps between authors or sidebar sections)",
            Field::InlineImages => "Show image thumbnails",
            Field::Avatars => "Show profile pictures",
            Field::LargeAvatars => "Two-row profile pictures",
            Field::PreviewRefresh => "Refresh previewed channels",
            Field::Graphics => "Draw pictures with",
            Field::EmojiAsNames => "Show emoji as :names:",
            Field::CustomEmojiPictures => "Custom emoji as pictures",
            Field::SaveDir => "Save files to",
            Field::Clock24 => "24-hour clock",
            Field::DateFormat => "Date format",
            Field::TimeZone => "Time zone",
            Field::Language => "Language (locales/<code>.toml)",
            Field::Desktop => "Desktop notifications",
            Field::Sound => "Sound",
            Field::ConversationLevel => "This conversation notifies about",
            Field::ChannelDefault => "Channels notify about (default)",
            Field::Keywords => "Keywords",
            Field::Vips => "VIPs (also on a profile card)",
            Field::Status => "Status",
            Field::Away => "Appear away",
            Field::Notifications => "Notifications",
            Field::Schedule => "Notification schedule",
            Field::ScheduleHours => "Receive notifications",
            Field::ScheduleDay(i) => ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"].get(i).copied().unwrap_or_default(),
            Field::Input => "Microphone",
            Field::Output => "Speaker",
            Field::HuddleVolume => "Huddle volume",
            Field::SoundVolume => "Notification sound volume",
            Field::MicGain => "Microphone gain",
            Field::SpeakingThreshold => "Speaking threshold",
            Field::NoiseSuppression => "Noise suppression",
            Field::EchoCancellation => "Echo cancellation",
            Field::Camera => "Camera (for huddles)",
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
            Field::InsertLeader => return keys.insert_leader.map(|k| k.label()).unwrap_or_else(|| "none".into()),
            Field::GroupHeader(_) => return String::new(),
            Field::Binding(i) => {
                let label = keys.keys_label(ACTIONS[i].context, ACTIONS[i].name);
                return if label.is_empty() { "(unbound)".into() } else { label };
            }
            _ => {}
        }
        let yes = |b: bool| if b { "on" } else { "off" }.to_string();
        let device = |d: &Option<String>| d.clone().unwrap_or_else(|| "system default".into());
        match self {
            Field::Leader | Field::InsertLeader | Field::GroupHeader(_) | Field::Binding(_) => String::new(),
            Field::WhichKeyShow => yes(s.which_key.show),
            Field::WhichKeyCorner => s.which_key.corner.clone(),
            Field::WhichKeyDelay if s.which_key.delay_secs <= 0.0 => "0 s (at once)".into(),
            Field::WhichKeyDelay => format!("{:.1} s", s.which_key.delay_secs),
            Field::WhichKeyHeight => format!("{}% of the screen", s.which_key.max_height_percent),
            Field::WhichKeyIcons => yes(s.which_key.icons),
            Field::Theme => s.appearance.theme.clone(),
            Field::HideInactive => match s.appearance.hide_inactive_days {
                0 => "never".into(),
                1 => "a day".into(),
                n => format!("{n} days"),
            },
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
            Field::CustomEmojiPictures => yes(s.messages.custom_emoji_pictures),
            Field::SaveDir if s.messages.save_dir.trim().is_empty() => format!("{} (your downloads folder)", super::save::shown_folder(&super::files::download_dir(""))),
            Field::SaveDir => s.messages.save_dir.clone(),
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
            Field::Notifications => {
                let now = super::clock::now().timestamp();
                match ws.dnd.until(now as f64).and_then(|t| super::clock::until_label(t as i64, now, s.time_format())) {
                    Some(label) => format!("paused until {label}"),
                    None => "on".into(),
                }
            }
            Field::Schedule => match ws.dnd.schedule.as_ref() {
                None => "not known yet".into(),
                Some(sch) => match sch.days {
                    ScheduleDays::Off => "off".into(),
                    ScheduleDays::EveryDay => "every day".into(),
                    // Seen on the test account: with it on, Slack applies no weekday hours.
                    ScheduleDays::Weekdays if sch.weekdays_off_allday => "weekdays (Slack's \"weekdays off all day\" is on, so these hours aren't applied — change it in Slack)".into(),
                    ScheduleDays::Weekdays => "weekdays".into(),
                    ScheduleDays::Custom => "custom".into(),
                    ScheduleDays::Unknown => UNKNOWN_SCHEDULE.into(),
                },
            },
            Field::ScheduleHours => ws.dnd.schedule.as_ref().map(|sch| format!("from {} to {}{}", clock(&sch.range.0, s.time_format()), clock(&sch.range.1, s.time_format()), schedule_zone(ws))).unwrap_or_default(),
            Field::ScheduleDay(i) => match ws.dnd.schedule.as_ref().and_then(|sch| sch.per_day.get(i)) {
                Some(DayHours::Between(from, to)) => format!("{} to {}{}", clock(from, s.time_format()), clock(to, s.time_format()), schedule_zone(ws)),
                Some(DayHours::AllDay) => "all day".into(),
                Some(DayHours::Never) => "none".into(),
                Some(DayHours::Unknown(v)) => format!("{v:?}: {UNKNOWN_SCHEDULE}"),
                None => String::new(),
            },
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
            Field::HideInactive => s.appearance.hide_inactive_days = step(HIDE_INACTIVE, s.appearance.hide_inactive_days, up, wrap),
            Field::MicGain => a.mic_gain = step(GAINS, a.mic_gain, up, wrap),
            Field::SpeakingThreshold => a.speaking_threshold_db = step(THRESHOLDS, a.speaking_threshold_db, up, wrap),
            // 0.1 s steps from 0 to 5 s.
            Field::WhichKeyDelay => {
                let w = &mut s.which_key;
                let tenths = (w.delay_secs * 10.0).round() as i32 + if up { 1 } else { -1 };
                w.delay_secs = (if wrap && tenths > 50 { 0 } else { tenths.clamp(0, 50) }) as f32 / 10.0;
            }
            // 10 % steps from 10 to 100 %.
            Field::WhichKeyHeight => {
                let w = &mut s.which_key;
                let next = if up { w.max_height_percent.saturating_add(10) } else { w.max_height_percent.saturating_sub(10) };
                w.max_height_percent = if wrap && next > 100 { 10 } else { next.clamp(10, 100) };
            }
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
            Field::TimeZone | Field::Language | Field::WorkspaceTheme | Field::SaveDir => {}
            // Chosen in a picker (`Prefs::change`).
            Field::Theme => {}
            Field::Compact => s.appearance.compact ^= true,
            Field::InlineImages => s.messages.inline_images ^= true,
            Field::Avatars => s.messages.avatars ^= true,
            Field::LargeAvatars => s.messages.large_avatars ^= true,
            Field::PreviewRefresh | Field::HideInactive => {
                self.adjust(s, true, true);
            }
            Field::Graphics => {
                let i = super::media::GRAPHICS.iter().position(|g| *g == s.messages.graphics).map_or(0, |i| (i + 1) % super::media::GRAPHICS.len());
                s.messages.graphics = super::media::GRAPHICS[i].into();
            }
            Field::EmojiAsNames => s.messages.emoji_as_names ^= true,
            Field::CustomEmojiPictures => s.messages.custom_emoji_pictures ^= true,
            Field::Clock24 => s.region.clock_24h ^= true,
            Field::DateFormat => {
                let i = DATE_FORMATS.iter().position(|f| *f == s.region.date_format).map_or(0, |i| (i + 1) % DATE_FORMATS.len());
                s.region.date_format = DATE_FORMATS[i].into();
            }
            Field::Desktop => s.notifications.desktop ^= true,
            Field::WhichKeyShow => s.which_key.show ^= true,
            Field::WhichKeyIcons => s.which_key.icons ^= true,
            Field::WhichKeyCorner => {
                let i = CORNERS.iter().position(|c| *c == s.which_key.corner).map_or(0, |i| (i + 1) % CORNERS.len());
                s.which_key.corner = CORNERS[i].into();
            }
            Field::WhichKeyDelay | Field::WhichKeyHeight => {
                self.adjust(s, true, true);
            }
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
            Field::ConversationLevel | Field::ChannelDefault | Field::Keywords | Field::Vips | Field::Status | Field::Away | Field::Notifications | Field::Schedule | Field::ScheduleHours | Field::ScheduleDay(_) | Field::Leader | Field::InsertLeader | Field::GroupHeader(_) | Field::Binding(_) => {}
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
    /// Change the notification schedule.
    Schedule(Vec<slack_core::dnd::ScheduleEdit>),
}

/// A conversation's own level cycles through the three and back to "default" (no override).
pub fn next_conversation_level(own: Option<&str>) -> String {
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
const INSERT_LEADER_LABEL: &str = "Leader while typing and in dialogs";
/// Shown while a recording waits for its first key.
const RECORD_START: &str = "press the keys… (a 1 s pause ends it)";
/// Shown once a recording is over.
const RECORD_DONE: &str = "Enter keep · Esc discard";

/// Keys pressed for a binding in F6 → Keys: every key counts (Enter and Esc too) until a
/// pause of [`RECORD_PAUSE`]; then Enter keeps them and Esc throws them away.
#[derive(Debug, Clone)]
pub struct Recording {
    pub keys: Vec<Key>,
    /// When the last key was pressed (or the recording started).
    pub last: Instant,
    /// The pause came: waiting for Enter or Esc.
    pub done: bool,
}

impl Recording {
    pub fn new(now: Instant) -> Recording {
        Recording { keys: Vec::new(), last: now, done: false }
    }

    /// Over once keys were pressed and none came for [`RECORD_PAUSE`].
    pub fn tick(&mut self, now: Instant) {
        if self.due().is_some_and(|due| due <= now) {
            self.done = true;
        }
    }

    /// When the pause ends the recording (None before the first key or once it is over).
    pub fn due(&self) -> Option<Instant> {
        (!self.done && !self.keys.is_empty()).then(|| self.last + RECORD_PAUSE)
    }

    /// A key pressed: recorded until the pause; after it Enter keeps (`Some(true)`), Esc
    /// discards (`Some(false)`), other keys do nothing.
    pub fn press(&mut self, key: Key, now: Instant) -> Option<bool> {
        use crossterm::event::{KeyCode, KeyModifiers};
        self.tick(now);
        if !self.done {
            if !matches!(key.code, KeyCode::Modifier(_)) {
                self.keys.push(key);
                self.last = now;
            }
            return None;
        }
        match (key.code, key.mods) {
            (KeyCode::Enter, KeyModifiers::NONE) => Some(true),
            (KeyCode::Esc, KeyModifiers::NONE) => Some(false),
            _ => None,
        }
    }
}

/// The selection in F6 → Keys, kept across a change of the keymap (see `Prefs::anchor`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Anchor {
    Field(Field),
    /// A section header: the group's prefix, else the title.
    Section(Option<Vec<Key>>, String),
}

/// Recorded keys in keys.toml notation: a leader that starts a longer sequence is
/// `<leader>`; a leader alone (or later in it) is the key itself.
pub fn recorded(keys: &Keymap, seq: &[Key]) -> String {
    let leaders = [keys.leader, keys.insert_leader];
    seq.iter().enumerate().map(|(n, k)| if n == 0 && seq.len() > 1 && leaders.contains(&Some(*k)) { "<leader>".to_string() } else { k.spec() }).collect::<Vec<_>>().join(" ")
}

/// Category names and fixed labels, for translation templates.
pub fn translatable() -> Vec<String> {
    let all = [
        Field::Theme, Field::WorkspaceTheme, Field::Compact, Field::HideInactive, Field::InlineImages, Field::Avatars, Field::LargeAvatars, Field::PreviewRefresh, Field::Graphics, Field::EmojiAsNames, Field::CustomEmojiPictures, Field::SaveDir, Field::Clock24, Field::DateFormat, Field::TimeZone, Field::Language, Field::Desktop, Field::Sound,
        Field::ConversationLevel, Field::ChannelDefault, Field::Keywords, Field::Vips, Field::Status, Field::Away, Field::Notifications, Field::Schedule, Field::ScheduleHours, Field::Input, Field::Output,
        Field::HuddleVolume, Field::SoundVolume, Field::MicGain, Field::SpeakingThreshold, Field::NoiseSuppression, Field::EchoCancellation, Field::Camera, Field::JoinSound, Field::SomeoneJoined, Field::SomeoneLeft, Field::RingSound, Field::RingTimeout, Field::AutoDecline, Field::AloneSound, Field::AloneSecs,
    ];
    let all = [&all[..], &(0..7).map(Field::ScheduleDay).collect::<Vec<_>>()].concat();
    CATEGORIES.iter().map(|c| c.0).chain(["Keys", LEADER_LABEL, INSERT_LEADER_LABEL, RECORD_START, RECORD_DONE, "filter", "to filter", "to change", "Enter keep · Esc clear"]).chain(all.iter().chain(KEYS_SETTINGS).map(|f| f.fixed_label())).map(String::from).collect()
}

#[derive(Debug, Default)]
pub struct Prefs {
    /// Text being typed into a field (keywords, a group's name).
    pub editing: Option<String>,
    /// The text being typed is a group's icon, not its name.
    pub editing_icon: bool,
    /// Keys being recorded for the selected Keys entry.
    pub recording: Option<Recording>,
    /// Keys: only the rows that contain this.
    pub filter: String,
    /// Keys go to the filter (after `/`) until Enter or Esc.
    pub filter_focused: bool,
    /// First row shown in the settings list.
    scroll: usize,
    pub category: usize,
    pub selected: usize,
    /// The row a click selected (a second click on it acts); keys, another category or filter
    /// and acting forget it.
    pub clicked: Option<usize>,
    /// Audio devices as ids (listed when the menu opens; only names, nothing is opened).
    pub devices: (Vec<String>, Vec<String>),
    /// Camera names (listed when the menu opens; nothing is opened).
    pub cameras: Vec<String>,
    /// Where the settings were saved, or why not.
    pub saved: Option<Result<String, String>>,
    /// Choosing a theme (the colour theme, or the workspace's own).
    pub picker: Option<super::theme_picker::ThemePicker>,
    /// Last drawn area, for clicks.
    pub area: Rect,
    /// The category column and the settings' rows at the last draw, and how many rows are shown.
    pub cats: Rect,
    pub list: Rect,
    pub shown: usize,
    /// The notification schedule's kind at the last `sync`: which of its rows Availability shows.
    schedule: Option<slack_core::dnd::ScheduleDays>,
}

impl Prefs {
    pub fn new() -> Prefs {
        let id = |d: &String| d.split(" — ").next().unwrap_or(d).to_string();
        let devices = huddle::devices::list_devices().map(|(i, o)| (i.iter().map(id).collect(), o.iter().map(id).collect())).unwrap_or_default();
        let cameras = huddle::camera::list_cameras().into_iter().map(|c| c.name).collect();
        Prefs { devices, cameras, ..Default::default() }
    }

    pub fn fields(&self, keys: &Keymap) -> Vec<Field> {
        let Some((_, fields)) = CATEGORIES.get(self.category) else {
            // The last category: the which-key settings, the leaders, then the bindings by the
            // tree, each section under its header; the filter keeps the rows that contain it.
            let filter = self.filter.to_lowercase();
            let hit = |text: &str| text.to_lowercase().contains(&filter);
            let mut out: Vec<Field> = KEYS_SETTINGS.iter().copied().filter(|f| hit(&f.label())).collect();
            for (n, section) in super::keymap::keys_tree(keys).iter().enumerate() {
                let rows: Vec<Field> = section.rows.iter().filter(|r| hit(&r.label) || hit(&super::i18n::tr(ACTIONS[r.info].description)) || hit(&r.keys)).map(|r| Field::Binding(r.info)).collect();
                if !rows.is_empty() {
                    out.push(Field::GroupHeader(n));
                    out.extend(rows);
                }
            }
            return out;
        };
        let mut out = fields.to_vec();
        if self.availability_shown() {
            out.extend(self.schedule_rows());
        }
        out
    }

    /// Whether Availability is shown (its rows follow the schedule).
    fn availability_shown(&self) -> bool {
        CATEGORIES.get(self.category).is_some_and(|c| c.0 == "Availability")
    }

    /// The schedule's rows under its kind: the hours for every day / weekdays, the days for custom.
    fn schedule_rows(&self) -> Vec<Field> {
        match self.schedule {
            Some(ScheduleDays::EveryDay | ScheduleDays::Weekdays) => vec![Field::ScheduleHours],
            Some(ScheduleDays::Custom) => (0..7).map(Field::ScheduleDay).collect(),
            _ => Vec::new(),
        }
    }

    /// Follow the workspace: the schedule's rows shown, the selection kept on one of them.
    pub fn sync(&mut self, ws: &Workspace) {
        let row = |p: &Prefs| p.availability_shown().then(|| CATEGORIES[p.category].1.iter().copied().chain(p.schedule_rows()).nth(p.selected)).flatten();
        let before = row(self);
        self.schedule = ws.dnd.schedule.as_ref().map(|s| s.days);
        if self.availability_shown() {
            self.selected = self.selected.min(CATEGORIES[self.category].1.len() + self.schedule_rows().len() - 1);
        }
        // The schedule changed in Slack under a row being typed in: what was typed is dropped,
        // never sent for the row now in its place.
        if self.editing.is_some() && row(self) != before {
            self.editing = None;
            self.saved = Some(Err("the notification schedule changed in Slack: what you typed was dropped".into()));
        }
    }

    /// Enter (`step` None) or + / - on a schedule row: the edits to send, or why not; None when
    /// there is nothing to send (Enter opened the text field).
    pub fn schedule_change(&mut self, ws: &Workspace, field: Field, step: Option<bool>) -> Option<Result<Vec<ScheduleEdit>, String>> {
        let typed = self.editing.take();
        let Some(schedule) = ws.dnd.schedule.as_ref() else { return Some(Err("the notification schedule isn't known yet".into())) };
        if schedule.days == ScheduleDays::Unknown {
            return Some(Err(format!("the notification schedule is {UNKNOWN_SCHEDULE}")));
        }
        let edit = match field {
            Field::Schedule => {
                let i = KINDS.iter().position(|k| *k == schedule.days).unwrap_or(0);
                ScheduleEdit::Days(KINDS[if step == Some(false) { i + KINDS.len() - 1 } else { i + 1 } % KINDS.len()])
            }
            Field::ScheduleHours => match (step, typed) {
                (Some(_), _) => return None,
                (None, Some(text)) => match super::commands::receive_hours(&text) {
                    Ok((from, to)) => ScheduleEdit::Range(from, to),
                    Err(e) => return Some(Err(e)),
                },
                (None, None) => {
                    self.editing = Some(format!("{}-{}", schedule.range.0, schedule.range.1));
                    return None;
                }
            },
            Field::ScheduleDay(i) => {
                let day = schedule.per_day.get(i)?;
                // Back to hours, a day gets the every-day hours.
                let hours = || DayHours::Between(schedule.range.0.clone(), schedule.range.1.clone());
                // Hours → all day → none → hours.
                let (next, before) = match day {
                    DayHours::Between(..) => (DayHours::AllDay, DayHours::Never),
                    DayHours::AllDay => (DayHours::Never, hours()),
                    DayHours::Never => (hours(), DayHours::AllDay),
                    DayHours::Unknown(_) => return Some(Err(format!("{} on Slack is {UNKNOWN_SCHEDULE}", field.label()))),
                };
                let hours = match (step, typed, day) {
                    (Some(true), ..) => next,
                    (Some(false), ..) => before,
                    (None, Some(text), _) => match text.trim().to_lowercase().as_str() {
                        "all day" => DayHours::AllDay,
                        "none" => DayHours::Never,
                        text => match super::commands::receive_hours(text) {
                            Ok((from, to)) => DayHours::Between(from, to),
                            Err(e) => return Some(Err(format!("{e} (or all day, or none)"))),
                        },
                    },
                    (None, None, DayHours::Between(from, to)) => {
                        self.editing = Some(format!("{from}-{to}"));
                        return None;
                    }
                    (None, None, _) => next,
                };
                ScheduleEdit::Day(i, hours)
            }
            _ => return None,
        };
        Some(Ok(vec![edit]))
    }

    pub fn category_name(&self) -> &'static str {
        CATEGORIES.get(self.category).map_or("Keys", |c| c.0)
    }

    pub fn switch_category(&mut self, delta: isize) {
        self.category = (self.category as isize + delta).rem_euclid(CATEGORIES.len() as isize + 1) as usize;
        self.set_filter(String::new());
        self.filter_focused = false;
    }

    /// The category drawn at a terminal position.
    pub fn category_at(&self, column: u16, row: u16) -> Option<usize> {
        let n = row.checked_sub(self.cats.y)? as usize;
        (self.cats.contains((column, row).into()) && n <= CATEGORIES.len()).then_some(n)
    }

    /// The row (an index into `fields`, a group's header too) drawn at a terminal position.
    pub fn row_at(&self, column: u16, row: u16) -> Option<usize> {
        let n = row.checked_sub(self.list.y)? as usize;
        (self.list.contains((column, row).into()) && n < self.shown).then_some(self.scroll + n)
    }

    pub fn selected_field(&self, keys: &Keymap) -> Option<Field> {
        self.fields(keys).get(self.selected).copied()
    }

    /// A click on row `n`: selects it; true when it should act as Enter (it was selected by
    /// the click before, so a first click never changes a setting).
    pub fn click(&mut self, n: usize) -> bool {
        self.filter_focused = false;
        let act = self.clicked == Some(n) && self.selected == n;
        self.selected = n;
        self.clicked = if act { None } else { Some(n) };
        act
    }

    pub fn move_by(&mut self, delta: isize, keys: &Keymap) {
        self.clicked = None;
        self.selected = (self.selected as isize + delta).clamp(0, (self.fields(keys).len() as isize - 1).max(0)) as usize;
    }

    /// Whether the key list is shown (typing goes to its filter).
    pub fn keys_shown(&self) -> bool {
        self.category == CATEGORIES.len()
    }

    /// A new filter for the key list: the selection goes to its top.
    pub fn set_filter(&mut self, filter: String) {
        self.filter = filter;
        self.selected = 0;
        self.clicked = None;
        self.scroll = 0;
    }

    /// A key while the filter has the focus: text goes into it (Space and letters too), the
    /// text field's erase takes one back, done keeps it and cancel clears it; both leave it.
    pub fn filter_key(&mut self, key: Key, action: Option<Action>) {
        let mut filter = self.filter.clone();
        match (action, key.code) {
            (Some(Action::Open), _) => self.filter_focused = false,
            (Some(Action::Close), _) => (filter, self.filter_focused) = (String::new(), false),
            (Some(Action::Erase), _) => {
                filter.pop();
            }
            (_, crossterm::event::KeyCode::Char(c)) if key.is_text() => filter.push(c),
            _ => {}
        }
        // A new filter: the selection goes to its top.
        if filter != self.filter {
            self.set_filter(filter);
        }
    }

    /// What is selected, in a form that a changed keymap keeps (a header by its group's prefix
    /// or its title; sections come and go as keys are rebound).
    pub fn anchor(&self, keys: &Keymap) -> Option<Anchor> {
        match self.selected_field(keys)? {
            Field::GroupHeader(n) => super::keymap::keys_tree(keys).into_iter().nth(n).map(|s| Anchor::Section(s.prefix, s.title)),
            f => Some(Anchor::Field(f)),
        }
    }

    /// Select what `anchor` was before the keymap changed, wherever it is now (if still listed).
    pub fn reselect(&mut self, anchor: Option<Anchor>, keys: &Keymap) {
        let Some(anchor) = anchor else { return };
        let sections = super::keymap::keys_tree(keys);
        let found = self.fields(keys).iter().position(|f| match (f, &anchor) {
            (Field::GroupHeader(n), Anchor::Section(prefix, title)) => sections.get(*n).is_some_and(|s| if prefix.is_some() { s.prefix == *prefix } else { s.title == *title }),
            (f, Anchor::Field(a)) => f == a,
            _ => false,
        });
        if let Some(n) = found {
            self.selected = n;
            self.clicked = None;
        }
    }

    /// The prefix of the group whose header is selected (None elsewhere).
    fn selected_group(&self, keys: &Keymap) -> Option<Vec<Key>> {
        let Some(Field::GroupHeader(n)) = self.selected_field(keys) else { return None };
        super::keymap::keys_tree(keys).into_iter().nth(n)?.prefix
    }

    /// Enter on a group's header: start editing its name; Enter again gives the change for
    /// keys.toml (the name or the icon typed; empty: back to the default).
    pub fn group_edit(&mut self, keys: &Keymap) -> Option<super::keys_file::Change> {
        let prefix = self.selected_group(keys)?;
        let Some(text) = self.editing.take() else {
            self.editing = Some(keys.group(&prefix).map(|g| g.0).unwrap_or_default());
            self.editing_icon = false;
            return None;
        };
        let text = Some(text.trim().to_string()).filter(|t| !t.is_empty());
        let own = keys.own_group(&prefix);
        let (name, icon) = if self.editing_icon { (own.map(|o| o.0.clone()), text) } else { (text, own.and_then(|o| o.1.clone())) };
        let prefix = prefix.iter().map(Key::spec).collect::<Vec<_>>().join(" ");
        Some(super::keys_file::Change::Group { prefix, name, icon })
    }

    /// `i` on a group's header: start editing its icon.
    pub fn edit_icon(&mut self, keys: &Keymap) {
        if let Some(prefix) = self.selected_group(keys) {
            self.editing = Some(keys.group(&prefix).map(|g| g.1).unwrap_or_default());
            self.editing_icon = true;
        }
    }

    /// Change the selected setting: local ones in `ws.settings` (the caller saves them),
    /// Slack-side ones returned for the caller to send. A text field starts editing first.
    pub fn change(&mut self, ws: &mut Workspace, current: Option<&str>, keys: &Keymap) -> Option<SlackChange> {
        let field = self.selected_field(keys)?;
        match field {
            Field::Leader | Field::InsertLeader | Field::Binding(_) => {
                self.recording = Some(Recording::new(Instant::now()));
                None
            }
            // See `group_edit`.
            Field::GroupHeader(_) => None,
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
            // Empty: back to the downloads folder.
            Field::SaveDir => {
                match self.editing.take() {
                    Some(text) => ws.settings.messages.save_dir = text.trim().to_string(),
                    None => self.editing = Some(ws.settings.messages.save_dir.clone()),
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
            // Like ␣an: pause 30 minutes, or resume whatever pauses.
            Field::Notifications => Some(SlackChange::Command(if ws.dnd.active(super::clock::now().timestamp() as f64) { "/dnd off".into() } else { "/dnd 30m".into() })),
            Field::Schedule | Field::ScheduleHours | Field::ScheduleDay(_) => match self.schedule_change(ws, field, None) {
                Some(Ok(edits)) => Some(SlackChange::Schedule(edits)),
                Some(Err(e)) => {
                    self.saved = Some(Err(e));
                    None
                }
                None => None,
            },
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

    /// + / - on the Notifications row: the command for the next longer or shorter pause (30 min, 1 h,
    /// 2 h, 8 h) while a snooze runs; None otherwise (a scheduled period has no length to change).
    pub fn pause_step(ws: &Workspace, up: bool) -> Option<String> {
        let now = super::clock::now().timestamp() as f64;
        let left = ((ws.dnd.snooze_until.filter(|t| *t > now)? - now) / 60.0).round() as u64;
        let next = if up { PAUSES.iter().find(|m| **m > left) } else { PAUSES.iter().rev().find(|m| **m < left) };
        next.map(|m| format!("/dnd {m}m"))
    }

    /// Whether the selected field is kept on Slack or in keys.toml (nothing to save in settings.toml).
    pub fn on_slack(&self, keys: &Keymap) -> bool {
        matches!(self.selected_field(keys), Some(Field::Leader | Field::InsertLeader | Field::GroupHeader(_) | Field::Binding(_) | Field::ConversationLevel | Field::ChannelDefault | Field::Keywords | Field::Vips | Field::Status | Field::Away | Field::Notifications | Field::Schedule | Field::ScheduleHours | Field::ScheduleDay(_)))
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, ws: &Workspace, current: Option<&str>, keys: &Keymap) {
        self.sync(ws);
        let popup = popup_area(area);
        self.area = popup;
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
        self.cats = left;
        use unicode_width::UnicodeWidthStr;
        let tr = super::i18n::tr;
        let (good, dim) = (Style::default().fg(super::theme::p().good), Style::default().fg(super::theme::p().dim));
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let mut rows: Vec<Line> = Vec::new();
        let fields = self.fields(keys);
        // Keys: the bindings by the tree, under the category's line with the filter.
        let keys_shown = self.keys_shown();
        let sections = if keys_shown { super::keymap::keys_tree(keys) } else { Vec::new() };
        let tree_row = |i: usize| sections.iter().flat_map(|s| &s.rows).find(|r| r.info == i);
        let icons = ws.settings.which_key.icons;
        let label_of = |field: &Field| match field {
            Field::Binding(i) if keys_shown => tree_row(*i).map_or_else(String::new, |r| if icons { format!("  {} {}", r.icon, r.label) } else { format!("  {}", r.label) }),
            f => f.label(),
        };
        let list_area = if keys_shown {
            // The filter's key (`/`) and, while it has the focus, a cursor.
            let slash = keys.first_key(super::keymap::Context::Preferences, Action::Filter).unwrap_or_default();
            let mut line = vec![Span::styled(format!(" {}  ", tr("Keys")), bold)];
            match (self.filter.is_empty(), self.filter_focused) {
                (true, false) => line.push(Span::styled(format!("{slash} {}", tr("to filter")), dim)),
                (_, true) => line.extend([Span::raw(format!("{}: ", tr("filter"))), Span::styled(format!("{}▏", self.filter), good), Span::styled(format!("  {}", tr("Enter keep · Esc clear")), dim)]),
                (false, false) => line.extend([Span::raw(format!("{}: ", tr("filter"))), Span::styled(self.filter.clone(), good), Span::styled(format!("  {slash} {}", tr("to change")), dim)]),
            }
            let line = Line::from(line);
            f.render_widget(Paragraph::new(line), Rect { height: right.height.min(1), ..right });
            Rect { y: right.y + 1, height: right.height.saturating_sub(1), ..right }
        } else {
            right
        };
        // The bindings' keys line up after their (short) labels, the settings' values after theirs.
        let width_of = |bindings: bool| (fields.iter().filter(|f| matches!(f, Field::Binding(_)) == bindings && !matches!(f, Field::GroupHeader(_))).map(|f| label_of(f).width()).max().unwrap_or(0) + 2).min(right.width as usize / 2 + 8);
        let (settings_w, bindings_w) = (width_of(false), width_of(true));
        // Keep the selection in view (the Keys list is long).
        let room = list_area.height.saturating_sub(5).max(1) as usize;
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + room {
            self.scroll = self.selected + 1 - room;
        }
        for (n, field) in fields.iter().enumerate().skip(self.scroll).take(room) {
            let selected = n == self.selected;
            let label_w = if matches!(field, Field::Binding(_)) && keys_shown { bindings_w } else { settings_w };
            // Each setting keeps to one row: a long value is cut (from the left while typing, so
            // the cursor stays in view).
            let value_w = (right.width as usize).saturating_sub(label_w + 1);
            let typing = self.editing.is_some() && selected;
            let value = match (&self.editing, &self.recording, selected) {
                (Some(text), _, true) => format!("{text}▏"),
                (_, Some(r), true) if r.keys.is_empty() => tr(RECORD_START).into_owned(),
                (_, Some(r), true) if r.done => format!("{} · {}", keys.label(&r.keys), tr(RECORD_DONE)),
                (_, Some(r), true) => format!("{} …", keys.label(&r.keys)),
                _ => match field {
                    // A group's icon (`i` changes it).
                    Field::GroupHeader(s) => sections.get(*s).and_then(|s| keys.group(s.prefix.as_ref()?)).map(|g| g.1).unwrap_or_default(),
                    Field::Binding(i) if keys_shown => tree_row(*i).map(|r| r.keys.clone()).filter(|k| !k.is_empty()).unwrap_or_else(|| "(unbound)".into()),
                    _ => field.value(ws, current, keys),
                },
            };
            let line = match field {
                Field::GroupHeader(s) => {
                    let title = fit(sections.get(*s).map_or("", |s| s.title.as_str()), (right.width as usize).saturating_sub(2), false);
                    let room = (right.width as usize).saturating_sub(title.width() + 3);
                    Line::from(vec![Span::styled(format!(" {title}  "), bold), Span::styled(fit(&value, room, typing), good)])
                }
                _ => {
                    let value = fit(&value, value_w, typing);
                    let label = fit(&label_of(field), label_w.saturating_sub(1), false);
                    let left = value_w.saturating_sub(value.width() + 2);
                    let mut spans = vec![Span::raw(format!(" {label}{}", " ".repeat(label_w.saturating_sub(label.width())))), Span::styled(value, good)];
                    // A binding's description, dim, where there is room.
                    if let (Field::Binding(i), true, true) = (field, keys_shown, left > 4) {
                        spans.push(Span::styled(format!("  {}", fit(&tr(ACTIONS[*i].description), left, false)), dim));
                    }
                    Line::from(spans)
                }
            };
            rows.push(if selected { line.style(highlight) } else { line });
        }
        let fixed: &[&str] = match self.category_name() {
            "Keys" => &[
                "Enter records keys until a 1 s pause (any key counts, Enter and Esc too). Delete unbinds, r resets.",
                "/ filters (Enter keeps the filter, Esc clears it). On a group: Enter renames it, i changes its icon.",
            ],
            "Availability" => &[
                "Status: \":emoji: text for 30m\" (or today); empty clears.",
                "Notification schedule: the hours you receive notifications, paused outside them; saved to Slack at once. Enter or + / - change the kind, hours are typed as 9:00-17:00 (24-hour).",
            ],
            "Notifications" => &["All but the first are kept on Slack. Keywords: comma-separated."],
            "Audio & video" => &[
                "+ / - (or Enter) change the levels; a huddle you are in follows at once.",
                "Below the speaking threshold the microphone sends silence.",
                "KLACK_AUDIO_INPUT / _OUTPUT, when set, win over the devices.",
            ],
            _ => &[],
        };
        let mut hints: Vec<String> = fixed.iter().map(|h| h.to_string()).collect();
        if let Some(sch) = ws.dnd.schedule.as_ref().filter(|s| self.availability_shown() && s.days == ScheduleDays::Custom) {
            let range = format!("{}-{}", sch.range.0, sch.range.1);
            hints.push(format!("A day: Enter edits its hours (or type all day / none); + / - go hours · all day (no quiet hours) · none (paused all day). Back to hours, a day gets the every-day ones ({range})."));
        }
        let shown = rows.len() as u16;
        let rows_shown = rows.len();
        f.render_widget(Paragraph::new(rows), list_area);
        let list = Rect { height: (room as u16).min(list_area.height), ..list_area };
        (self.list, self.shown) = (list, rows_shown);
        super::scroll::indicator_beside(f, popup, list, self.scroll, room, fields.len(), Style::default().fg(super::theme::p().accent));
        // The hints under a blank row, wrapped at words and indented like the rows.
        let below = Rect { x: right.x + 1, y: list_area.y + shown + 1, width: right.width.saturating_sub(1), height: list_area.height.saturating_sub(shown + 1) };
        if !hints.is_empty() && below.height > 0 {
            f.render_widget(Paragraph::new(hints.into_iter().map(|h| Line::styled(h, dim)).collect::<Vec<_>>()).wrap(Wrap { trim: true }), below);
        }
        if let Some(picker) = self.picker.as_mut() {
            picker.draw(f, area, &ws.settings.appearance.theme);
        }
    }
}

/// " (Asia/Jerusalem)" when the account's zone (the schedule's times are in it) is not klack's.
pub fn zone_note(account: Option<chrono_tz::Tz>, klack: Option<chrono_tz::Tz>, now: i64) -> String {
    let Some(account) = account else { return String::new() };
    let differs = match klack {
        Some(klack) => klack != account,
        // The system's zone has no name here: compared by its offsets now and in half a year.
        None => [now, now + 182 * 86_400].into_iter().any(|t| super::clock::in_zone(Some(account), t).map(|d| *d.offset()) != super::clock::in_zone(None, t).map(|d| *d.offset())),
    };
    if differs { format!(" ({})", account.name()) } else { String::new() }
}

/// `zone_note` for the workspace's account and klack's zone now.
pub fn schedule_zone(ws: &Workspace) -> String {
    zone_note(ws.dnd.zone, super::clock::zone(), super::clock::now().timestamp())
}

/// The schedule in words, its times in `time_format` followed by `zone` (see `zone_note`).
pub fn describe_schedule(schedule: Option<&slack_core::dnd::Schedule>, time_format: &str, zone: &str) -> String {
    let Some(sch) = schedule else { return "not known yet".into() };
    let hours = || format!("receive {}–{}{zone}", clock(&sch.range.0, time_format), clock(&sch.range.1, time_format));
    match sch.days {
        ScheduleDays::Off => "off (notifications at any time)".into(),
        ScheduleDays::EveryDay => format!("every day, {}", hours()),
        ScheduleDays::Weekdays => format!("weekdays, {}", hours()),
        ScheduleDays::Custom => "custom (hours per day, F6)".into(),
        ScheduleDays::Unknown => UNKNOWN_SCHEDULE.into(),
    }
}

/// A schedule time ("9:00") in the clock format.
pub fn clock(t: &str, format: &str) -> String {
    let time = slack_core::dnd::slack_time(t).and_then(|t| {
        let (h, m) = t.split_once(':')?;
        chrono::NaiveTime::from_hms_opt(h.parse().ok()?, m.parse().ok()?, 0)
    });
    time.map_or_else(|| t.to_string(), |time| time.format(format).to_string())
}

/// The preferences window: four fifths of the terminal, at least 84×22 (or all of a smaller
/// one) and at most 120 columns wide, where values are still read along with their labels.
fn popup_area(area: Rect) -> Rect {
    let width = (area.width * 4 / 5).clamp(84, 120).min(area.width);
    let height = (area.height * 4 / 5).max(22).min(area.height);
    let [popup] = Layout::horizontal([Constraint::Length(width)]).flex(Flex::Center).areas(area);
    let [popup] = Layout::vertical([Constraint::Length(height)]).flex(Flex::Center).areas(popup);
    popup
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
    fn the_save_folder() {
        let mut ws = Workspace::default();
        let mut p = Prefs::default();
        p.category = CATEGORIES.iter().position(|(name, _)| *name == "Messages & media").unwrap();
        p.selected = CATEGORIES[p.category].1.iter().position(|f| *f == Field::SaveDir).unwrap();
        let keys = Keymap::load("").0;
        assert!(Field::SaveDir.value(&ws, None, &keys).ends_with("(your downloads folder)"));
        assert_eq!(p.change(&mut ws, None, &Keymap::default()), None, "Enter starts editing");
        assert_eq!(p.editing.as_deref(), Some(""));
        p.editing = Some("  ~/Work/slack  ".into());
        p.change(&mut ws, None, &Keymap::default());
        assert_eq!((ws.settings.messages.save_dir.as_str(), p.on_slack(&Keymap::default())), ("~/Work/slack", false), "kept locally, trimmed");
        assert_eq!(Field::SaveDir.value(&ws, None, &keys), "~/Work/slack");
        if std::env::var_os("KLACK_DOWNLOAD_DIR").is_none() {
            let home = directories::UserDirs::new().unwrap().home_dir().to_path_buf();
            assert_eq!(super::super::files::download_dir(&ws.settings.messages.save_dir), home.join("Work/slack"));
            assert_ne!(super::super::files::download_dir(""), home.join("Work/slack"), "empty: the downloads folder");
        }
    }

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
        p.selected = p.fields(&Keymap::default()).iter().position(|f| *f == Field::Camera).unwrap();
        let steps: Vec<Option<String>> = (0..3)
            .map(|_| {
                p.change(&mut ws, None, &Keymap::default());
                ws.settings.video.camera.clone()
            })
            .collect();
        assert_eq!(steps, vec![Some("cam A".into()), Some("cam B".into()), None], "none → each camera → none");
        assert!(!p.on_slack(&Keymap::default()));
    }

    #[test]
    fn slack_side_changes() {
        let mut ws = Workspace::default();
        let mut p = Prefs { category: 4, ..Default::default() };
        p.selected = 2;
        assert_eq!(p.change(&mut ws, Some("C1"), &Keymap::default()), Some(SlackChange::ConversationLevel { channel: "C1".into(), level: "everything".into() }), "default → everything");
        let mut cycle = Vec::new();
        for _ in 0..4 {
            let Some(SlackChange::ConversationLevel { level, .. }) = p.change(&mut ws, Some("C1"), &Keymap::default()) else { panic!() };
            if level == "default" { ws.notify.channels.remove("C1") } else { ws.notify.channels.insert("C1".into(), level.clone()) };
            cycle.push(level);
        }
        assert_eq!(cycle, ["everything", "mentions_dms", "nothing", "default"], "back to the default after nothing");
        assert_eq!(p.change(&mut ws, None, &Keymap::default()), None, "no conversation, no change");
        p.selected = 3;
        assert_eq!(p.change(&mut ws, None, &Keymap::default()), Some(SlackChange::ChannelDefault("nothing".into())));
        p.selected = 4;
        ws.notify.keywords = vec!["a".into()];
        assert_eq!(p.change(&mut ws, None, &Keymap::default()), None, "Enter starts editing");
        assert_eq!(p.editing.as_deref(), Some("a"));
        p.editing = Some(" deploy ,, release notes ".into());
        assert_eq!(p.change(&mut ws, None, &Keymap::default()), Some(SlackChange::Keywords("deploy,release notes".into())));
        assert!(p.editing.is_none() && p.on_slack(&Keymap::default()));
    }

    #[test]
    fn the_notification_schedule() {
        use slack_core::dnd::{DayHours, Schedule, ScheduleDays::*, ScheduleEdit::*};
        let keys = Keymap::default();
        let mut ws = Workspace::default();
        ws.settings.region.clock_24h = true;
        let mut p = Prefs::default();
        let rows = |p: &mut Prefs, ws: &Workspace| {
            p.sync(ws);
            p.fields(&Keymap::default())
        };
        let value = |f: Field, ws: &Workspace| f.value(ws, None, &Keymap::default());
        let sent = |edits| Some(SlackChange::Schedule(edits));
        // Not known yet (no prefs): said so, and nothing is sent.
        assert_eq!(rows(&mut p, &ws), [Field::Status, Field::Away, Field::Notifications, Field::Schedule]);
        assert_eq!(value(Field::Schedule, &ws), "not known yet");
        p.selected = 3;
        assert!(p.on_slack(&keys));
        assert_eq!(p.change(&mut ws, None, &keys), None);
        assert!(matches!(p.saved.take(), Some(Err(_))));
        // Weekdays, notifications 9:00–17:00 (as on the test account).
        let prefs = serde_json::json!({"dnd_enabled": true, "dnd_days": "weekdays", "dnd_end_hour": "9:00", "dnd_start_hour": "17:00", "dnd_enabled_monday": "partial", "dnd_before_monday": "8:00", "dnd_after_monday": "22:00", "dnd_enabled_saturday": "all_day", "dnd_enabled_sunday": "disabled"});
        ws.dnd.schedule = Some(Schedule::from_prefs(&prefs));
        assert_eq!(rows(&mut p, &ws), [Field::Status, Field::Away, Field::Notifications, Field::Schedule, Field::ScheduleHours]);
        assert_eq!((value(Field::Schedule, &ws).as_str(), value(Field::ScheduleHours, &ws).as_str()), ("weekdays", "from 09:00 to 17:00"));
        ws.settings.region.clock_24h = false;
        assert_eq!(value(Field::ScheduleHours, &ws), "from 9:00 AM to 5:00 PM", "in the clock format");
        ws.settings.region.clock_24h = true;
        // The kind: Enter (and +) the next, - the one before.
        assert_eq!(p.change(&mut ws, None, &keys), sent(vec![Days(Custom)]));
        assert_eq!(p.schedule_change(&ws, Field::Schedule, Some(false)), Some(Ok(vec![Days(EveryDay)])));
        // The hours: Enter edits them as typed (24-hour), Enter again sends them; bad ones are refused.
        p.selected = 4;
        assert!(p.on_slack(&keys));
        assert_eq!((p.change(&mut ws, None, &keys), p.editing.as_deref()), (None, Some("9:00-17:00")));
        p.editing = Some("10:00 - 18:00".into());
        assert_eq!(p.change(&mut ws, None, &keys), sent(vec![Range("10:00".into(), "18:00".into())]));
        p.editing = Some("10-18".into());
        assert_eq!(p.change(&mut ws, None, &keys), None);
        assert!(matches!(p.saved.take(), Some(Err(e)) if e.contains("time of day")));
        assert_eq!(p.schedule_change(&ws, Field::ScheduleHours, Some(true)), None, "+ / - don't change hours");
        // Off: turned on again as every day (Enter), or another kind with + / -.
        ws.dnd.schedule.as_mut().unwrap().days = Off;
        assert_eq!(rows(&mut p, &ws).len(), 4);
        p.selected = 3;
        assert_eq!(value(Field::Schedule, &ws), "off");
        assert_eq!(p.change(&mut ws, None, &keys), sent(vec![Days(EveryDay)]));
        assert_eq!(p.schedule_change(&ws, Field::Schedule, Some(false)), Some(Ok(vec![Days(Custom)])));
        // Custom: a row per day, Monday first: hours, "all day" (no quiet hours) or "none" (paused all day).
        ws.dnd.schedule.as_mut().unwrap().days = Custom;
        assert_eq!(rows(&mut p, &ws)[4..], (0..7).map(Field::ScheduleDay).collect::<Vec<_>>());
        assert_eq!(Field::ScheduleDay(0).label(), "Monday");
        assert_eq!([0, 5, 6].map(|d| value(Field::ScheduleDay(d), &ws)), ["08:00 to 22:00", "none", "all day"]);
        // Enter on a day with hours edits them; "all day" / "none" may be typed instead.
        p.selected = 4;
        assert_eq!((p.change(&mut ws, None, &keys), p.editing.as_deref()), (None, Some("8:00-22:00")));
        p.editing = Some("None".into());
        assert_eq!(p.change(&mut ws, None, &keys), sent(vec![Day(0, DayHours::Never)]));
        p.editing = Some("all day".into());
        assert_eq!(p.change(&mut ws, None, &keys), sent(vec![Day(0, DayHours::AllDay)]));
        p.editing = Some("7:00-9:00".into());
        assert_eq!(p.change(&mut ws, None, &keys), sent(vec![Day(0, DayHours::Between("7:00".into(), "9:00".into()))]));
        // Enter on "none" / "all day" steps on (hours → all day → none → hours); back to hours, a
        // day gets the every-day hours.
        p.selected = 9;
        assert_eq!(p.change(&mut ws, None, &keys), sent(vec![Day(5, DayHours::Between("9:00".into(), "17:00".into()))]));
        p.selected = 10;
        assert_eq!(p.change(&mut ws, None, &keys), sent(vec![Day(6, DayHours::Never)]));
        assert_eq!(p.schedule_change(&ws, Field::ScheduleDay(0), Some(true)), Some(Ok(vec![Day(0, DayHours::AllDay)])));
        assert_eq!(p.schedule_change(&ws, Field::ScheduleDay(0), Some(false)), Some(Ok(vec![Day(0, DayHours::Never)])));
        assert_eq!(p.schedule_change(&ws, Field::ScheduleDay(6), Some(false)), Some(Ok(vec![Day(6, DayHours::Between("9:00".into(), "17:00".into()))])));
        // A day klack doesn't know: shown, not changed.
        ws.dnd.schedule.as_mut().unwrap().per_day[1] = DayHours::Unknown("sometimes".into());
        assert!(value(Field::ScheduleDay(1), &ws).contains("change it in Slack"));
        p.selected = 5;
        assert_eq!(p.change(&mut ws, None, &keys), None);
        assert!(matches!(p.saved.take(), Some(Err(_))) && p.editing.is_none());
        assert!(matches!(p.schedule_change(&ws, Field::ScheduleDay(1), Some(true)), Some(Err(_))));
        // A schedule klack doesn't know: shown, no rows to change, nothing sent.
        ws.dnd.schedule = Some(Schedule::from_prefs(&serde_json::json!({"dnd_enabled": true, "dnd_days": "weekends"})));
        assert_eq!(rows(&mut p, &ws).len(), 4);
        assert_eq!(p.selected, 3, "the selection stays on a row shown");
        assert!(value(Field::Schedule, &ws).contains("change it in Slack"));
        assert_eq!(p.change(&mut ws, None, &keys), None);
        assert!(matches!(p.saved.take(), Some(Err(_))));
        // Slack's "weekdays off all day" option changes what weekdays means: said.
        ws.dnd.schedule = Some(Schedule::from_prefs(&serde_json::json!({"dnd_enabled": true, "dnd_days": "weekdays", "dnd_weekdays_off_allday": true})));
        assert!(value(Field::Schedule, &ws).starts_with("weekdays ("), "{}", value(Field::Schedule, &ws));
        // The times are the account's zone: named when it isn't klack's.
        let (jerusalem, kyiv): (chrono_tz::Tz, chrono_tz::Tz) = ("Asia/Jerusalem".parse().unwrap(), "Europe/Kyiv".parse().unwrap());
        assert_eq!(zone_note(Some(jerusalem), Some(kyiv), 1_800_000_000), " (Asia/Jerusalem)");
        assert_eq!(zone_note(Some(jerusalem), Some(jerusalem), 1_800_000_000), "");
        assert_eq!(zone_note(None, Some(kyiv), 1_800_000_000), "", "the account's zone not known: klack's");
    }

    #[test]
    fn typing_stops_when_its_row_goes() {
        use slack_core::dnd::{Schedule, ScheduleDays};
        let keys = Keymap::default();
        let mut ws = Workspace::default();
        ws.dnd.schedule = Some(Schedule::from_prefs(&serde_json::json!({"dnd_enabled": true, "dnd_days": "weekdays", "dnd_end_hour": "9:00", "dnd_start_hour": "17:00"})));
        let mut p = Prefs::default();
        p.sync(&ws);
        p.selected = 4;
        assert_eq!((p.change(&mut ws, None, &keys), p.selected_field(&keys)), (None, Some(Field::ScheduleHours)));
        p.editing = Some("10:00-18:00".into());
        // Weekdays → every day elsewhere: the same row, the typing goes on.
        ws.dnd.schedule.as_mut().unwrap().days = ScheduleDays::EveryDay;
        p.sync(&ws);
        assert_eq!(p.editing.as_deref(), Some("10:00-18:00"));
        // → custom: row 4 is now Monday; the hours typed must not become Monday's.
        ws.dnd.schedule.as_mut().unwrap().days = ScheduleDays::Custom;
        p.sync(&ws);
        assert_eq!((p.editing.as_deref(), p.selected_field(&keys)), (None, Some(Field::ScheduleDay(0))));
        assert!(matches!(&p.saved, Some(Err(e)) if e.contains("changed")), "{:?}", p.saved);
        // The row gone altogether (off): the same.
        ws.dnd.schedule.as_mut().unwrap().days = ScheduleDays::Weekdays;
        p.sync(&ws);
        p.change(&mut ws, None, &keys);
        assert!(p.editing.is_some());
        ws.dnd.schedule.as_mut().unwrap().days = ScheduleDays::Off;
        p.sync(&ws);
        assert_eq!((p.editing.as_deref(), p.selected), (None, 3));
        // Typing elsewhere (the status) is not touched.
        p.selected = 0;
        p.change(&mut ws, None, &keys);
        assert!(p.editing.is_some());
        ws.dnd.schedule.as_mut().unwrap().days = ScheduleDays::Custom;
        p.sync(&ws);
        assert!(p.editing.is_some());
    }

    #[test]
    fn availability_becomes_commands() {
        let mut ws = Workspace { self_id: "UME".into(), ..Default::default() };
        let mut p = Prefs::default();
        assert_eq!(CATEGORIES[0].0, "Availability");
        p.selected = 1;
        assert_eq!(p.change(&mut ws, None, &Keymap::default()), Some(SlackChange::Command("/away".into())));
        ws.presence.insert("UME".into(), false);
        assert_eq!(p.change(&mut ws, None, &Keymap::default()), Some(SlackChange::Command("/active".into())));
        p.selected = 2;
        let now = crate::tui::clock::now().timestamp() as f64;
        assert_eq!(p.change(&mut ws, None, &Keymap::default()), Some(SlackChange::Command("/dnd 30m".into())), "on → pause 30 minutes");
        ws.dnd.snooze_until = Some(now + 29.6 * 60.0);
        assert_eq!(p.change(&mut ws, None, &Keymap::default()), Some(SlackChange::Command("/dnd off".into())), "paused → resume");
        ws.dnd.snooze_until = None;
        ws.dnd.window = Some((now - 600.0, now + 600.0));
        assert_eq!(p.change(&mut ws, None, &Keymap::default()), Some(SlackChange::Command("/dnd off".into())), "a scheduled period is resumed the same way");
        // + / - step the pause length while snoozing.
        let step = |ws: &mut Workspace, minutes: f64, up| {
            ws.dnd = crate::tui::state::Dnd { snooze_until: Some(now + minutes * 60.0), ..Default::default() };
            Prefs::pause_step(ws, up)
        };
        assert_eq!(step(&mut ws, 29.6, true), Some("/dnd 60m".into()));
        assert_eq!(step(&mut ws, 60.0, true), Some("/dnd 120m".into()));
        assert_eq!(step(&mut ws, 100.0, true), Some("/dnd 120m".into()), "the next length above");
        assert_eq!(step(&mut ws, 120.0, false), Some("/dnd 60m".into()));
        assert_eq!(step(&mut ws, 480.0, true), None, "the longest stays");
        assert_eq!(step(&mut ws, 30.0, false), None, "the shortest stays: - never resumes");
        ws.dnd = crate::tui::state::Dnd { window: Some((now - 600.0, now + 600.0)), ..Default::default() };
        assert_eq!(Prefs::pause_step(&ws, true), None, "a scheduled period has no length to step");
        ws.dnd = Default::default();
        assert_eq!(Prefs::pause_step(&ws, true), None, "not paused: nothing to step");
        // The value reads as on / paused until.
        let mut s = ws.settings.clone();
        s.region.clock_24h = true;
        ws.settings = s;
        assert_eq!(Field::Notifications.value(&ws, None, &Keymap::default()), "on");
        ws.dnd.snooze_until = Some(now + 600.0);
        assert!(Field::Notifications.value(&ws, None, &Keymap::default()).starts_with("paused until "));
        p.selected = 0;
        assert_eq!(p.change(&mut ws, None, &Keymap::default()), None, "Enter edits the status");
        p.editing = Some(":coffee: break for 30m".into());
        assert_eq!(p.change(&mut ws, None, &Keymap::default()), Some(SlackChange::Command("/status :coffee: break for 30m".into())));
        p.editing = Some("  ".into());
        assert_eq!(p.change(&mut ws, None, &Keymap::default()), Some(SlackChange::Command("/status clear".into())));
    }

    #[test]
    fn clicks_map_to_categories_and_rows() {
        // Categories in the column at x 10..32 from y 5; settings rows from y 5 (Keys: from y 6).
        let mut p = Prefs { cats: Rect::new(10, 5, 22, 20), list: Rect::new(32, 5, 60, 15), shown: 4, scroll: 3, ..Default::default() };
        assert_eq!((p.category_at(11, 5), p.category_at(11, 7)), (Some(0), Some(2)));
        assert_eq!(p.category_at(11, 5 + CATEGORIES.len() as u16), Some(CATEGORIES.len()), "Keys, the last");
        assert_eq!(p.category_at(11, 6 + CATEGORIES.len() as u16), None, "below the last");
        assert_eq!((p.category_at(11, 4), p.category_at(9, 5), p.category_at(32, 5)), (None, None, None), "border, side, other column");
        assert_eq!((p.row_at(33, 5), p.row_at(33, 8)), (Some(3), Some(6)), "the scroll offset counts");
        assert_eq!(p.row_at(33, 9), None, "below the last shown row");
        assert_eq!((p.row_at(33, 4), p.row_at(31, 5)), (None, None), "above the list, in the category column");
        // Keys: the filter line above the list; a group's header is a row of its own.
        p.category = CATEGORIES.len();
        p.list = Rect::new(32, 6, 60, 14);
        p.scroll = 0;
        p.shown = 3;
        let keys = Keymap::default();
        let header = p.fields(&keys).iter().position(|f| matches!(f, Field::GroupHeader(_))).unwrap();
        p.shown = header + 1;
        assert_eq!(p.row_at(40, 5), None, "the filter line");
        assert_eq!(p.row_at(40, 6), Some(0));
        let n = p.row_at(40, 6 + header as u16).unwrap();
        assert!(matches!(p.fields(&keys)[n], Field::GroupHeader(_)), "a header maps to its field");
    }

    #[test]
    fn a_first_click_only_selects() {
        let keys = Keymap::default();
        let mut p = Prefs { cats: Rect::new(10, 5, 22, 20), list: Rect::new(32, 5, 60, 15), shown: 10, ..Default::default() };
        // A category, then its first row: selected, not changed; the second click changes it.
        let n = p.category_at(11, 6).unwrap();
        p.switch_category(n as isize - p.category as isize);
        assert!(!p.click(p.row_at(33, 5).unwrap()), "the first click on row 0");
        assert!(p.click(0), "the second");
        assert!(!p.click(0), "acting forgets the click");
        assert!(p.click(0));
        // Right after F6 opens, row 0 is selected but not by a click.
        assert!(!Prefs::default().click(0));
        // Another row, then keys, a filter or another category in between.
        assert!(!p.click(2) && p.selected == 2);
        p.move_by(1, &keys);
        p.move_by(-1, &keys);
        assert!(!p.click(2), "keys moved the selection");
        p.click(0);
        p.switch_category(0);
        assert!(!p.click(0), "the category was switched");
        p.category = CATEGORIES.len();
        p.click(0);
        p.filter_key(Key::parse("m").unwrap(), None);
        assert!(!p.click(0), "the filter changed");
    }

    #[test]
    fn categories_cycle() {
        let mut p = Prefs::default();
        p.switch_category(-1);
        assert_eq!((p.category, p.category_name()), (CATEGORIES.len(), "Keys"));
        let keys = Keymap::load("").0;
        assert!(p.keys_shown() && p.fields(&keys).len() > ACTIONS.len() / 2);
        p.switch_category(-1);
        p.move_by(10, &keys);
        assert_eq!(p.selected, p.fields(&keys).len() - 1);
    }

    fn keys_prefs() -> Prefs {
        Prefs { category: CATEGORIES.len(), ..Default::default() }
    }

    #[test]
    fn keys_category_follows_the_tree() {
        let keys = Keymap::load("").0;
        let p = keys_prefs();
        assert_eq!(p.category_name(), "Keys");
        let mut want = vec![Field::WhichKeyShow, Field::WhichKeyCorner, Field::WhichKeyDelay, Field::WhichKeyHeight, Field::WhichKeyIcons, Field::Leader, Field::InsertLeader];
        for (n, section) in super::super::keymap::keys_tree(&keys).iter().enumerate() {
            want.push(Field::GroupHeader(n));
            want.extend(section.rows.iter().map(|r| Field::Binding(r.info)));
        }
        assert_eq!(p.fields(&keys), want);
        // Each workspace is its own row, so 2…9 can be rebound here too.
        for n in 1..=9 {
            let i = ACTIONS.iter().position(|a| a.name == format!("workspace_{n}")).unwrap();
            assert!(want.contains(&Field::Binding(i)), "workspace_{n}");
        }
    }

    #[test]
    fn filter_narrows_bindings() {
        let keys = Keymap::load("").0;
        let sections = super::super::keymap::keys_tree(&keys);
        let index = |context: super::super::keymap::Context, name: &str| ACTIONS.iter().position(|a| a.context == context && a.name == name).unwrap();
        let mut p = keys_prefs();
        p.filter = "huddle".into();
        let fields = p.fields(&keys);
        assert!(fields.contains(&Field::Binding(index(super::super::keymap::Context::Global, "mute"))), "{fields:?}");
        assert!(!fields.contains(&Field::Binding(index(super::super::keymap::Context::Global, "quit"))));
        for (n, field) in fields.iter().enumerate() {
            match field {
                Field::Binding(i) => {
                    let row = sections.iter().flat_map(|s| &s.rows).find(|r| r.info == *i).unwrap();
                    let text = format!("{} {} {}", row.label, ACTIONS[*i].description, row.keys).to_lowercase();
                    assert!(text.contains("huddle"), "{text}");
                }
                Field::GroupHeader(_) => assert!(matches!(fields.get(n + 1), Some(Field::Binding(_))), "an empty section's header is hidden: {fields:?}"),
                other => panic!("{other:?} does not mention huddle"),
            }
        }
        p.filter = "CTRL+Q".into();
        assert!(p.fields(&keys).contains(&Field::Binding(index(super::super::keymap::Context::Global, "quit"))), "keys match, in any case");
        p.filter = "leader".into();
        assert!(p.fields(&keys).starts_with(&[Field::Leader, Field::InsertLeader]), "settings match their labels");
        p.filter = "nothing matches this".into();
        assert!(p.fields(&keys).is_empty());
        p.move_by(1, &keys);
        assert_eq!((p.selected, p.selected_field(&keys)), (0, None), "an empty list does not panic");
    }

    #[test]
    fn recording_ends_after_a_pause() {
        use std::time::{Duration, Instant};
        let k = |s: &str| super::super::keymap::Key::parse(s).unwrap();
        let now = Instant::now();
        let mut r = Recording { keys: vec![k("space"), k("h"), k("m")], last: now - Duration::from_millis(1100), done: false };
        r.tick(now);
        assert!(r.done, "a pause of {RECORD_PAUSE:?} ends it");
        let mut r = Recording::new(now);
        r.tick(now + Duration::from_secs(5));
        assert!(!r.done && r.due().is_none(), "nothing pressed yet: it waits");
        assert_eq!(r.press(k("esc"), now), None, "Esc is recorded");
        assert_eq!(r.press(k("enter"), now + Duration::from_millis(500)), None, "Enter is recorded");
        assert_eq!(r.keys, [k("esc"), k("enter")]);
        assert_eq!(r.due(), Some(now + Duration::from_millis(500) + RECORD_PAUSE));
        r.tick(now + Duration::from_millis(1400));
        assert!(!r.done, "not a second since the last key");
        assert_eq!(r.press(k("x"), now + Duration::from_millis(1600)), None, "after the pause a key only decides");
        assert!(r.done);
        assert_eq!(r.keys, [k("esc"), k("enter")]);
        assert_eq!(r.press(k("enter"), now + Duration::from_secs(2)), Some(true), "Enter keeps");
        let mut r = Recording { keys: vec![k("a")], last: now, done: true };
        assert_eq!(r.press(k("esc"), now), Some(false), "Esc discards");
    }

    #[test]
    fn a_lone_leader_is_the_key_itself() {
        let keys = Keymap::load("").0;
        let k = |s: &str| super::super::keymap::Key::parse(s).unwrap();
        assert_eq!(recorded(&keys, &[k("space")]), "space");
        assert_eq!(recorded(&keys, &[k("space"), k("h"), k("m")]), "<leader> h m");
        assert_eq!(recorded(&keys, &[k("ctrl+space"), k("x")]), "<leader> x", "either leader");
        assert_eq!(recorded(&keys, &[k("g"), k("space"), k("enter"), k("esc")]), "g space enter esc", "only at the start");
        let (map, warnings) = Keymap::load(&format!("[messages]\nreact = \"{}\"\n", recorded(&keys, &[k("space"), k("x")])));
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(map.sequences(super::super::keymap::Context::Messages, "react"), vec![vec![super::super::keymap::LEADER, k("x")]]);
    }

    #[test]
    fn renaming_a_group() {
        use super::super::keys_file::Change;
        let keys = Keymap::load("").0;
        let sections = super::super::keymap::keys_tree(&keys);
        let huddle = sections.iter().position(|s| s.title.ends_with("huddle")).unwrap();
        let mut p = keys_prefs();
        p.selected = p.fields(&keys).iter().position(|f| *f == Field::GroupHeader(huddle)).unwrap();
        assert!(p.on_slack(&keys), "kept in keys.toml, not settings.toml");
        assert_eq!(p.group_edit(&keys), None, "Enter starts editing the name");
        assert_eq!(p.editing.as_deref(), Some("huddle"));
        p.editing = Some(" calls ".into());
        assert_eq!(p.group_edit(&keys), Some(Change::Group { prefix: "<leader> h".into(), name: Some("calls".into()), icon: None }));
        assert!(p.editing.is_none());
        p.edit_icon(&keys);
        assert_eq!(p.editing.as_deref(), Some("🎧"));
        p.editing = Some("📞".into());
        assert_eq!(p.group_edit(&keys), Some(Change::Group { prefix: "<leader> h".into(), name: None, icon: Some("📞".into()) }));
        let own = Keymap::load("[groups]\n\"<leader> h\" = { name = \"calls\", icon = \"📞\" }\n").0;
        p.edit_icon(&own);
        p.editing = Some(String::new());
        assert_eq!(p.group_edit(&own), Some(Change::Group { prefix: "<leader> h".into(), name: Some("calls".into()), icon: None }), "an empty icon is the default again; the name stays");
        p.selected = 7;
        assert!(matches!(p.selected_field(&keys), Some(Field::GroupHeader(0))));
        assert_eq!((p.group_edit(&keys), p.editing.as_ref()), (None, None), "Direct keys is not a group: nothing to rename");
        p.edit_icon(&keys);
        assert!(p.editing.is_none());
    }

    #[test]
    fn the_filter_takes_every_key_after_slash() {
        use super::super::keymap::{Context, Mode, Resolved};
        let keys = Keymap::load("").0;
        let k = |s: &str| Key::parse(s).unwrap();
        assert_eq!(keys.resolve(&[Context::Preferences], Mode::Dialog, &[], k("/")), Resolved::Action(Context::Preferences, Action::Filter), "/ is the filter, rebindable");
        // As the event loop does: the text field's keys, else the key itself.
        let press = |p: &mut Prefs, s: &str| {
            let action = match keys.resolve(&[Context::TextField], Mode::Dialog, &[], k(s)) {
                Resolved::Action(_, a) => Some(a),
                _ => None,
            };
            p.filter_key(k(s), action);
        };
        let mut p = keys_prefs();
        p.filter_focused = true;
        for key in ["h", "u", "d", "space", "q", "r", "i", "j", "k", "l"] {
            press(&mut p, key);
        }
        assert_eq!(p.filter, "hud qrijkl", "Space and the list's own keys type too");
        press(&mut p, "backspace");
        assert_eq!(p.filter, "hud qrijk");
        p.selected = 3;
        press(&mut p, "ctrl+x");
        assert_eq!((p.filter.as_str(), p.selected), ("hud qrijk", 3), "Ctrl keys do not type");
        press(&mut p, "x");
        assert_eq!(p.selected, 0, "a new filter: back to the top");
        press(&mut p, "enter");
        assert_eq!((p.filter.as_str(), p.filter_focused), ("hud qrijkx", false), "Enter keeps it and goes back to the list");
        p.filter_focused = true;
        press(&mut p, "esc");
        assert_eq!((p.filter.as_str(), p.filter_focused), ("", false), "Esc clears it");
        p.filter = "mute".into();
        p.filter_focused = true;
        p.switch_category(1);
        assert_eq!((p.filter.as_str(), p.filter_focused), ("", false), "another category drops the filter");
    }

    #[test]
    fn the_selection_follows_the_edit() {
        use super::super::keymap::{Context, LEADER};
        use super::super::keys_file::{apply, Change};
        let keys = Keymap::load("").0;
        let mute = ACTIONS.iter().position(|a| a.context == Context::Global && a.name == "mute").unwrap();
        let mut p = keys_prefs();
        p.selected = p.fields(&keys).iter().position(|f| *f == Field::Binding(mute)).unwrap();
        let anchor = p.anchor(&keys);
        let after = Keymap::load(&apply("", &Change::Binding { context: Context::Global, name: "mute", seqs: Some(vec![]) }).unwrap()).0;
        assert_ne!(p.selected_field(&after), Some(Field::Binding(mute)), "the rows moved: the same place is another action");
        p.reselect(anchor, &after);
        assert_eq!(p.selected_field(&after), Some(Field::Binding(mute)), "still the action unbound");
        let fields = p.fields(&after);
        let header = fields[..p.selected].iter().rposition(|f| matches!(f, Field::GroupHeader(_))).unwrap();
        let Field::GroupHeader(n) = fields[header] else { unreachable!() };
        assert_eq!(super::super::keymap::keys_tree(&after)[n].title, "Unbound", "now under Unbound");
        // A group's header, by its prefix (its title changes with the name).
        let huddle = super::super::keymap::keys_tree(&keys).iter().position(|s| s.title.ends_with("huddle")).unwrap();
        p.selected = p.fields(&keys).iter().position(|f| *f == Field::GroupHeader(huddle)).unwrap();
        let anchor = p.anchor(&keys);
        let renamed = Keymap::load(&apply("[global]\nquit = []\nhelp = []\n", &Change::Group { prefix: "<leader> h".into(), name: Some("calls".into()), icon: None }).unwrap()).0;
        p.reselect(anchor, &renamed);
        let Some(Field::GroupHeader(n)) = p.selected_field(&renamed) else { panic!("{:?}", p.selected_field(&renamed)) };
        let section = &super::super::keymap::keys_tree(&renamed)[n];
        assert_eq!((section.prefix.clone(), section.title.ends_with("calls")), (Some(vec![LEADER, Key::parse("h").unwrap()]), true));
    }

    #[test]
    fn which_key_settings() {
        let mut s = Settings::default();
        let corners: Vec<String> = (0..4)
            .map(|_| {
                Field::WhichKeyCorner.change(&mut s, &Default::default());
                s.which_key.corner.clone()
            })
            .collect();
        assert_eq!(corners, ["bottom-left", "top-left", "top-right", "bottom-right"]);
        Field::WhichKeyShow.change(&mut s, &Default::default());
        Field::WhichKeyIcons.change(&mut s, &Default::default());
        assert!(!s.which_key.show && !s.which_key.icons);
        assert!(Field::WhichKeyDelay.adjust(&mut s, true, false));
        assert_eq!(s.which_key.delay_secs, 0.5);
        for _ in 0..10 {
            Field::WhichKeyDelay.adjust(&mut s, false, false);
        }
        assert_eq!(s.which_key.delay_secs, 0.0, "stops at 0 (at once)");
        let ws = Workspace::default();
        assert_eq!(Field::WhichKeyDelay.value(&ws, None, &Keymap::default()), "0.4 s");
        s.which_key.delay_secs = 5.0;
        Field::WhichKeyDelay.adjust(&mut s, true, false);
        assert_eq!(s.which_key.delay_secs, 5.0, "stops at 5 s");
        Field::WhichKeyDelay.change(&mut s, &Default::default());
        assert_eq!(s.which_key.delay_secs, 0.0, "Enter wraps around");
        Field::WhichKeyHeight.adjust(&mut s, true, false);
        assert_eq!(s.which_key.max_height_percent, 60);
        s.which_key.max_height_percent = 100;
        Field::WhichKeyHeight.adjust(&mut s, true, false);
        assert_eq!(s.which_key.max_height_percent, 100);
        Field::WhichKeyHeight.change(&mut s, &Default::default());
        assert_eq!(s.which_key.max_height_percent, 10, "Enter wraps to the lowest");
        Field::WhichKeyHeight.adjust(&mut s, false, false);
        assert_eq!(s.which_key.max_height_percent, 10);
        assert_eq!(Field::WhichKeyHeight.value(&ws, None, &Keymap::default()), "50% of the screen");
        assert!(translatable().iter().any(|t| t == Field::WhichKeyCorner.fixed_label()));
    }

    #[test]
    fn the_window_grows_with_the_terminal() {
        let size = |w, h| {
            let r = super::popup_area(Rect::new(0, 0, w, h));
            (r.width, r.height)
        };
        assert_eq!(size(80, 20), (80, 20), "a small terminal: all of it");
        assert_eq!(size(100, 30), (84, 24), "at least the old size");
        assert_eq!(size(140, 50), (112, 40));
        assert_eq!(size(300, 80), (120, 64), "not wider than 120 columns");
        let r = super::popup_area(Rect::new(0, 0, 300, 80));
        assert_eq!((r.x, r.y), (90, 8), "centred");
    }

    #[test]
    fn workspace_themes() {
        let mut ws = Workspace { team_id: "T1".into(), ..Default::default() };
        let p = Prefs { category: 1, selected: 1, ..Default::default() };
        assert_eq!(p.selected_field(&Keymap::default()), Some(Field::WorkspaceTheme));
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

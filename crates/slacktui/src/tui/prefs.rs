//! Preferences (F6): local settings by category; a change applies at once and is saved
//! to settings.toml.

use chrono::Local;
use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use super::settings::Settings;
use super::state::{LEVELS, Workspace, level_label};
use super::text::DIM;

/// Day-separator formats offered (strftime).
pub const DATE_FORMATS: &[&str] = &["%A, %B %-d", "%a %-d %b", "%d.%m.%Y", "%Y-%m-%d", "%m/%d/%Y"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Compact,
    InlineImages,
    Avatars,
    EmojiAsNames,
    Clock24,
    DateFormat,
    Desktop,
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
}

/// Pause lengths offered, in minutes (0: not paused).
pub const PAUSES: &[u64] = &[0, 30, 60, 120, 480];

pub const CATEGORIES: &[(&str, &[Field])] = &[
    ("Availability", &[Field::Status, Field::Away, Field::Pause]),
    ("Appearance", &[Field::Compact]),
    ("Messages & media", &[Field::InlineImages, Field::Avatars, Field::EmojiAsNames]),
    ("Language & region", &[Field::Clock24, Field::DateFormat]),
    ("Notifications", &[Field::Desktop, Field::ConversationLevel, Field::ChannelDefault, Field::Keywords, Field::Vips]),
    ("Audio", &[Field::Input, Field::Output]),
];

impl Field {
    pub fn label(self) -> &'static str {
        match self {
            Field::Compact => "Compact list (no gap between authors)",
            Field::InlineImages => "Show image thumbnails",
            Field::Avatars => "Show profile pictures",
            Field::EmojiAsNames => "Show emoji as :names:",
            Field::Clock24 => "24-hour clock",
            Field::DateFormat => "Date format",
            Field::Desktop => "Desktop notifications",
            Field::ConversationLevel => "This conversation notifies about",
            Field::ChannelDefault => "Channels notify about (default)",
            Field::Keywords => "Keywords",
            Field::Vips => "VIPs (v on a profile card)",
            Field::Status => "Status",
            Field::Away => "Appear away",
            Field::Pause => "Pause notifications",
            Field::Input => "Microphone",
            Field::Output => "Speaker",
        }
    }

    pub fn value(self, ws: &Workspace, current: Option<&str>) -> String {
        let s = &ws.settings;
        let yes = |b: bool| if b { "on" } else { "off" }.to_string();
        let device = |d: &Option<String>| d.clone().unwrap_or_else(|| "system default".into());
        match self {
            Field::Compact => yes(s.appearance.compact),
            Field::InlineImages => yes(s.messages.inline_images),
            Field::Avatars => yes(s.messages.avatars),
            Field::EmojiAsNames => yes(s.messages.emoji_as_names),
            Field::Clock24 => yes(s.region.clock_24h),
            Field::DateFormat => Local::now().format(&s.region.date_format).to_string(),
            Field::Desktop => yes(s.notifications.desktop),
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
                let now = Local::now().timestamp() as f64;
                match ws.dnd.snooze_until.filter(|t| *t > now) {
                    Some(t) => chrono::TimeZone::timestamp_opt(&Local, t as i64, 0).single().map(|d| format!("until {}", d.format(s.time_format()))).unwrap_or_default(),
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
        }
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
            Field::Compact => s.appearance.compact ^= true,
            Field::InlineImages => s.messages.inline_images ^= true,
            Field::Avatars => s.messages.avatars ^= true,
            Field::EmojiAsNames => s.messages.emoji_as_names ^= true,
            Field::Clock24 => s.region.clock_24h ^= true,
            Field::DateFormat => {
                let i = DATE_FORMATS.iter().position(|f| *f == s.region.date_format).map_or(0, |i| (i + 1) % DATE_FORMATS.len());
                s.region.date_format = DATE_FORMATS[i].into();
            }
            Field::Desktop => s.notifications.desktop ^= true,
            // Slack-side: see `Prefs::change`.
            Field::ConversationLevel | Field::ChannelDefault | Field::Keywords | Field::Vips | Field::Status | Field::Away | Field::Pause => {}
            Field::Input => s.audio.input = next(&s.audio.input, &devices.0),
            Field::Output => s.audio.output = next(&s.audio.output, &devices.1),
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

fn next_level(level: &str) -> String {
    let i = LEVELS.iter().position(|l| *l == level).map_or(0, |i| (i + 1) % LEVELS.len());
    LEVELS[i].to_string()
}

#[derive(Debug, Default)]
pub struct Prefs {
    /// Text being typed into a field (keywords).
    pub editing: Option<String>,
    pub category: usize,
    pub selected: usize,
    /// Audio devices as ids (listed when the menu opens; only names, nothing is opened).
    pub devices: (Vec<String>, Vec<String>),
    /// Where the settings were saved, or why not.
    pub saved: Option<Result<String, String>>,
}

impl Prefs {
    pub fn new() -> Prefs {
        let id = |d: &String| d.split(" — ").next().unwrap_or(d).to_string();
        let devices = huddle::devices::list_devices().map(|(i, o)| (i.iter().map(id).collect(), o.iter().map(id).collect())).unwrap_or_default();
        Prefs { devices, ..Default::default() }
    }

    pub fn fields(&self) -> &'static [Field] {
        CATEGORIES[self.category].1
    }

    pub fn switch_category(&mut self, delta: isize) {
        self.category = (self.category as isize + delta).rem_euclid(CATEGORIES.len() as isize) as usize;
        self.selected = 0;
    }

    pub fn move_by(&mut self, delta: isize) {
        self.selected = (self.selected as isize + delta).clamp(0, self.fields().len() as isize - 1) as usize;
    }

    /// Change the selected setting: local ones in `ws.settings` (the caller saves them),
    /// Slack-side ones returned for the caller to send. A text field starts editing first.
    pub fn change(&mut self, ws: &mut Workspace, current: Option<&str>) -> Option<SlackChange> {
        let field = *self.fields().get(self.selected)?;
        match field {
            Field::ConversationLevel => {
                let channel = current?.to_string();
                let mpim = ws.conversations.get(&channel).is_some_and(|c| c.is_mpim);
                let level = next_level(ws.notify.level(&channel, mpim));
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
                let now = Local::now().timestamp() as f64;
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
        matches!(self.fields().get(self.selected), Some(Field::ConversationLevel | Field::ChannelDefault | Field::Keywords | Field::Vips | Field::Status | Field::Away | Field::Pause))
    }

    pub fn draw(&self, f: &mut Frame, area: Rect, ws: &Workspace, current: Option<&str>) {
        let [popup] = Layout::horizontal([Constraint::Length(84.min(area.width))]).flex(Flex::Center).areas(area);
        let [popup] = Layout::vertical([Constraint::Length(18.min(area.height))]).flex(Flex::Center).areas(popup);
        f.render_widget(Clear, popup);
        let saved = match &self.saved {
            Some(Ok(path)) => format!(" saved to {path} "),
            Some(Err(e)) => format!(" not saved: {e} "),
            None => String::new(),
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(0x61, 0xaf, 0xef)))
            .title(" Preferences ")
            .title_bottom(Line::styled(saved, Style::default().fg(DIM)));
        let inner = block.inner(popup);
        f.render_widget(block, popup);
        let [left, right] = Layout::horizontal([Constraint::Length(22), Constraint::Min(10)]).areas(inner);
        let highlight = Style::default().bg(Color::Rgb(0x2c, 0x3e, 0x5c));
        let cats: Vec<Line> = CATEGORIES
            .iter()
            .enumerate()
            .map(|(n, (name, _))| {
                let line = Line::raw(format!(" {name}"));
                if n == self.category { line.style(highlight.add_modifier(Modifier::BOLD)) } else { line }
            })
            .collect();
        f.render_widget(Paragraph::new(cats), left);
        let mut rows: Vec<Line> = Vec::new();
        let label_w = self.fields().iter().map(|f| f.label().chars().count()).max().unwrap_or(0) + 2;
        for (n, field) in self.fields().iter().enumerate() {
            let value = match (&self.editing, n == self.selected) {
                (Some(text), true) => format!("{text}▏"),
                _ => field.value(ws, current),
            };
            let line = Line::from(vec![Span::raw(format!(" {:<label_w$}", field.label())), Span::styled(value, Style::default().fg(Color::Rgb(0x98, 0xc3, 0x79)))]);
            rows.push(if n == self.selected { line.style(highlight) } else { line });
        }
        if CATEGORIES[self.category].0 == "Availability" {
            rows.push(Line::raw(""));
            rows.push(Line::styled(" Status: \":emoji: text for 30m\" (or today); empty clears.", Style::default().fg(DIM)));
        }
        if CATEGORIES[self.category].0 == "Notifications" {
            rows.push(Line::raw(""));
            rows.push(Line::styled(" All but the first are kept on Slack. Keywords: comma-separated.", Style::default().fg(DIM)));
        }
        if CATEGORIES[self.category].0 == "Audio" {
            rows.push(Line::raw(""));
            rows.push(Line::styled(" SLACKTUI_AUDIO_INPUT / _OUTPUT, when set, win over these.", Style::default().fg(DIM)));
        }
        rows.push(Line::raw(""));
        rows.push(Line::styled(" Keys are set in keys.toml (F1 lists them).", Style::default().fg(DIM)));
        rows.push(Line::styled(" Your status: /status, /away, /dnd.", Style::default().fg(DIM)));
        f.render_widget(Paragraph::new(rows).wrap(Wrap { trim: false }), right);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn slack_side_changes() {
        let mut ws = Workspace::default();
        let mut p = Prefs { category: 4, ..Default::default() };
        p.selected = 1;
        assert_eq!(p.change(&mut ws, Some("C1")), Some(SlackChange::ConversationLevel { channel: "C1".into(), level: "nothing".into() }), "mentions (default) → nothing");
        assert_eq!(p.change(&mut ws, None), None, "no conversation, no change");
        p.selected = 2;
        assert_eq!(p.change(&mut ws, None), Some(SlackChange::ChannelDefault("nothing".into())));
        p.selected = 3;
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
        ws.dnd.snooze_until = Some(Local::now().timestamp() as f64 + 29.6 * 60.0);
        assert_eq!(p.change(&mut ws, None), Some(SlackChange::Command("/dnd 60m".into())), "30 min left → an hour");
        ws.dnd.snooze_until = Some(Local::now().timestamp() as f64 + 470.0 * 60.0);
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
        assert_eq!(p.category, CATEGORIES.len() - 1);
        p.move_by(10);
        assert_eq!(p.selected, p.fields().len() - 1);
    }
}

//! Preferences (F6): local settings by category; a change applies at once and is saved
//! to settings.toml.

use chrono::Local;
use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use super::settings::Settings;
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
    Input,
    Output,
}

pub const CATEGORIES: &[(&str, &[Field])] = &[
    ("Appearance", &[Field::Compact]),
    ("Messages & media", &[Field::InlineImages, Field::Avatars, Field::EmojiAsNames]),
    ("Language & region", &[Field::Clock24, Field::DateFormat]),
    ("Notifications", &[Field::Desktop]),
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
            Field::Input => "Microphone",
            Field::Output => "Speaker",
        }
    }

    pub fn value(self, s: &Settings) -> String {
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
            Field::Input => s.audio.input = next(&s.audio.input, &devices.0),
            Field::Output => s.audio.output = next(&s.audio.output, &devices.1),
        }
    }
}

#[derive(Debug, Default)]
pub struct Prefs {
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

    /// Change the selected setting; the caller applies and saves `s`.
    pub fn change(&mut self, s: &mut Settings) {
        if let Some(f) = self.fields().get(self.selected) {
            f.change(s, &self.devices);
        }
    }

    pub fn draw(&self, f: &mut Frame, area: Rect, s: &Settings) {
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
            let line = Line::from(vec![Span::raw(format!(" {:<label_w$}", field.label())), Span::styled(field.value(s), Style::default().fg(Color::Rgb(0x98, 0xc3, 0x79)))]);
            rows.push(if n == self.selected { line.style(highlight) } else { line });
        }
        if CATEGORIES[self.category].0 == "Audio" {
            rows.push(Line::raw(""));
            rows.push(Line::styled(" SLACKTUI_AUDIO_INPUT / _OUTPUT, when set, win over these.", Style::default().fg(DIM)));
        }
        rows.push(Line::raw(""));
        rows.push(Line::styled(" Keys are set in keys.toml (F1 lists them).", Style::default().fg(DIM)));
        rows.push(Line::styled(" Kept on Slack: status (/status, /away, /dnd).", Style::default().fg(DIM)));
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
    fn categories_cycle() {
        let mut p = Prefs::default();
        p.switch_category(-1);
        assert_eq!(p.category, CATEGORIES.len() - 1);
        p.move_by(10);
        assert_eq!(p.selected, p.fields().len() - 1);
    }
}

//! The huddle view (F8): instead of the message pane, a tile per participant with their
//! picture, speaking meter and mute state, and the call's controls underneath.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui_image::sliced::{SignedPosition, SlicedImage};
use slack_core::types::File;

use super::call::{Call, meter};
use super::media::{Media, Thumb};
use super::state::Workspace;

/// A participant's tile at the smallest scale, in cells.
pub const TILE_W: u16 = 26;
pub const TILE_H: u16 = 9;
/// The picture inside a tile at the smallest scale (about square).
pub const PICTURE: (u16, u16) = (8, 4);
/// Tiles grow up to this many times their smallest size when there is room.
pub const MAX_SCALE: u16 = 3;

#[derive(Default)]
pub struct HuddleView {
    /// Pictures that were on screen but not loaded yet (the caller fetches them).
    pub wanted: Vec<(File, (u16, u16))>,
}

/// Where `n` tiles go in `area`, and their scale (1…MAX_SCALE): the largest whole scale
/// at which all of them fit (trying every column count), in centred rows. If none fits even
/// at scale 1, as many as fit are placed and the rest left out (the caller says how many).
pub fn grid(n: usize, area: Rect) -> (Vec<Rect>, u16) {
    let fits = |cols: usize, scale: u16| {
        let rows = n.div_ceil(cols.max(1));
        cols as u16 * TILE_W * scale <= area.width && rows as u16 * TILE_H * scale <= area.height
    };
    let best = (1..=MAX_SCALE).rev().find_map(|scale| (1..=n.max(1)).find(|c| fits(*c, scale)).map(|c| (c, scale)));
    let (cols, scale) = best.unwrap_or(((area.width / TILE_W).max(1) as usize, 1));
    let (w, h) = (TILE_W * scale, TILE_H * scale);
    let shown = n.min(cols * (area.height / h) as usize);
    let rects = (0..shown)
        .map(|i| {
            let (row, col) = (i / cols, i % cols);
            let in_row = (shown - row * cols).min(cols) as u16;
            let left = area.x + area.width.saturating_sub(in_row * w) / 2;
            Rect { x: left + col as u16 * w, y: area.y + row as u16 * h, width: w.min(area.width), height: h }
        })
        .collect();
    (rects, scale)
}

/// The call's keys in a line, shortened when `width` is too narrow for the long form.
pub fn controls(keys: (&str, &str, &str, &str), muted: bool, width: usize) -> String {
    let (mute, leave, thread, view) = keys;
    let mic = if muted { "unmute" } else { "mute" };
    let long = format!(" {mute} {mic} · {thread} thread · {leave} leave · {view} back to the messages");
    if long.chars().count() <= width { long } else { format!(" {mute} {mic} · {thread} thread · {leave} leave · {view} messages") }
}

/// Rows `text` takes when wrapped at word boundaries to `width` (as `Wrap { trim: true }`).
pub fn wrapped_rows(text: &str, width: usize) -> u16 {
    let width = width.max(1);
    let (mut rows, mut used) = (1u16, 0usize);
    for word in text.split_whitespace() {
        let w = unicode_width::UnicodeWidthStr::width(word);
        if used > 0 && used + 1 + w > width {
            rows += 1;
            used = 0;
        }
        used = if used == 0 { w } else { used + 1 + w };
        while used > width {
            rows += 1;
            used -= width;
        }
    }
    rows
}

/// The audio settings in a line, shortened when `width` is too narrow for the long form.
pub fn audio_summary(a: &super::settings::Audio, width: usize) -> String {
    let threshold = a.speaking_threshold_db.map_or("off".to_string(), |d| format!("{d} dB"));
    let on = |b: bool| if b { "on" } else { "off" };
    let long = format!(" volume {}% · mic gain {}% · threshold {threshold} · noise suppression {} · F6 → Audio", a.huddle_volume, a.mic_gain, on(a.noise_suppression));
    if long.chars().count() <= width {
        long
    } else {
        format!(" vol {}% · gain {}% · gate {threshold} · noise {}", a.huddle_volume, a.mic_gain, on(a.noise_suppression))
    }
}

/// What a tile says under the meter.
pub fn state_text(muted: bool, speaking: bool, gated: bool) -> &'static str {
    if muted {
        "muted"
    } else if gated {
        "below the speaking threshold"
    } else if speaking {
        "speaking"
    } else {
        ""
    }
}

impl HuddleView {
    /// `keys`: mute, leave, thread and view keys, for the controls line.
    pub fn draw(&mut self, f: &mut Frame, area: Rect, call: &Call, ws: &Workspace, media: &Media, keys: (&str, &str, &str, &str)) {
        let dim = Style::default().fg(super::theme::p().dim);
        let green = Style::default().fg(super::theme::p().own);
        let place = call.place(ws);
        let state = if call.connected { "connected" } else { "connecting…" };
        let people = match call.roster.len() {
            1 => "1 person".to_string(),
            n => format!("{n} people"),
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(super::theme::p().accent))
            .title(Line::from(vec![Span::styled(format!(" 🎧 {place} "), green.add_modifier(Modifier::BOLD)), Span::styled(format!("· {people} · {state} "), dim)]));
        let inner = block.inner(area);
        f.render_widget(block, area);
        // The controls and audio lines, wrapped in a narrow pane.
        let width = inner.width as usize;
        let texts = [controls(keys, call.muted, width), audio_summary(&ws.settings.audio, width)];
        let footer_h = (texts.iter().map(|t| wrapped_rows(t, width)).sum::<u16>()).min(inner.height);
        let tiles_area = Rect { height: inner.height.saturating_sub(footer_h + 1), y: inner.y + 1, ..inner };
        let footer = Rect { y: inner.y + inner.height - footer_h, height: footer_h, ..inner };

        let (rects, scale) = grid(call.roster.len(), tiles_area);
        let picture = (PICTURE.0 * scale, PICTURE.1 * scale);
        for (p, rect) in call.roster.iter().zip(&rects) {
            self.draw_tile(f, *rect, picture, call, p, ws, media);
        }
        if rects.len() < call.roster.len() {
            let more = Rect { y: tiles_area.y + tiles_area.height.saturating_sub(1), height: 1.min(tiles_area.height), ..tiles_area };
            f.render_widget(Paragraph::new(format!("+{} more (make the window larger)", call.roster.len() - rects.len())).style(dim).alignment(Alignment::Center), more);
        }

        let lines: Vec<Line> = texts.into_iter().map(|t| Line::styled(t, dim)).collect();
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), footer);
    }

    fn draw_tile(&mut self, f: &mut Frame, rect: Rect, picture: (u16, u16), call: &Call, p: &huddle::session::Participant, ws: &Workspace, media: &Media) {
        let me = p.user_id() == ws.self_id;
        let (level, speaking, muted, gated) = call.participant_state(p, &ws.self_id);
        let green = Style::default().fg(super::theme::p().own);
        let dim = Style::default().fg(super::theme::p().dim);
        let name = ws.person_label(p.user_id());
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(if speaking { green.add_modifier(Modifier::BOLD) } else { dim })
            .title(Span::styled(format!(" {name} "), if speaking { green.add_modifier(Modifier::BOLD) } else { Style::default() }));
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        if inner.height == 0 {
            return;
        }
        // The picture, centred at the top; the initial while it loads (or without graphics).
        // Leave a row above the picture once the tile is large.
        let top = if inner.height > picture.1 + 4 { 1 } else { 0 };
        let pic = Rect { x: inner.x + inner.width.saturating_sub(picture.0) / 2, y: inner.y + top, width: picture.0.min(inner.width), height: picture.1.min(inner.height - top) };
        // Each size is its own cache entry (thumbnails are encoded for one size).
        let file = ws.users.get(p.user_id()).and_then(|u| super::media::avatar_file_sized(u, true)).filter(|_| media.avatars && ws.settings.messages.avatars).map(|mut file| {
            file.id = format!("{}:{}x{}", file.id, picture.0, picture.1);
            file
        });
        let mut drawn = false;
        if let Some(file) = file {
            match media.thumbs.get(&file.id) {
                Some(Thumb::Ready(proto)) => {
                    f.render_widget(SlicedImage::new(proto, SignedPosition { x: 0, y: 0 }), pic);
                    drawn = true;
                }
                Some(_) => {}
                None => self.wanted.push((file, picture)),
            }
        }
        if !drawn {
            let initial: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
            let mid = Rect { y: pic.y + pic.height / 2, height: 1.min(pic.height), ..pic };
            f.render_widget(Paragraph::new(initial).style(Style::default().add_modifier(Modifier::BOLD)).alignment(Alignment::Center), mid);
        }
        let below = Rect { y: pic.y + pic.height + top, height: inner.height.saturating_sub(pic.height + 2 * top), ..inner };
        let meter_line = if muted { Line::styled("🔇", dim) } else { Line::styled(meter(level), if speaking { green } else { dim }) };
        let mut lines = vec![meter_line.alignment(Alignment::Center)];
        let text = state_text(muted, speaking, gated);
        let text = if me && text.is_empty() { "you" } else { text };
        lines.push(Line::styled(text, if speaking { green } else { dim }).alignment(Alignment::Center));
        f.render_widget(Paragraph::new(lines), below);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_fill_rows_and_centre() {
        let area = Rect::new(10, 5, 80, 20);
        // 80 columns hold three 26-wide tiles; 20 rows hold two rows of 9.
        let (one, scale) = grid(1, area);
        assert_eq!((one, scale), (vec![Rect::new(10 + (80 - 52) / 2, 5, 52, 18)], 2), "a single tile, doubled and centred");
        let (four, scale) = grid(4, area);
        assert_eq!((four.len(), scale), (4, 1));
        let corners: Vec<(u16, u16)> = four.iter().map(|r| (r.x, r.y)).collect();
        assert_eq!(corners, vec![(24, 5), (50, 5), (24, 14), (50, 14)], "2 × 2 (the fewest columns that fit), centred");
        let (three, _) = grid(3, Rect::new(0, 0, 80, 18));
        assert_eq!((three[2].x, three[2].y), ((80 - 26) / 2, 9), "a short last row is centred");
        assert_eq!(grid(9, area).0.len(), 6, "only what fits");
        assert!(grid(3, Rect::new(0, 0, 80, 5)).0.is_empty(), "too short for a tile");
        let (narrow, _) = grid(2, Rect::new(0, 0, 20, 20));
        assert_eq!((narrow.len(), narrow[0].width), (2, 20), "one column, clipped to the width");
        // Two people in a tall pane: one column at twice the size beats two small columns.
        let (tall, scale) = grid(2, Rect::new(0, 0, 67, 40));
        assert_eq!((scale, tall[0].x, tall[1].y), (2, (67 - 52) / 2, 18));
        assert_eq!(grid(1, Rect::new(0, 0, 200, 60)).1, MAX_SCALE);
    }

    #[test]
    fn audio_line_shortens() {
        let a = crate::tui::settings::Audio { speaking_threshold_db: Some(-45), noise_suppression: true, ..Default::default() };
        assert_eq!(audio_summary(&a, 200), " volume 100% · mic gain 100% · threshold -45 dB · noise suppression on · F6 → Audio");
        assert_eq!(audio_summary(&a, 60), " vol 100% · gain 100% · gate -45 dB · noise on");
        assert_eq!(wrapped_rows(" vol 100% · gain 100% · gate off · noise off", 80), 1);
        assert_eq!(wrapped_rows(" vol 100% · gain 100% · gate off · noise off", 38), 2);
        assert_eq!(wrapped_rows("abcdefghij", 4), 3, "a long word is broken");
        let keys = ("F4", "F3", "F7", "F8");
        assert_eq!(controls(keys, true, 80), " F4 unmute · F7 thread · F3 leave · F8 back to the messages");
        assert_eq!(controls(keys, false, 40), " F4 mute · F7 thread · F3 leave · F8 messages");
    }

    #[test]
    fn tile_states() {
        assert_eq!(state_text(true, false, false), "muted");
        assert_eq!(state_text(false, true, false), "speaking");
        assert_eq!(state_text(false, true, true), "below the speaking threshold");
        assert_eq!(state_text(false, false, false), "");
    }
}

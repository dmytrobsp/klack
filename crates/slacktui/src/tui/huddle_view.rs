//! The huddle view (F8): instead of the message pane, a tile per participant with their
//! picture or video (the border lights up while they speak, 🔇 in the title when muted),
//! and the call's controls underneath.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui_image::sliced::{SignedPosition, SlicedImage};
use slack_core::types::File;

use super::call::Call;
use super::media::{Media, Thumb};
use super::state::Workspace;

/// The smallest tile, in cells: a short name and a small picture.
pub const MIN_TILE: (u16, u16) = (18, 6);
/// Tile width ÷ height in cells: a 16:9 picture in cells twice as tall as wide, plus borders.
const ASPECT: f32 = 3.2;

#[derive(Default)]
pub struct HuddleView {
    /// Pictures that were on screen but not loaded yet (the caller fetches them).
    pub wanted: Vec<(File, (u16, u16))>,
    /// Each participant's video picture as encoded for the terminal, with the frame it came
    /// from and the size it was encoded for (re-encoded only when either changes).
    encoded: std::collections::HashMap<String, (std::sync::Arc<Vec<u8>>, ratatui::layout::Size, ratatui_image::protocol::Protocol)>,
    /// First row of tiles shown when they do not all fit.
    pub scroll: usize,
    /// Where the tiles were drawn (the mouse wheel scrolls them).
    pub tiles_area: Rect,
}

/// Where tiles go: `tiles` are (tile index, rect); `above`/`below` count the tiles scrolled
/// out of view; `scroll` is the first row shown (clamped).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Grid {
    pub tiles: Vec<(usize, Rect)>,
    pub cols: usize,
    pub above: usize,
    pub below: usize,
    pub scroll: usize,
}

/// Lay out `n` tiles in `area`: the largest tiles (about camera-shaped) at which all of them
/// fit, trying every column count, rows centred. When even the smallest tiles do not fit,
/// the grid scrolls by rows from `scroll`, with a row kept free above and below for the
/// "more" markers.
pub fn layout(n: usize, area: Rect, scroll: usize) -> Grid {
    if n == 0 || area.width == 0 || area.height == 0 {
        return Grid::default();
    }
    // Keep a tile camera-shaped within the space it gets.
    let fit = |w: u16, h: u16| -> (u16, u16) {
        if w as f32 > h as f32 * ASPECT { ((h as f32 * ASPECT) as u16, h) } else { (w, (w as f32 / ASPECT).round().max(1.0) as u16) }
    };
    let best = (1..=n)
        .filter_map(|cols| {
            let rows = n.div_ceil(cols);
            let (w, h) = fit(area.width / cols as u16, area.height / rows as u16);
            (w >= MIN_TILE.0 && h >= MIN_TILE.1).then_some((cols, rows, w, h))
        })
        .max_by_key(|(cols, _, w, h)| (*w as u32 * *h as u32, usize::MAX - cols));
    let (cols, rows_shown, w, h, first_row, top, above, below, scroll) = match best {
        Some((cols, rows, w, h)) => (cols, rows, w, h, 0, area.y + area.height.saturating_sub(rows as u16 * h) / 2, 0, 0, 0),
        None => {
            let cols = ((area.width / MIN_TILE.0) as usize).max(1);
            let (w, h) = fit(area.width / cols as u16, area.height);
            let h = h.max(MIN_TILE.1);
            let rows = n.div_ceil(cols);
            // One row for each marker, and at least one row of tiles.
            let room = (area.height.saturating_sub(2) / h).max(1) as usize;
            let scroll = scroll.min(rows.saturating_sub(room));
            let below = n.saturating_sub((scroll + room) * cols);
            (cols, room.min(rows - scroll), w, h, scroll, area.y + 1, scroll * cols, below, scroll)
        }
    };
    let mut tiles = Vec::new();
    for row in 0..rows_shown {
        let first = (first_row + row) * cols;
        let in_row = cols.min(n.saturating_sub(first));
        let left = area.x + area.width.saturating_sub(in_row as u16 * w) / 2;
        for c in 0..in_row {
            let rect = Rect { x: left + c as u16 * w, y: top + row as u16 * h, width: w, height: h }.intersection(area);
            tiles.push((first + c, rect));
        }
    }
    Grid { tiles, cols, above, below, scroll }
}

/// The footer lines that fit and their height: at most a third of the view's `height`, so
/// tiles keep the room; the audio line goes first, then the controls are cut.
pub fn footer(lines: &[String], width: usize, height: u16) -> (Vec<String>, u16) {
    let budget = (height / 3).max(1);
    let mut shown: Vec<String> = Vec::new();
    let mut used = 0;
    for line in lines {
        let rows = wrapped_rows(line, width);
        if used + rows <= budget {
            shown.push(line.clone());
            used += rows;
        } else if shown.is_empty() {
            // Not even the controls fit: as much of them as the budget allows.
            shown.push(line.clone());
            used = budget;
        }
    }
    (shown, used.min(height))
}

/// "▲ 4 more" / "▼ 6 more".
pub fn more_marker(up: bool, n: usize) -> String {
    format!("{} {n} more", if up { "▲" } else { "▼" })
}

/// The call's keys in a line, shortened when `width` is too narrow for the long form.
pub fn controls(keys: Keys, muted: bool, camera_on: bool, sharing: bool, width: usize) -> String {
    let Keys { mute, leave, thread, view, camera, share } = keys;
    let mic = if muted { "unmute" } else { "mute" };
    let cam = if camera_on { "camera off" } else { "camera on" };
    let screen = if sharing { "stop sharing" } else { "share screen" };
    let long = format!(" {mute} {mic} · {camera} {cam} · {share} {screen} · {thread} thread · {leave} leave · {view} back to the messages");
    if long.chars().count() <= width { long } else { format!(" {mute} {mic} · {camera} {cam} · {share} {screen} · {thread} thread · {leave} leave · {view} messages") }
}

/// The call's keys, for the hints.
#[derive(Debug, Clone, Copy)]
pub struct Keys<'a> {
    pub mute: &'a str,
    pub leave: &'a str,
    pub thread: &'a str,
    pub view: &'a str,
    pub camera: &'a str,
    pub share: &'a str,
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

/// A tile's title: the name, and 🔇 when muted.
pub fn tile_title(name: &str, muted: bool) -> String {
    if muted { format!(" {name} 🔇 ") } else { format!(" {name} ") }
}

/// A picture's size in cells for a tile's inside: about square (cells are twice as tall as
/// wide), as large as fits, in a few steps so each size is cached once; None when tiny.
pub fn picture_size(inner: Rect) -> Option<(u16, u16)> {
    const STEPS: [u16; 9] = [2, 3, 4, 5, 6, 8, 10, 12, 16];
    let fit = inner.height.min(inner.width / 2);
    STEPS.iter().rev().find(|s| **s <= fit).map(|h| (h * 2, *h))
}

/// A tile's border: lit (green, thick) while the person speaks.
fn tile_block(title: Span<'static>, speaking: bool) -> Block<'static> {
    let (style, kind) = if speaking { (Style::default().fg(super::theme::p().own).add_modifier(Modifier::BOLD), BorderType::Thick) } else { (Style::default().fg(super::theme::p().dim), BorderType::Plain) };
    Block::default().borders(Borders::ALL).border_type(kind).border_style(style).title(title)
}

impl HuddleView {
    /// `keys`: mute, leave, thread and view keys, for the controls line.
    pub fn draw(&mut self, f: &mut Frame, area: Rect, call: &Call, ws: &Workspace, media: &Media, keys: Keys) {
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
        let all = [controls(keys, call.muted, call.camera_on(), call.is_sharing(), width), audio_summary(&ws.settings.audio, width)];
        let (texts, footer_h) = footer(&all, width, inner.height);
        let tiles_area = Rect { height: inner.height.saturating_sub(footer_h + 1), y: inner.y + 1, ..inner };
        let footer = Rect { y: inner.y + inner.height - footer_h, height: footer_h, ..inner };

        // Screen shares first, then the people.
        let shares = call.shown_shares(&ws.self_id);
        let tiles = shares.len() + call.roster.len();
        self.tiles_area = tiles_area;
        let grid = layout(tiles, tiles_area, self.scroll);
        self.scroll = grid.scroll;
        for (n, rect) in &grid.tiles {
            match shares.get(*n) {
                Some((share, user)) => self.draw_share(f, *rect, call, share, user, ws, media),
                None => self.draw_tile(f, *rect, call, &call.roster[*n - shares.len()], ws, media),
            }
        }
        // Tiles scrolled out of view are counted above and below.
        let marker = Style::default().fg(super::theme::p().accent).add_modifier(Modifier::BOLD);
        if grid.above > 0 {
            let top = Rect { height: 1.min(tiles_area.height), ..tiles_area };
            f.render_widget(Paragraph::new(more_marker(true, grid.above)).style(marker).alignment(Alignment::Center), top);
        }
        if grid.below > 0 {
            let bottom = Rect { y: tiles_area.y + tiles_area.height.saturating_sub(1), height: 1.min(tiles_area.height), ..tiles_area };
            f.render_widget(Paragraph::new(more_marker(false, grid.below)).style(marker).alignment(Alignment::Center), bottom);
        }

        let lines: Vec<Line> = texts.iter().map(|t| Line::styled(t.clone(), dim)).collect();
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), footer);
    }

    fn draw_tile(&mut self, f: &mut Frame, rect: Rect, call: &Call, p: &huddle::session::Participant, ws: &Workspace, media: &Media) {
        let (_, speaking, muted, gated) = call.participant_state(p, &ws.self_id);
        let name = ws.person_label(p.user_id());
        // Below the speaking threshold our own name is dimmed: nothing is being sent.
        let title_style = if speaking { Style::default().fg(super::theme::p().own).add_modifier(Modifier::BOLD) } else if gated { Style::default().fg(super::theme::p().dim) } else { Style::default() };
        let block = tile_block(Span::styled(tile_title(&name, muted), title_style), speaking);
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        if inner.height == 0 {
            return;
        }
        // Someone's video fills the tile.
        if let Some(frame) = call.video(&p.attendee_id) {
            self.draw_video(f, inner, &p.attendee_id, &frame, media);
            return;
        }
        // Otherwise their picture, centred; the initial while it loads (or without graphics).
        let picture = picture_size(inner);
        let (pw, ph) = picture.unwrap_or((1, 1));
        let pic = Rect { x: inner.x + inner.width.saturating_sub(pw) / 2, y: inner.y + inner.height.saturating_sub(ph) / 2, width: pw.min(inner.width), height: ph.min(inner.height) };
        // Each size is its own cache entry (thumbnails are encoded for one size).
        let file = picture.and_then(|picture| {
            ws.users.get(p.user_id()).and_then(|u| super::media::avatar_file_sized(u, true)).filter(|_| media.avatars && ws.settings.messages.avatars).map(|mut file| {
                file.id = format!("{}:{}x{}", file.id, picture.0, picture.1);
                (file, picture)
            })
        });
        let mut drawn = false;
        if let Some((file, picture)) = file {
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
    }

    /// Someone's screen share: their name and the picture filling the tile.
    #[allow(clippy::too_many_arguments)]
    fn draw_share(&mut self, f: &mut Frame, rect: Rect, call: &Call, share: &str, user: &str, ws: &Workspace, media: &Media) {
        let accent = Style::default().fg(super::theme::p().accent);
        let title = if user == ws.self_id { " 🖥 Your screen ".to_string() } else { format!(" 🖥 {}'s screen ", ws.person_label(user)) };
        let block = Block::default().borders(Borders::ALL).border_style(accent).title(Span::styled(title, accent.add_modifier(Modifier::BOLD)));
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        match call.video(share) {
            Some(frame) => self.draw_video(f, inner, share, &frame, media),
            None => {
                let mid = Rect { y: inner.y + inner.height / 2, height: 1.min(inner.height), ..inner };
                f.render_widget(Paragraph::new("waiting for the picture…").style(Style::default().fg(super::theme::p().dim)).alignment(Alignment::Center), mid);
            }
        }
    }

    /// A video picture fitted into `area` (keeping its aspect), centred.
    fn draw_video(&mut self, f: &mut Frame, area: Rect, attendee: &str, frame: &huddle::session::VideoFrame, media: &Media) {
        use ratatui::layout::{Constraint, Flex, Layout, Size};
        if area.width == 0 || area.height == 0 {
            return;
        }
        let size = Size::new(area.width, area.height);
        let fresh = self.encoded.get(attendee).is_some_and(|(rgb, s, _)| std::sync::Arc::ptr_eq(rgb, &frame.rgb) && *s == size);
        if !fresh {
            let Some(img) = image::RgbImage::from_raw(frame.width as u32, frame.height as u32, frame.rgb.to_vec()) else { return };
            match media.picker.new_protocol(image::DynamicImage::ImageRgb8(img), size, ratatui_image::Resize::Fit(None)) {
                Ok(proto) => {
                    self.encoded.insert(attendee.to_string(), (frame.rgb.clone(), size, proto));
                }
                Err(e) => tracing::debug!("video picture: {e}"),
            }
        }
        if let Some((_, _, proto)) = self.encoded.get(attendee) {
            let fit = proto.size();
            let [x] = Layout::horizontal([Constraint::Length(fit.width)]).flex(Flex::Center).areas(area);
            let [cell] = Layout::vertical([Constraint::Length(fit.height)]).flex(Flex::Center).areas(x);
            f.render_widget(ratatui_image::Image::new(proto), cell);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tiles never leave the area or overlap.
    fn sane(g: &Grid, area: Rect) {
        for (n, (_, a)) in g.tiles.iter().enumerate() {
            assert_eq!(a.intersection(area), *a, "inside");
            for (_, b) in &g.tiles[n + 1..] {
                assert!(!a.intersects(*b), "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn tiles_shrink_as_people_join() {
        let area = Rect::new(2, 3, 120, 40);
        let one = layout(1, area, 0);
        let (_, r) = one.tiles[0];
        assert!(r.width >= 100 && r.height >= 30, "one person gets most of the space: {r:?}");
        assert_eq!(r.x - area.x, area.width - r.width - (r.x - area.x), "centred");
        let mut last = u32::MAX;
        for n in 1..=12 {
            let g = layout(n, area, 0);
            sane(&g, area);
            assert_eq!(g.tiles.len(), n, "{n} fit without scrolling");
            let (_, r) = g.tiles[0];
            assert!(r.width >= MIN_TILE.0 && r.height >= MIN_TILE.1);
            let size = r.width as u32 * r.height as u32;
            assert!(size <= last, "tiles do not grow when someone joins ({n})");
            last = size;
        }
    }

    #[test]
    fn overflow_scrolls_with_markers() {
        let area = Rect::new(0, 0, 80, 20);
        let g = layout(30, area, 0);
        sane(&g, area);
        assert_eq!(g.cols, 4, "80 columns of 18-wide tiles");
        assert_eq!((g.above, g.tiles.len() + g.below), (0, 30));
        assert!(g.below > 0 && g.tiles.iter().all(|(_, r)| r.y >= 1 && r.y + r.height <= 19), "rows free for the markers");
        let s1 = layout(30, area, 1);
        assert_eq!((s1.above, s1.tiles[0].0), (4, 4), "one row down");
        let end = layout(30, area, 99);
        assert_eq!(end.below, 0, "scrolling stops at the end");
        assert_eq!(end.above + end.tiles.len(), 30);
        assert_eq!(more_marker(true, 4), "▲ 4 more");
        assert!(layout(0, area, 0).tiles.is_empty());
        // Too small for even one minimum row: still one row, never nothing.
        assert_eq!(layout(5, Rect::new(0, 0, 40, 5), 0).tiles.len(), 2);
    }

    #[test]
    fn footer_yields_to_the_tiles() {
        let lines = vec!["F4 unmute · F9 camera on · F10 share screen".to_string(), "vol 100% · gain 100%".to_string()];
        assert_eq!(footer(&lines, 80, 40), (lines.clone(), 2), "room for both");
        assert_eq!(footer(&lines, 20, 40).1, 4, "wrapped");
        assert_eq!(footer(&lines, 20, 9), (vec![lines[0].clone()], 3), "the audio line goes first");
        assert_eq!(footer(&lines, 10, 6), (vec![lines[0].clone()], 2), "the controls are cut to a third");
    }

    #[test]
    fn audio_line_shortens() {
        let a = crate::tui::settings::Audio { speaking_threshold_db: Some(-45), noise_suppression: true, ..Default::default() };
        assert_eq!(audio_summary(&a, 200), " volume 100% · mic gain 100% · threshold -45 dB · noise suppression on · F6 → Audio");
        assert_eq!(audio_summary(&a, 60), " vol 100% · gain 100% · gate -45 dB · noise on");
        assert_eq!(wrapped_rows(" vol 100% · gain 100% · gate off · noise off", 80), 1);
        assert_eq!(wrapped_rows(" vol 100% · gain 100% · gate off · noise off", 38), 2);
        assert_eq!(wrapped_rows("abcdefghij", 4), 3, "a long word is broken");
        let keys = Keys { mute: "F4", leave: "F3", thread: "F7", view: "F8", camera: "F9", share: "F10" };
        assert_eq!(controls(keys, true, false, false, 120), " F4 unmute · F9 camera on · F10 share screen · F7 thread · F3 leave · F8 back to the messages");
        assert_eq!(controls(keys, false, true, true, 40), " F4 mute · F9 camera off · F10 stop sharing · F7 thread · F3 leave · F8 messages");
    }

    #[test]
    fn titles_and_pictures() {
        assert_eq!(tile_title("Ann", true), " Ann 🔇 ");
        assert_eq!(tile_title("Ann", false), " Ann ");
        // About square in pixels (cells are 1:2), in steps, the largest that fits.
        assert_eq!(picture_size(Rect::new(0, 0, 50, 16)), Some((32, 16)));
        assert_eq!(picture_size(Rect::new(0, 0, 13, 16)), Some((12, 6)), "width-bound");
        assert_eq!(picture_size(Rect::new(0, 0, 40, 7)), Some((12, 6)), "between steps: the smaller");
        assert_eq!(picture_size(Rect::new(0, 0, 40, 1)), None, "tiny: the initial instead");
    }
}

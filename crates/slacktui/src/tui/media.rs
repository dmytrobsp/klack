//! Inline image thumbnails: terminal graphics detection, sizing and a per-file cache.

use std::collections::HashMap;

use ratatui::layout::Size;
use ratatui_image::FontSize;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::sliced::SlicedProtocol;
use slack_core::types::File;

/// Largest thumbnail, in cells.
pub const MAX_COLS: u16 = 36;
pub const MAX_ROWS: u16 = 10;

pub enum Thumb {
    Loading,
    Ready(SlicedProtocol),
    /// Why, in a few words (shown in place of the picture).
    Failed(String),
}

pub struct Media {
    pub picker: Picker,
    pub thumbs: HashMap<String, Thumb>,
    /// Draw profile pictures next to author names (real graphics only; `SLACKTUI_AVATARS=0` turns them off).
    pub avatars: bool,
    /// What the terminal said it can do (for "auto").
    detected: ProtocolType,
}

/// The ways of drawing pictures to choose from ("auto": ask the terminal).
pub const GRAPHICS: &[&str] = &["auto", "kitty", "sixel", "iterm2", "halfblocks"];

/// Overrides the graphics setting.
pub const GRAPHICS_ENV: &str = "SLACKTUI_GRAPHICS";

/// An avatar is two cells wide and one tall (about square).
pub const AVATAR_SIZE: (u16, u16) = (2, 1);
pub const AVATAR_PREFIX: &str = "avatar:";

/// A Slack-hosted picture for `user`, as a pseudo file for the thumbnail cache. Gravatar
/// URLs are replaced by their `d=` fallback (Slack's default picture), so no third party is
/// contacted; anything not on `slack-edge.com` is skipped.
pub fn avatar_file(user: &slack_core::types::User) -> Option<File> {
    avatar_file_sized(user, false)
}

/// [`avatar_file`], or with `large` the 192-pixel picture (for the huddle view; cached
/// under its own id, since cache entries are encoded for one size).
pub fn avatar_file_sized(user: &slack_core::types::User, large: bool) -> Option<File> {
    let source = if large { user.profile.image_192.as_deref().or(user.profile.image_48.as_deref()) } else { user.profile.image_48.as_deref() };
    let url = slack_core::reqwest::Url::parse(source?).ok()?;
    let slack_hosted = |u: &slack_core::reqwest::Url| u.scheme() == "https" && u.host_str().is_some_and(|h| h == "slack-edge.com" || h.ends_with(".slack-edge.com"));
    let url = if slack_hosted(&url) {
        url
    } else {
        let fallback = url.query_pairs().find(|(k, _)| k == "d").and_then(|(_, v)| slack_core::reqwest::Url::parse(&v).ok())?;
        slack_hosted(&fallback).then_some(fallback)?
    };
    let id = if large { format!("{AVATAR_PREFIX}{}@192", user.id) } else { format!("{AVATAR_PREFIX}{}", user.id) };
    Some(File { id, mimetype: "image/png".into(), thumb_360: Some(url.to_string()), ..Default::default() })
}

impl Media {
    /// Ask the terminal for its graphics protocol and cell size (call after entering raw
    /// mode, before reading events); falls back to half-block characters. `choice` is the
    /// graphics setting (see [`GRAPHICS`]).
    pub fn detect(choice: &str) -> Media {
        let mut picker = Picker::from_query_stdio().unwrap_or_else(|e| {
            tracing::warn!("terminal graphics query failed, using half blocks: {e}");
            Picker::halfblocks()
        });
        // A terminal that does not report its cell size (xterm by default) loses its sixel
        // support in the query above; ask for it on its own and take the size from the
        // window's pixel size.
        if picker.protocol_type() == ProtocolType::Halfblocks && sixel_advertised() {
            tracing::info!("the terminal advertises sixel but did not report its cell size: using sixel");
            picker.set_protocol_type(ProtocolType::Sixel);
        }
        tracing::info!(protocol = ?picker.protocol_type(), font = ?picker.font_size(), capabilities = ?picker.capabilities(), "terminal graphics");
        let detected = picker.protocol_type();
        let mut media = Media { picker, thumbs: HashMap::new(), avatars: false, detected };
        media.choose(choice);
        media
    }

    /// Pictures drawn with `picker`, without profile pictures (for tests).
    #[cfg(test)]
    pub fn with(picker: Picker) -> Media {
        let detected = picker.protocol_type();
        Media { picker, thumbs: HashMap::new(), avatars: false, detected }
    }

    /// Draw pictures as `choice` says ("auto": as detected; `SLACKTUI_GRAPHICS` wins);
    /// pictures made the other way are dropped.
    pub fn choose(&mut self, choice: &str) {
        let env = std::env::var(GRAPHICS_ENV).ok().filter(|v| !v.trim().is_empty());
        let protocol = protocol_named(env.as_deref().unwrap_or(choice)).unwrap_or(self.detected);
        if protocol != self.picker.protocol_type() {
            self.thumbs.clear();
        }
        self.picker.set_protocol_type(protocol);
        self.avatars = protocol != ProtocolType::Halfblocks && std::env::var("SLACKTUI_AVATARS").map_or(true, |v| v != "0");
    }

    /// Cells to reserve under an image file's line when at most `max_cols` wide, or None
    /// for non-images.
    pub fn thumb_size(&self, file: &File, max_cols: u16) -> Option<(u16, u16)> {
        thumb_cells(file, self.picker.font_size(), MAX_COLS.min(max_cols), MAX_ROWS)
    }
}

/// The protocol a [`GRAPHICS`] name stands for (None for "auto" and unknown names).
fn protocol_named(name: &str) -> Option<ProtocolType> {
    match name.trim().to_ascii_lowercase().as_str() {
        "kitty" => Some(ProtocolType::Kitty),
        "sixel" => Some(ProtocolType::Sixel),
        "iterm2" => Some(ProtocolType::Iterm2),
        "halfblocks" | "half blocks" => Some(ProtocolType::Halfblocks),
        _ => None,
    }
}

/// Whether a primary device attributes reply (`ESC [ ? 63 ; 1 ; 2 ; 4 ; … c`) lists sixel (4).
fn da1_has_sixel(reply: &[u8]) -> bool {
    let Some(start) = reply.windows(3).position(|w| w == b"\x1b[?") else { return false };
    let rest = &reply[start + 3..];
    let Some(end) = rest.iter().position(|b| *b == b'c') else { return false };
    rest[..end].split(|b| *b == b';').any(|p| p == b"4")
}

/// Ask the terminal for its device attributes (in raw mode) and see whether it lists sixel;
/// gives up after a short wait.
#[cfg(unix)]
fn sixel_advertised() -> bool {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    use std::io::Write;
    let mut out = std::io::stdout();
    if out.write_all(b"\x1b[c").and_then(|_| out.flush()).is_err() {
        return false;
    }
    let stdin = rustix::stdio::stdin();
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    let mut reply = Vec::new();
    while !reply.ends_with(b"c") || !reply.windows(3).any(|w| w == b"\x1b[?") {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break;
        }
        let timeout = Timespec { tv_sec: left.as_secs() as _, tv_nsec: left.subsec_nanos() as _ };
        let mut fds = [PollFd::new(&stdin, PollFlags::IN)];
        if !matches!(poll(&mut fds, Some(&timeout)), Ok(n) if n > 0) {
            break;
        }
        let mut buf = [0u8; 64];
        match rustix::io::read(stdin, &mut buf) {
            Ok(n) if n > 0 => reply.extend_from_slice(&buf[..n]),
            _ => break,
        }
    }
    da1_has_sixel(&reply)
}

#[cfg(not(unix))]
fn sixel_advertised() -> bool {
    false
}

/// Fit the image's pixel size (from the file metadata, 4:3 if unknown) into
/// `max_cols` × `max_rows` cells without upscaling.
pub fn thumb_cells(file: &File, font: FontSize, max_cols: u16, max_rows: u16) -> Option<(u16, u16)> {
    if !file.is_image() || file.preview_url(360).is_none() {
        return None;
    }
    let (fw, fh) = (font.width.max(1) as f64, font.height.max(1) as f64);
    let (w, h) = match (file.original_w, file.original_h) {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w as f64, h as f64),
        _ => (4.0 * fw * max_cols as f64, 3.0 * fw * max_cols as f64),
    };
    let scale = (max_cols as f64 * fw / w).min(max_rows as f64 * fh / h).min(1.0);
    let cols = ((w * scale) / fw).ceil().clamp(1.0, max_cols as f64) as u16;
    let rows = ((h * scale) / fh).ceil().clamp(1.0, max_rows as f64) as u16;
    Some((cols, rows))
}

/// The kitty protocol draws a picture with these placeholder cells (ratatui-image's
/// `PLACEHOLDER`); only the first cell of each row says which row and column it is (with
/// diacritics), the ones after it are numbered from their left neighbour.
const PLACEHOLDER: char = '\u{10EEEE}';

/// Clear `area` for a popup, and the picture cells it cuts off: each cell shows its own slice of
/// a picture, so the cell left of the popup would be a stray half (an avatar's), and the cells
/// right of it lose their numbering and show the wrong slice.
pub fn clear_popup(f: &mut ratatui::Frame, area: ratatui::layout::Rect) {
    f.render_widget(ratatui::widgets::Clear, area);
    let buf = f.buffer_mut();
    let screen = buf.area;
    for y in area.top()..area.bottom().min(screen.bottom()) {
        if let Some(x) = area.left().checked_sub(1).filter(|x| *x >= screen.left()) {
            blank_picture_cell(&mut buf[(x, y)]);
        }
        let mut x = area.right();
        while x < screen.right() && is_continuation(buf[(x, y)].symbol()) {
            blank_picture_cell(&mut buf[(x, y)]);
            x += 1;
        }
    }
}

/// A placeholder numbered from its left neighbour (no diacritics after it).
fn is_continuation(symbol: &str) -> bool {
    symbol.split_once(PLACEHOLDER).is_some_and(|(_, marks)| marks.is_empty())
}

/// Turn a picture cell into a blank, keeping anything sent before the placeholder (the first
/// cell carries the picture itself, sent only once).
fn blank_picture_cell(cell: &mut ratatui::buffer::Cell) {
    if let Some((before, _)) = cell.symbol().split_once(PLACEHOLDER) {
        let blank = format!("{before} ");
        cell.set_symbol(&blank);
    }
}

/// Download, decode and encode a thumbnail for `file` at `size` cells (blocking parts run
/// off the UI task). The error's outermost context says what failed in a few words
/// ([`short_reason`]); the rest is the detail for the log.
pub async fn load_thumb(client: slack_core::Client, picker: Picker, file: File, size: (u16, u16)) -> anyhow::Result<SlicedProtocol> {
    use anyhow::Context;
    let px_width = size.0 as u32 * picker.font_size().width as u32;
    let url = file.preview_url(px_width).ok_or_else(|| anyhow::anyhow!("no preview"))?.to_string();
    let bytes = if file.id.starts_with(AVATAR_PREFIX) {
        // Public CDN pictures: fetched without the session.
        async { anyhow::Ok(client.http().get(&url).send().await?.error_for_status()?.bytes().await?.to_vec()) }.await
    } else {
        client.download(&url).await
    }
    .context("download failed")?;
    tokio::task::spawn_blocking(move || {
        let img = image::load_from_memory(&bytes).context("not a picture that can be shown")?;
        SlicedProtocol::new(&picker, img, Some(Size::new(size.0, size.1))).context("could not draw it")
    })
    .await?
}

/// The few words before the detail of a [`load_thumb`] error (`{:#}`: "context: detail").
pub fn short_reason(error: &str) -> &str {
    error.split(": ").next().unwrap_or(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: Option<u64>, h: Option<u64>) -> File {
        File { mimetype: "image/png".into(), thumb_360: Some("https://x/t.png".into()), original_w: w, original_h: h, ..Default::default() }
    }

    #[test]
    fn popups_cut_pictures_cleanly() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        // Two rows of a picture 6 cells wide as ratatui-image writes them: the first cell of a
        // row numbered (and the very first carrying the picture), the rest bare placeholders.
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 2));
        for y in 0..2u16 {
            for x in 0..6u16 {
                let symbol = match (x, y) {
                    (0, 0) => "\x1b_Gdata\x1b\\\u{10EEEE}\u{305}\u{305}".to_string(),
                    (0, _) => "\u{10EEEE}\u{30D}\u{305}".to_string(),
                    _ => "\u{10EEEE}".to_string(),
                };
                buf[(x, y)].set_symbol(&symbol);
            }
        }
        let backend = ratatui::backend::TestBackend::new(10, 2);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                *f.buffer_mut() = buf.clone();
                // A popup over cells 2..4 of both rows.
                clear_popup(f, Rect::new(2, 0, 2, 2));
                let b = f.buffer_mut();
                assert!(b[(0, 0)].symbol().contains('\u{10EEEE}'), "further left: untouched");
                assert_eq!(b[(1, 0)].symbol(), " ", "the half beside the popup is blanked");
                assert_eq!(b[(4, 1)].symbol(), " ");
                assert_eq!(b[(5, 1)].symbol(), " ", "the unnumbered rest is blanked");
                assert_eq!(b[(6, 1)].symbol(), " ", "past the picture: nothing to do");
            })
            .unwrap();
        // The cell carrying the picture keeps it when blanked.
        let mut cell = ratatui::buffer::Cell::default();
        cell.set_symbol("\x1b_Gdata\x1b\\\u{10EEEE}\u{305}");
        blank_picture_cell(&mut cell);
        assert_eq!(cell.symbol(), "\x1b_Gdata\x1b\\ ");
        assert!(is_continuation("\u{10EEEE}") && !is_continuation("\u{10EEEE}\u{305}") && !is_continuation("a"));
    }

    #[test]
    fn graphics_choices() {
        // xterm's real replies: as a VT340 (sixel) and by default.
        assert!(da1_has_sixel(b"\x1b[?63;1;2;4;6;9;15;16;22;28c"));
        assert!(!da1_has_sixel(b"\x1b[?64;1;2;6;9;15;16;17;18;21;22;28c"));
        assert!(!da1_has_sixel(b"\x1b[?63;1;2;14c"), "14 is not 4");
        assert!(!da1_has_sixel(b"\x1b[0n"), "no attributes");
        assert!(!da1_has_sixel(b"\x1b[?63;1;4"), "cut short");
        assert_eq!(protocol_named("Sixel"), Some(ProtocolType::Sixel));
        assert_eq!(protocol_named("auto"), None);
        assert!(GRAPHICS.iter().skip(1).all(|g| protocol_named(g).is_some()), "every choice but auto names a protocol");
    }

    #[test]
    fn avatars_only_from_slack() {
        let user = |url: &str| {
            let mut u = slack_core::types::User { id: "U1".into(), ..Default::default() };
            u.profile.image_48 = Some(url.into());
            u
        };
        let own = avatar_file(&user("https://avatars.slack-edge.com/2026-10-18/1_48.png")).unwrap();
        assert_eq!((own.id.as_str(), own.thumb_360.as_deref()), ("avatar:U1", Some("https://avatars.slack-edge.com/2026-10-18/1_48.png")));
        // Real default: a Gravatar URL with Slack's picture as the fallback.
        let gravatar = "https://secure.gravatar.com/avatar/9aca10.jpg?s=48&d=https%3A%2F%2Fa.slack-edge.com%2Fdf10d%2Fimg%2Favatars%2Fava_0010-48.png";
        assert_eq!(avatar_file(&user(gravatar)).unwrap().thumb_360.as_deref(), Some("https://a.slack-edge.com/df10d/img/avatars/ava_0010-48.png"));
        assert!(avatar_file(&user("https://secure.gravatar.com/avatar/x.jpg?s=48&d=identicon")).is_none());
        assert!(avatar_file(&user("https://evil.example/slack-edge.com/a.png")).is_none());
        assert!(avatar_file(&user("https://notslack-edge.com/a.png")).is_none());
        // The large picture has its own cache id; without one the small picture is used.
        let mut u = user("https://avatars.slack-edge.com/2026-10-18/1_48.png");
        assert_eq!(avatar_file_sized(&u, true).unwrap().id, "avatar:U1@192");
        u.profile.image_192 = Some("https://avatars.slack-edge.com/2026-10-18/1_192.png".into());
        assert_eq!(avatar_file_sized(&u, true).unwrap().thumb_360.as_deref(), Some("https://avatars.slack-edge.com/2026-10-18/1_192.png"));
    }

    #[test]
    fn sizing() {
        let font = FontSize::new(10, 20);
        // Large landscape: width-bound (36 cols = 360 px → 180 px tall = 9 rows).
        assert_eq!(thumb_cells(&image(Some(1600), Some(800)), font, 36, 10), Some((36, 9)));
        // Tall: height-bound (10 rows = 200 px → 100 px wide = 10 cols).
        assert_eq!(thumb_cells(&image(Some(400), Some(800)), font, 36, 10), Some((10, 10)));
        // Small images are not upscaled.
        assert_eq!(thumb_cells(&image(Some(50), Some(40)), font, 36, 10), Some((5, 2)));
        // Non-images get no box.
        let pdf = File { mimetype: "application/pdf".into(), ..Default::default() };
        assert_eq!(thumb_cells(&pdf, font, 36, 10), None);
    }
}

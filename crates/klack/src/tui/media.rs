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
    /// Draw profile pictures next to author names (real graphics only; `KLACK_AVATARS=0` turns them off).
    pub avatars: bool,
    /// What the terminal said it can do (for "auto").
    detected: ProtocolType,
}

/// The ways of drawing pictures to choose from ("auto": ask the terminal).
pub const GRAPHICS: &[&str] = &["auto", "kitty", "sixel", "iterm2", "halfblocks"];

/// Overrides the graphics setting.
pub const GRAPHICS_ENV: &str = "KLACK_GRAPHICS";

/// An avatar is two cells wide and one tall (about square).
pub const AVATAR_SIZE: (u16, u16) = (2, 1);
/// A two-row avatar ("Two-row profile pictures"), beside the name and the first line.
pub const AVATAR_SIZE_LARGE: (u16, u16) = (4, 2);
pub const AVATAR_PREFIX: &str = "avatar:";
/// Cache ids of custom emoji pictures, drawn in [`AVATAR_SIZE`] cells.
pub const EMOJI_PREFIX: &str = "emoji:";

/// The picture of the workspace's own emoji `name`.
pub fn emoji_file(ws: &super::state::Workspace, name: &str) -> Option<File> {
    picture_file(format!("{EMOJI_PREFIX}{name}"), super::text::custom_picture(ws, name)?)
}

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
    let id = if large { format!("{AVATAR_PREFIX}{}@192", user.id) } else { format!("{AVATAR_PREFIX}{}", user.id) };
    picture_file(id, source?)
}

/// The picture beside a message: its author's, else (bots, apps, webhooks) the icon it carries in
/// `bot_profile.icons` or `icons`.
pub fn message_avatar(ws: &super::state::Workspace, m: &slack_core::types::Message, large: bool) -> Option<File> {
    if let Some(file) = m.user.as_deref().and_then(|u| ws.users.get(u)).and_then(|u| avatar_file_sized(u, large)) {
        return Some(file);
    }
    let icons = m.bot_profile.as_ref().map(|b| &b["icons"]).filter(|i| i.is_object()).or(m.icons.as_ref())?;
    let keys: &[&str] = if large { &["image_192", "image_132", "image_72", "image_64", "image_48"] } else { &["image_48", "image_64", "image_72", "image_36"] };
    let source = keys.iter().find_map(|k| icons[*k].as_str())?;
    let who = m.bot_id.as_deref().or(m.username.as_deref())?;
    picture_file(format!("{AVATAR_PREFIX}bot:{who}{}", if large { "@large" } else { "" }), source)
}

/// A picture as a pseudo file for the thumbnail cache: only from Slack's CDN (a Gravatar URL's
/// `d=` fallback is Slack's default picture), never an SVG (it cannot be decoded). The id ends
/// with the picture's file name, so a changed picture (a new URL) is loaded afresh.
fn picture_file(id: String, source: &str) -> Option<File> {
    let url = slack_core::reqwest::Url::parse(source).ok()?;
    let slack_hosted = |u: &slack_core::reqwest::Url| u.scheme() == "https" && u.host_str().is_some_and(|h| h == "slack-edge.com" || h.ends_with(".slack-edge.com"));
    let url = if slack_hosted(&url) {
        url
    } else {
        let fallback = url.query_pairs().find(|(k, _)| k == "d").and_then(|(_, v)| slack_core::reqwest::Url::parse(&v).ok())?;
        slack_hosted(&fallback).then_some(fallback)?
    };
    if url.path().ends_with(".svg") {
        return None;
    }
    let name = url.path_segments().and_then(|mut s| s.next_back()).unwrap_or_default();
    let id = format!("{id}:{name}");
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
        let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
        if let Some(better) = override_for(picker.protocol_type(), &term_program) {
            tracing::info!("{term_program} answers the kitty query but cannot draw kitty's placeholders: using {better:?}");
            picker.set_protocol_type(better);
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

    /// Draw pictures as `choice` says ("auto": as detected; `KLACK_GRAPHICS` wins);
    /// pictures made the other way are dropped.
    pub fn choose(&mut self, choice: &str) {
        let env = std::env::var(GRAPHICS_ENV).ok().filter(|v| !v.trim().is_empty());
        let protocol = protocol_named(env.as_deref().unwrap_or(choice)).unwrap_or(self.detected);
        if protocol != self.picker.protocol_type() {
            self.thumbs.clear();
        }
        self.picker.set_protocol_type(protocol);
        self.avatars = protocol != ProtocolType::Halfblocks && std::env::var("KLACK_AVATARS").map_or(true, |v| v != "0");
    }

    /// The terminal draws real pictures (not half blocks).
    pub fn graphics(&self) -> bool {
        self.picker.protocol_type() != ProtocolType::Halfblocks
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
#[cfg(any(unix, test))]
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
    // No room at all (a pane squeezed to nothing): no picture rather than an inverted range.
    if !file.is_image() || file.preview_url(360).is_none() || max_cols == 0 || max_rows == 0 {
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

/// A better protocol than the one detected, for terminals that answer the kitty graphics query
/// without drawing kitty's Unicode placeholders (which ratatui-image uses): WezTerm, whose
/// iTerm2 protocol works. `term_program` is `$TERM_PROGRAM`.
fn override_for(detected: ProtocolType, term_program: &str) -> Option<ProtocolType> {
    (detected == ProtocolType::Kitty && term_program == "WezTerm").then_some(ProtocolType::Iterm2)
}

/// The kitty protocol draws a picture with these placeholder cells (ratatui-image's
/// `PLACEHOLDER`); only the first cell of each row says which row and column it is (with
/// diacritics), the ones after it are numbered from their left neighbour.
const PLACEHOLDER: char = '\u{10EEEE}';

/// Clear `area` for a popup, and the picture cells it cuts off: each cell shows its own slice of
/// a picture, so the cell left of the popup would be a stray half (an avatar's), and the cells
/// right of it lose their numbering and show the wrong slice.
pub fn clear_popup(f: &mut ratatui::Frame, area: ratatui::layout::Rect) {
    hold_pictures(f.buffer_mut(), area);
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

thread_local! {
    /// Picture data that popups covered in this frame, put back by [`reattach_pictures`].
    static HELD: std::cell::RefCell<Vec<(u16, u16, String)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Keep the picture data in the cells of `area` (sent before the first placeholder): kitty
/// gets each picture once, in the first frame that draws it, so a picture first drawn under a
/// popup would never show.
fn hold_pictures(buf: &ratatui::buffer::Buffer, area: ratatui::layout::Rect) {
    let area = area.intersection(buf.area);
    HELD.with_borrow_mut(|held| {
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if let Some((data, _)) = buf[(x, y)].symbol().split_once(PLACEHOLDER).filter(|(data, _)| !data.is_empty()) {
                    held.push((x, y, data.to_string()));
                }
            }
        }
    });
}

/// Once the frame is drawn, put the held picture data back in front of whatever covers its
/// cell (a virtual placement: it shows nothing there), or of the wide character it is part of.
pub fn reattach_pictures(buf: &mut ratatui::buffer::Buffer) {
    use unicode_width::UnicodeWidthStr;
    let held: Vec<(u16, u16, String)> = HELD.take().into_iter().filter(|(x, y, _)| buf.area.contains((*x, *y).into())).collect();
    // Where each goes, before any is attached (the data would count into the widths).
    let at: Vec<u16> = held.iter().map(|(x, y, _)| (1..=x - buf.area.left()).find(|&k| buf[(x - k, *y)].symbol().width() > k as usize).map_or(*x, |k| x - k)).collect();
    for ((_, y, data), x) in held.into_iter().zip(at) {
        let cell = &mut buf[(x, y)];
        // The terminal shows only what was there: keep its width for ratatui's diff.
        let width = std::num::NonZeroU16::new(cell.symbol().width().max(1) as u16).unwrap_or(std::num::NonZeroU16::MIN);
        let symbol = format!("{data}{}", cell.symbol());
        cell.set_symbol(&symbol).set_diff_option(ratatui::buffer::CellDiffOption::ForcedWidth(width));
    }
}

/// [`clear_popup`] for a dialog: everything else on screen fades first (dim colour and the
/// terminal's dim attribute), so the dialog stands out. Picture cells are left alone — kitty
/// tells pictures apart by their cells' foreground colour.
pub fn clear_modal(f: &mut ratatui::Frame, area: ratatui::layout::Rect) {
    fade_outside(f.buffer_mut(), area);
    clear_popup(f, area);
}

fn fade_outside(buf: &mut ratatui::buffer::Buffer, keep: ratatui::layout::Rect) {
    let dim = super::theme::p().dim;
    let screen = buf.area;
    for y in screen.top()..screen.bottom() {
        for x in screen.left()..screen.right() {
            if keep.contains((x, y).into()) {
                continue;
            }
            let cell = &mut buf[(x, y)];
            if cell.symbol().contains(PLACEHOLDER) {
                continue;
            }
            cell.set_fg(dim);
            cell.modifier.insert(ratatui::style::Modifier::DIM);
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
        let mut img = image::load_from_memory(&bytes).context("not a picture that can be shown")?;
        if file.id.starts_with(AVATAR_PREFIX) {
            // Profile pictures fill their cells (cells are not square): cut, not boxed.
            let font = picker.font_size();
            img = img.resize_to_fill(size.0 as u32 * font.width as u32, size.1 as u32 * font.height as u32, image::imageops::FilterType::Triangle);
        }
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
    fn pictures_first_drawn_under_a_popup_still_reach_the_terminal() {
        use ratatui::layout::Rect;
        use ratatui::widgets::Paragraph;
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(10, 3)).unwrap();
        terminal
            .draw(|f| {
                // A picture's first cell (carrying its data) at (2, 1), then a popup with text over it.
                f.buffer_mut()[(2, 1)].set_symbol("\x1b_Gdata\x1b\\\u{10EEEE}\u{305}\u{305}");
                f.buffer_mut()[(6, 2)].set_symbol("\x1b_Gwide\x1b\\\u{10EEEE}\u{305}\u{305}");
                clear_popup(f, Rect::new(1, 0, 8, 3));
                f.render_widget(Paragraph::new(vec!["".into(), " hello".into(), "    界界".into()]), Rect::new(1, 0, 8, 3));
                reattach_pictures(f.buffer_mut());
                let b = f.buffer_mut();
                assert_eq!(b[(2, 1)].symbol(), "\x1b_Gdata\x1b\\h", "the data, then what the popup drew there");
                assert_eq!((b[(5, 2)].symbol(), b[(6, 2)].symbol()), ("\x1b_Gwide\x1b\\界", " "), "in front of the wide character it is part of");
            })
            .unwrap();
        // What reaches the terminal: nothing after the data is skipped.
        let row = |y: u16| (0..10).map(|x| terminal.backend().buffer()[(x, y)].symbol().to_string()).collect::<String>();
        assert_eq!(row(1), "  \x1b_Gdata\x1b\\hello   ");
        assert_eq!(row(2), "     \x1b_Gwide\x1b\\界 界  ", "(a test terminal keeps a blank after a wide character)");
        terminal
            .draw(|f| {
                reattach_pictures(f.buffer_mut());
                assert_eq!(f.buffer_mut()[(2, 1)].symbol(), " ", "held for one frame only");
            })
            .unwrap();
    }

    #[test]
    fn no_room_no_thumbnail() {
        let font = FontSize::new(10, 20);
        assert_eq!(thumb_cells(&image(Some(800), Some(500)), font, 0, 10), None, "a pane with no columns left");
        assert_eq!(thumb_cells(&image(Some(800), Some(500)), font, 10, 0), None);
        assert_eq!(thumb_cells(&image(Some(800), Some(500)), font, 1, 1), Some((1, 1)));
    }

    #[test]
    fn dialogs_fade_the_rest() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        use ratatui::style::{Color, Modifier};
        let mut buf = Buffer::empty(Rect::new(0, 0, 6, 3));
        for x in 0..6 {
            buf[(x, 1)].set_symbol("x").set_fg(Color::Red);
        }
        buf[(0, 0)].set_symbol("\u{10EEEE}").set_fg(Color::Rgb(0, 0, 7));
        fade_outside(&mut buf, Rect::new(2, 1, 2, 1));
        assert!(buf[(0, 1)].modifier.contains(Modifier::DIM) && buf[(0, 1)].fg != Color::Red, "outside: faded");
        assert_eq!((buf[(2, 1)].fg, buf[(2, 1)].modifier), (Color::Red, Modifier::empty()), "the dialog's area: untouched");
        assert_eq!(buf[(0, 0)].fg, Color::Rgb(0, 0, 7), "picture cells keep their colour (it names the picture)");
    }

    #[test]
    fn graphics_choices() {
        // xterm's real replies: as a VT340 (sixel) and by default.
        assert_eq!(override_for(ProtocolType::Kitty, "WezTerm"), Some(ProtocolType::Iterm2), "WezTerm: no kitty placeholders");
        assert_eq!(override_for(ProtocolType::Kitty, "kitty"), None);
        assert_eq!(override_for(ProtocolType::Kitty, ""), None);
        assert_eq!(override_for(ProtocolType::Sixel, "WezTerm"), None, "only the kitty answer is overridden");
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
        assert_eq!((own.id.as_str(), own.thumb_360.as_deref()), ("avatar:U1:1_48.png", Some("https://avatars.slack-edge.com/2026-10-18/1_48.png")));
        // Real default: a Gravatar URL with Slack's picture as the fallback.
        let gravatar = "https://secure.gravatar.com/avatar/9aca10.jpg?s=48&d=https%3A%2F%2Fa.slack-edge.com%2Fdf10d%2Fimg%2Favatars%2Fava_0010-48.png";
        assert_eq!(avatar_file(&user(gravatar)).unwrap().thumb_360.as_deref(), Some("https://a.slack-edge.com/df10d/img/avatars/ava_0010-48.png"));
        assert!(avatar_file(&user("https://secure.gravatar.com/avatar/x.jpg?s=48&d=identicon")).is_none());
        assert!(avatar_file(&user("https://evil.example/slack-edge.com/a.png")).is_none());
        assert!(avatar_file(&user("https://notslack-edge.com/a.png")).is_none());
        assert!(avatar_file(&user("https://a.slack-edge.com/9cc0056/img/avatars/slack_system_user/slack_logo_mark.svg")).is_none(), "SVG (the Slack system user's): initials instead");
        // The large picture has its own cache id; without one the small picture is used.
        let mut u = user("https://avatars.slack-edge.com/2026-10-18/1_48.png");
        assert_eq!(avatar_file_sized(&u, true).unwrap().id, "avatar:U1@192:1_48.png");
        u.profile.image_192 = Some("https://avatars.slack-edge.com/2026-10-18/1_192.png".into());
        assert_eq!(avatar_file_sized(&u, true).unwrap().thumb_360.as_deref(), Some("https://avatars.slack-edge.com/2026-10-18/1_192.png"));
        // A new picture is a new URL, so a new cache entry (not the old picture).
        let changed = avatar_file(&user("https://avatars.slack-edge.com/2026-10-19/2_48.png")).unwrap();
        assert_ne!(changed.id, own.id);
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

    #[test]
    fn bots_and_apps_get_their_icons() {
        let ws = super::super::state::Workspace::default();
        let bot = |v: serde_json::Value| -> slack_core::types::Message { serde_json::from_value(v).unwrap() };
        // An app's message (bot_profile, as Slack documents it) and a webhook post (icons).
        let app = bot(serde_json::json!({"ts": "1.0", "subtype": "bot_message", "bot_id": "B1", "bot_profile": {"id": "B1", "name": "deploys", "icons": {"image_36": "https://avatars.slack-edge.com/b_36.png", "image_48": "https://avatars.slack-edge.com/b_48.png", "image_72": "https://avatars.slack-edge.com/b_72.png"}}}));
        let small = message_avatar(&ws, &app, false).unwrap();
        assert_eq!((small.id.as_str(), small.thumb_360.as_deref()), ("avatar:bot:B1:b_48.png", Some("https://avatars.slack-edge.com/b_48.png")));
        assert_eq!(message_avatar(&ws, &app, true).unwrap().thumb_360.as_deref(), Some("https://avatars.slack-edge.com/b_72.png"), "the larger icon for two-row pictures");
        let hook = bot(serde_json::json!({"ts": "2.0", "subtype": "bot_message", "bot_id": "B2", "username": "ci", "icons": {"image_48": "https://avatars.slack-edge.com/h_48.png"}}));
        assert_eq!(message_avatar(&ws, &hook, false).unwrap().thumb_360.as_deref(), Some("https://avatars.slack-edge.com/h_48.png"));
        let elsewhere = bot(serde_json::json!({"ts": "3.0", "bot_id": "B3", "icons": {"image_48": "https://example.com/i.png"}}));
        assert!(message_avatar(&ws, &elsewhere, false).is_none(), "nothing fetched from third parties");
        let emoji_only = bot(serde_json::json!({"ts": "4.0", "bot_id": "B4", "icons": {"emoji": ":robot_face:"}}));
        assert!(message_avatar(&ws, &emoji_only, false).is_none());
        // The real system message from the testbed: USLACK, bot B01, no icons.
        let system = bot(serde_json::json!({"ts": "5.0", "user": "USLACK", "subtype": "bot_message", "bot_id": "B01"}));
        assert!(message_avatar(&ws, &system, false).is_none());
    }
}

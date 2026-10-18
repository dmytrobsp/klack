//! Inline image thumbnails: terminal graphics detection, sizing and a per-file cache.

use std::collections::HashMap;

use ratatui::layout::Size;
use ratatui_image::FontSize;
use ratatui_image::picker::Picker;
use ratatui_image::sliced::SlicedProtocol;
use slack_core::types::File;

/// Largest thumbnail, in cells.
pub const MAX_COLS: u16 = 36;
pub const MAX_ROWS: u16 = 10;

pub enum Thumb {
    Loading,
    Ready(SlicedProtocol),
    Failed,
}

pub struct Media {
    pub picker: Picker,
    pub thumbs: HashMap<String, Thumb>,
    /// Draw profile pictures next to author names (real graphics only; `SLACKTUI_AVATARS=0` turns them off).
    pub avatars: bool,
}

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
    /// mode, before reading events); falls back to half-block characters.
    pub fn detect() -> Media {
        let picker = Picker::from_query_stdio().unwrap_or_else(|e| {
            tracing::warn!("terminal graphics query failed, using half blocks: {e}");
            Picker::halfblocks()
        });
        tracing::info!(protocol = ?picker.protocol_type(), font = ?picker.font_size(), "terminal graphics");
        let avatars = picker.protocol_type() != ratatui_image::picker::ProtocolType::Halfblocks && std::env::var("SLACKTUI_AVATARS").map_or(true, |v| v != "0");
        Media { picker, thumbs: HashMap::new(), avatars }
    }

    /// Cells to reserve under an image file's line when at most `max_cols` wide, or None
    /// for non-images.
    pub fn thumb_size(&self, file: &File, max_cols: u16) -> Option<(u16, u16)> {
        thumb_cells(file, self.picker.font_size(), MAX_COLS.min(max_cols), MAX_ROWS)
    }
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

/// Download, decode and encode a thumbnail for `file` at `size` cells (blocking parts run
/// off the UI task).
pub async fn load_thumb(client: slack_core::Client, picker: Picker, file: File, size: (u16, u16)) -> anyhow::Result<SlicedProtocol> {
    let px_width = size.0 as u32 * picker.font_size().width as u32;
    let url = file.preview_url(px_width).ok_or_else(|| anyhow::anyhow!("no preview"))?.to_string();
    let bytes = if file.id.starts_with(AVATAR_PREFIX) {
        // Public CDN pictures: fetched without the session.
        client.http().get(&url).send().await?.error_for_status()?.bytes().await?.to_vec()
    } else {
        client.download(&url).await?
    };
    tokio::task::spawn_blocking(move || {
        let img = image::load_from_memory(&bytes)?;
        Ok(SlicedProtocol::new(&picker, img, Some(Size::new(size.0, size.1)))?)
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: Option<u64>, h: Option<u64>) -> File {
        File { mimetype: "image/png".into(), thumb_360: Some("https://x/t.png".into()), original_w: w, original_h: h, ..Default::default() }
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

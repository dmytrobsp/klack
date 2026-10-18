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
        Media { picker, thumbs: HashMap::new() }
    }

    /// Cells to reserve under an image file's line, or None for non-images.
    pub fn thumb_size(&self, file: &File) -> Option<(u16, u16)> {
        thumb_cells(file, self.picker.font_size(), MAX_COLS, MAX_ROWS)
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
    let bytes = client.download(&url).await?;
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

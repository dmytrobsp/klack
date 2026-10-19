//! System clipboard (arboard), falling back to the terminal's OSC 52 escape when no
//! clipboard is reachable (e.g. over SSH).

use std::io::Write;
use std::path::PathBuf;

/// What a paste found on the system clipboard.
#[derive(Debug, PartialEq)]
pub enum Pasted {
    /// Files copied in a file manager.
    Files(Vec<PathBuf>),
    Text(String),
    /// A picture (a screenshot, an image copied in a browser), as RGBA pixels.
    Image { width: usize, height: usize, rgba: Vec<u8> },
    /// Nothing that can be pasted; says why.
    Nothing(&'static str),
}

#[derive(Default)]
pub struct Clipboard {
    /// Kept alive: on Linux the owning instance serves the contents to other apps.
    system: Option<arboard::Clipboard>,
    /// Last text placed on the system clipboard, for `hand_off`.
    last: Option<String>,
    /// The system clipboard could not be reached; not tried again (it can take seconds).
    unavailable: bool,
}

impl Clipboard {
    fn system(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.system.is_none() && !self.unavailable {
            self.system = arboard::Clipboard::new().map_err(|e| tracing::warn!("no system clipboard: {e}")).ok();
            self.unavailable = self.system.is_none();
        }
        self.system.as_mut()
    }

    /// Copy `text`; returns how it was copied.
    pub fn copy(&mut self, text: &str) -> &'static str {
        if let Some(cb) = self.system() {
            match cb.set_text(text) {
                Ok(()) => {
                    self.last = Some(text.to_string());
                    return "clipboard";
                }
                Err(e) => tracing::warn!("clipboard copy failed: {e}"),
            }
        }
        osc52(text);
        "terminal clipboard (OSC 52)"
    }

    /// Read the clipboard for a paste: files first (a file manager offers their paths as text
    /// too), then text (some apps offer a picture of copied text as well), then a picture.
    pub fn paste(&mut self) -> Pasted {
        let Some(cb) = self.system() else { return Pasted::Nothing("no system clipboard here") };
        if let Some(files) = cb.get().file_list().ok().filter(|f| !f.is_empty()) {
            return Pasted::Files(files);
        }
        if let Some(text) = cb.get_text().ok().filter(|t| !t.is_empty()) {
            return Pasted::Text(text);
        }
        match cb.get_image() {
            Ok(image) => Pasted::Image { width: image.width, height: image.height, rgba: image.bytes.into_owned() },
            Err(arboard::Error::ContentNotAvailable) => Pasted::Nothing("the clipboard is empty or holds nothing klack can paste"),
            Err(e) => {
                tracing::warn!("clipboard paste failed: {e}");
                Pasted::Nothing("the clipboard could not be read")
            }
        }
    }

    /// Before exiting: our copy dies with the process on Linux, so if the clipboard still
    /// holds it, hand it to the terminal via OSC 52 (ignored by terminals without support).
    pub fn hand_off(&mut self) {
        let (Some(cb), Some(last)) = (self.system.as_mut(), self.last.take()) else { return };
        if cb.get_text().is_ok_and(|t| t == last) {
            osc52(&last);
        }
    }
}

/// A pasted picture as a PNG file's bytes.
pub fn png(width: usize, height: usize, rgba: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    let image = image::RgbaImage::from_raw(width as u32, height as u32, rgba).ok_or_else(|| anyhow::anyhow!("a {width}×{height} picture with the wrong number of pixels"))?;
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}

fn osc52(text: &str) {
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()));
    let _ = out.flush();
}

fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_padding() {
        assert_eq!(super::base64(b""), "");
        assert_eq!(super::base64(b"f"), "Zg==");
        assert_eq!(super::base64(b"fo"), "Zm8=");
        assert_eq!(super::base64(b"foo"), "Zm9v");
        assert_eq!(super::base64("héllo".as_bytes()), "aMOpbGxv");
    }

    #[test]
    fn pasted_pictures_become_png() {
        let rgba = vec![255, 0, 0, 255, 0, 0, 255, 128];
        let png = super::png(2, 1, rgba.clone()).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        assert_eq!(image::load_from_memory(&png).unwrap().to_rgba8().into_raw(), rgba);
        assert!(super::png(2, 2, rgba).is_err(), "too few pixels");
    }
}

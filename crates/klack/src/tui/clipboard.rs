//! System clipboard (arboard), falling back to the terminal's OSC 52 escape when no
//! clipboard is reachable (e.g. over SSH).

use std::io::Write;

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
    /// Copy `text`; returns how it was copied.
    pub fn copy(&mut self, text: &str) -> &'static str {
        if self.system.is_none() && !self.unavailable {
            self.system = arboard::Clipboard::new().map_err(|e| tracing::warn!("no system clipboard: {e}")).ok();
            self.unavailable = self.system.is_none();
        }
        if let Some(cb) = self.system.as_mut() {
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

    /// Before exiting: our copy dies with the process on Linux, so if the clipboard still
    /// holds it, hand it to the terminal via OSC 52 (ignored by terminals without support).
    pub fn hand_off(&mut self) {
        let (Some(cb), Some(last)) = (self.system.as_mut(), self.last.take()) else { return };
        if cb.get_text().is_ok_and(|t| t == last) {
            osc52(&last);
        }
    }
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
}

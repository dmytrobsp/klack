//! The QR code of the sign-in link and the layout of the "Sign in with a code" screen: pure, so the
//! guide only prints the lines.
// Task Y3 prints `code_screen` in the sign-in guide and removes this.
#![cfg_attr(not(test), allow(dead_code))]

use qrcode::{Color, EcLevel, QrCode};
use unicode_width::UnicodeWidthStr;

/// The modules of the QR code of `text` (dark = true), error correction M, with a 4-module quiet zone of
/// light on every side. Square. Empty when the text does not fit a code.
pub fn modules(text: &str) -> Vec<Vec<bool>> {
    let Ok(code) = QrCode::with_error_correction_level(text, EcLevel::M) else { return Vec::new() };
    let w = code.width();
    let colors = code.to_colors();
    let size = w + 8;
    let mut rows = vec![vec![false; size]; size];
    for y in 0..w {
        for x in 0..w {
            rows[y + 4][x + 4] = colors[y * w + x] == Color::Dark;
        }
    }
    rows
}

/// One terminal line per two module rows: `▀` with the top module as foreground and the bottom as background, dark
/// as black and light as bright white, so it scans on dark themes too. An odd last row is padded with light.
pub fn half_blocks(modules: &[Vec<bool>]) -> Vec<String> {
    let mut lines = Vec::new();
    for pair in modules.chunks(2) {
        let mut line = String::new();
        for x in 0..pair[0].len() {
            let top = pair[0][x];
            let bottom = pair.get(1).is_some_and(|r| r[x]);
            line.push_str(if top { "\x1b[30m" } else { "\x1b[97m" });
            line.push_str(if bottom { "\x1b[40m" } else { "\x1b[107m" });
            line.push('▀');
        }
        line.push_str("\x1b[0m");
        lines.push(line);
    }
    lines
}

/// An OSC 8 hyperlink whose visible text is the address itself.
pub fn osc8(url: &str) -> String {
    format!("\x1b]8;;{url}\x1b\\{url}\x1b]8;;\x1b\\")
}

/// Where the text goes relative to the code, or no code at all when the terminal is too small.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Fit {
    Beside,
    Under,
    LinkOnly,
}

/// Beside needs the code, a gap of 3 and the text on one line; under needs the code's width; both need the code's
/// rows plus 6 (heading, email, blank, code prompt, hint, margin).
pub fn fit(qr_cols: usize, qr_rows: usize, text_cols: usize, width: u16, height: u16) -> Fit {
    let (width, height) = (width as usize, height as usize);
    if qr_rows + 6 > height {
        Fit::LinkOnly
    } else if qr_cols + 3 + text_cols <= width {
        Fit::Beside
    } else if qr_cols <= width {
        Fit::Under
    } else {
        Fit::LinkOnly
    }
}

/// The lines of the code screen for a terminal of `width` x `height` (heading, email, blank, the code with its
/// text, as far as it fits).
pub fn code_screen(email: &str, width: u16, height: u16) -> Vec<String> {
    let link = slack_core::signin::signin_link(email);
    let intro = "On your phone, scan this — or open";
    let text: Vec<(String, usize)> = vec![
        (intro.to_string(), intro.width()),
        (osc8(&link), link.width()),
        ("Slack emails you a code. Type it here;".to_string(), 38),
        ("don't enter it on the phone.".to_string(), 28),
    ];
    let text_cols = text.iter().map(|t| t.1).max().unwrap_or(0);
    let mut lines = vec!["Sign in with a code".to_string(), format!("  Email: {email}"), String::new()];
    let m = modules(&link);
    let (qr, qr_cols) = (half_blocks(&m), m.len());
    match if qr.is_empty() { Fit::LinkOnly } else { fit(qr_cols, qr.len(), text_cols, width, height) } {
        Fit::Beside => {
            for (i, q) in qr.iter().enumerate() {
                match text.get(i) {
                    Some((t, _)) => lines.push(format!("{q}   {t}")),
                    None => lines.push(q.clone()),
                }
            }
        }
        Fit::Under => {
            lines.extend(qr);
            lines.push(String::new());
            lines.extend(text.into_iter().map(|t| t.0));
        }
        Fit::LinkOnly => lines.extend(text.into_iter().skip(1).map(|t| t.0)),
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_code_has_its_quiet_zone_and_finders() {
        let m = modules("https://slack.com/signin?email=person%40example.com");
        assert!(!m.is_empty() && m.iter().all(|r| r.len() == m.len()));
        let n = m.len();
        assert!(m[..4].iter().all(|r| r.iter().all(|d| !d)));
        assert!(m[n - 4..].iter().all(|r| r.iter().all(|d| !d)));
        assert!((4..=10).all(|x| m[4][x]));
        assert!(!m[5][5]);
    }

    #[test]
    fn two_rows_make_one_line() {
        let l = half_blocks(&[vec![true, false], vec![false, true]]);
        assert_eq!(l, vec!["\x1b[30m\x1b[107m▀\x1b[97m\x1b[40m▀\x1b[0m".to_string()]);
        let l = half_blocks(&[vec![true], vec![true], vec![true]]);
        assert_eq!(l.len(), 2);
        assert_eq!(l[1], "\x1b[30m\x1b[107m▀\x1b[0m");
    }

    #[test]
    fn the_link_is_its_own_text() {
        assert_eq!(osc8("https://x.io/a"), "\x1b]8;;https://x.io/a\x1b\\https://x.io/a\x1b]8;;\x1b\\");
    }

    #[test]
    fn the_screen_fits_the_terminal() {
        assert_eq!(fit(37, 19, 50, 95, 40), Fit::Beside);
        assert_eq!(fit(37, 19, 50, 80, 40), Fit::Under);
        assert_eq!(fit(37, 19, 50, 30, 40), Fit::LinkOnly);
        assert_eq!(fit(37, 19, 50, 95, 20), Fit::LinkOnly);
        let s = code_screen("person@example.com", 30, 40).join("\n");
        assert!(s.contains(&osc8("https://slack.com/signin?email=person%40example.com")));
        assert!(!s.contains('▀'));
        assert!(code_screen("person@example.com", 120, 40).join("\n").contains('▀'));
    }

    /// Prints the screen for the live check: `KLACK_QR_OUT=file cargo test -p klack qr_print -- --ignored`.
    #[test]
    #[ignore]
    fn qr_print() {
        let out = std::env::var("KLACK_QR_OUT").unwrap();
        std::fs::write(out, code_screen("person@example.com", 100, 40).join("\n") + "\n").unwrap();
    }
}

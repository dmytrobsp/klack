//! The QR code of the sign-in link and the layout of the "Sign in with a code" screen: pure, so the
//! guide only prints the lines.

use qrcode::{Color, EcLevel, QrCode};
use unicode_width::UnicodeWidthStr;

/// The modules of the QR code of `text` (dark = true), error correction M, with a 4-module quiet zone of
/// light on every side. Square. Empty when the text does not fit a code.
pub fn modules(text: &str) -> Vec<Vec<bool>> {
    modules_with(text, EcLevel::M, 4)
}

/// The modules of the compact layout's code: error correction L and a 2-module quiet zone, so it fits a terminal of
/// 80 x 24.
pub fn compact_modules(text: &str) -> Vec<Vec<bool>> {
    modules_with(text, EcLevel::L, 2)
}

/// The modules of the QR code of `text` at error correction `ec`, with a quiet zone of `quiet` modules.
fn modules_with(text: &str, ec: EcLevel, quiet: usize) -> Vec<Vec<bool>> {
    let Ok(code) = QrCode::with_error_correction_level(text, ec) else { return Vec::new() };
    let w = code.width();
    let colors = code.to_colors();
    let size = w + 2 * quiet;
    let mut rows = vec![vec![false; size]; size];
    for y in 0..w {
        for x in 0..w {
            rows[y + quiet][x + quiet] = colors[y * w + x] == Color::Dark;
        }
    }
    rows
}

/// One terminal line per two module rows: `▀` with the top module as foreground and the bottom as background, dark
/// as 24-bit black and light as 24-bit white (not palette colours, which follow the theme), so it scans on dark themes too. An odd last row is padded with light.
pub fn half_blocks(modules: &[Vec<bool>]) -> Vec<String> {
    let mut lines = Vec::new();
    for pair in modules.chunks(2) {
        let mut line = String::new();
        for x in 0..pair[0].len() {
            let top = pair[0][x];
            let bottom = pair.get(1).is_some_and(|r| r[x]);
            line.push_str(if top { "\x1b[38;2;0;0;0m" } else { "\x1b[38;2;255;255;255m" });
            line.push_str(if bottom { "\x1b[48;2;0;0;0m" } else { "\x1b[48;2;255;255;255m" });
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
    /// One header line, a smaller code ([`compact_modules`]) and the link: for a terminal of 80 x 24.
    Compact,
    LinkOnly,
}

/// Lines above the code: the heading, the email and a blank.
const HEADER_LINES: usize = 3;
/// Lines the guide prints below the code screen: a hint or message line, then the code prompt.
pub const PROMPT_LINES: usize = 2;
/// Lines of text beside or under the code (the intro, the link and the two hint lines).
const TEXT_LINES: usize = 4;

/// Beside needs the code, a gap of 3 and the text on one line across, and the header, the code and the prompt
/// lines down; under needs the code's width across, and down the header, the code, a blank, the text and the
/// prompt lines. Each layout's own check decides; otherwise only the link is shown.
pub fn fit(qr_cols: usize, qr_rows: usize, text_cols: usize, width: u16, height: u16) -> Fit {
    let (width, height) = (width as usize, height as usize);
    if qr_cols + 3 + text_cols <= width && HEADER_LINES + qr_rows + PROMPT_LINES <= height {
        Fit::Beside
    } else if qr_cols <= width && HEADER_LINES + qr_rows + 1 + TEXT_LINES + PROMPT_LINES <= height {
        Fit::Under
    } else {
        Fit::LinkOnly
    }
}

/// Whether the compact layout fits: the code's width across, and down its header line, the code, the link (as many
/// lines as it wraps to at this width) and the prompt lines.
pub fn compact_fits(qr_cols: usize, qr_rows: usize, link_cols: usize, width: u16, height: u16) -> bool {
    let (width, height) = (width as usize, height as usize);
    qr_cols <= width && 1 + qr_rows + link_cols.div_ceil(width.max(1)) + PROMPT_LINES <= height
}

/// The layout of the code screen of `link` on a terminal `width` x `height`: a full one when it fits ([`fit`]), else
/// the compact one when that fits, else the link only. `text_cols`: the widest line of the full layouts' text.
fn layout(link: &str, text_cols: usize, width: u16, height: u16) -> Fit {
    let m = modules(link);
    let full = if m.is_empty() { Fit::LinkOnly } else { fit(m.len(), m.len().div_ceil(2), text_cols, width, height) };
    if full != Fit::LinkOnly {
        return full;
    }
    let c = compact_modules(link);
    if !c.is_empty() && compact_fits(c.len(), c.len().div_ceil(2), link.width(), width, height) { Fit::Compact } else { Fit::LinkOnly }
}

/// The lines of the code screen for a terminal of `width` x `height` (heading, email, blank, the code with its
/// text, as far as it fits; on a small terminal one header line, a smaller code and the link; on a smaller one the
/// link and a dim line saying that a bigger window shows the code).
pub fn code_screen(email: &str, width: u16, height: u16) -> Vec<String> {
    let link = slack_core::signin::signin_link(email);
    let intro = "On your phone, scan this — or open";
    let (hint1, hint2) = ("Slack emails you a code. Type it here;", "don't enter it on the phone.");
    let text: [(String, usize); TEXT_LINES] = [
        (intro.to_string(), intro.width()),
        (osc8(&link), link.width()),
        (hint1.to_string(), hint1.width()),
        (hint2.to_string(), hint2.width()),
    ];
    let text_cols = text.iter().map(|t| t.1).max().unwrap_or(0);
    let chosen = layout(&link, text_cols, width, height);
    if chosen == Fit::Compact {
        let mut lines = vec![format!("Sign in with a code · {email}")];
        lines.extend(half_blocks(&compact_modules(&link)));
        lines.push(osc8(&link));
        return lines;
    }
    let mut lines = vec!["Sign in with a code".to_string(), format!("  Email: {email}"), String::new()];
    let qr = half_blocks(&modules(&link));
    match chosen {
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
        Fit::Compact | Fit::LinkOnly => {
            lines.extend(text.into_iter().skip(1).map(|t| t.0));
            lines.push("\x1b[2ma bigger window shows a QR code\x1b[0m".to_string());
        }
    }
    lines
}

/// The text without its SGR and OSC 8 escapes.
fn visible(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\x1b' {
            out.push(c);
        } else if it.peek() == Some(&'[') {
            for d in it.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            // OSC: up to the string terminator ESC \
            while let Some(d) = it.next() {
                if d == '\x1b' {
                    it.next();
                    break;
                }
            }
        }
    }
    out
}

/// How many terminal lines `lines` take on a terminal `width` columns wide, the long ones wrapped.
pub fn rows_on(lines: &[String], width: u16) -> usize {
    let width = (width as usize).max(1);
    lines.iter().map(|l| visible(l).width().div_ceil(width).max(1)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_code_has_its_quiet_zone_and_finders() {
        let m = modules("https://app.slack.com/get-started?email=person%40example.com#/createnew");
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
        assert_eq!(l, vec!["\x1b[38;2;0;0;0m\x1b[48;2;255;255;255m▀\x1b[38;2;255;255;255m\x1b[48;2;0;0;0m▀\x1b[0m".to_string()]);
        let l = half_blocks(&[vec![true], vec![true], vec![true]]);
        assert_eq!(l.len(), 2);
        assert_eq!(l[1], "\x1b[38;2;0;0;0m\x1b[48;2;255;255;255m▀\x1b[0m");
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
        assert!(s.contains(&osc8("https://app.slack.com/get-started?email=person%40example.com#/createnew")));
        assert!(!s.contains('▀'));
        // Beside needs 3 + rows + 2 lines, under 3 + rows + 1 + 4 + 2.
        assert_eq!(fit(37, 19, 50, 80, 19 + 8), Fit::LinkOnly);
        assert_eq!(fit(37, 19, 50, 80, 19 + 9), Fit::LinkOnly);
        assert_eq!(fit(37, 19, 50, 80, 19 + 10), Fit::Under);
        assert_eq!(fit(37, 19, 50, 95, 19 + 5), Fit::Beside);
        assert_eq!(fit(37, 19, 50, 95, 19 + 4), Fit::LinkOnly);
        assert!(code_screen("person@example.com", 120, 40).join("\n").contains('▀'));
    }

    #[test]
    fn beside_lines_are_no_wider_than_the_fit_says() {
        let link = slack_core::signin::signin_link("person@example.com");
        let qr_cols = modules(&link).len();
        // The code (45 columns), the gap and the link (71) need 119 columns to stand side by side.
        assert_eq!(qr_cols, 45);
        assert_eq!(fit(qr_cols, qr_cols.div_ceil(2), link.width(), 118, 40), Fit::Under);
        let screen = code_screen("person@example.com", 120, 40);
        let text_cols = ["On your phone, scan this — or open", link.as_str(), "Slack emails you a code. Type it here;"].iter().map(|t| t.width()).max().unwrap();
        for l in &screen[3..] {
            let v = visible(l);
            assert!(v.width() <= qr_cols + 3 + text_cols, "{v:?}");
            assert!(v.width() >= qr_cols, "{v:?}");
        }
        assert!(visible(&screen[3]).ends_with("or open"));
    }

    #[test]
    fn wrapped_lines_are_counted() {
        // The link is 71 columns: 4 lines on a terminal 20 wide; an empty line is still one.
        let link = osc8("https://app.slack.com/get-started?email=person%40example.com#/createnew");
        assert_eq!(rows_on(&[String::new(), "abc".into(), link], 20), 6);
        let screen = code_screen("person@example.com", 100, 40);
        assert_eq!(rows_on(&screen, 100), screen.len());
    }

    #[test]
    fn an_80x24_terminal_gets_the_compact_code() {
        let link = slack_core::signin::signin_link("person@example.com");
        assert!((70..=75).contains(&link.width()), "{link}");
        let screen = code_screen("person@example.com", 80, 24);
        // One header line, the code, the link; the message and the prompt lines come under it.
        assert_eq!(screen[0], "Sign in with a code · person@example.com");
        assert!(screen[1..screen.len() - 1].iter().all(|l| l.contains('▀')), "{screen:?}");
        assert_eq!(screen.last().unwrap(), &osc8(&link));
        for l in &screen {
            assert!(visible(l).width() <= 80, "{:?}", visible(l));
        }
        assert!(rows_on(&screen, 80) + PROMPT_LINES <= 24, "{}", rows_on(&screen, 80));
        // A terminal big enough for the full code still gets it, under its text.
        let under = code_screen("person@example.com", 103, 50);
        assert_eq!(under[0], "Sign in with a code");
        assert!(under.iter().any(|l| l.contains('▀')) && !under.iter().any(|l| l.contains('▀') && l.contains("or open")));
    }

    #[test]
    fn the_compact_code_is_smaller_with_a_narrower_quiet_zone() {
        let link = slack_core::signin::signin_link("person@example.com");
        let (full, small) = (modules(&link), compact_modules(&link));
        // Error correction L takes the link in version 4 (33 modules) instead of 5 (37), and the quiet zone is 2.
        assert_eq!((full.len(), small.len()), (45, 37));
        assert!(small[..2].iter().chain(&small[35..]).all(|r| r.iter().all(|d| !d)));
        assert!(small.iter().all(|r| !r[0] && !r[1] && !r[35] && !r[36]));
        assert!((2..=8).all(|x| small[2][x]) && !small[3][3]);
        assert_eq!(layout(&link, 71, 80, 24), Fit::Compact);
        assert_eq!(layout(&link, 71, 103, 50), Fit::Under);
        assert_eq!(layout(&link, 71, 130, 40), Fit::Beside);
        assert_eq!(layout(&link, 71, 36, 40), Fit::LinkOnly);
        // Down: 1 + 19 + the link + 2; the link wraps to two lines below 71 columns.
        assert!(compact_fits(37, 19, 71, 80, 23) && !compact_fits(37, 19, 71, 80, 22));
        assert!(compact_fits(37, 19, 71, 40, 24) && !compact_fits(37, 19, 71, 40, 23));
        assert!(!compact_fits(37, 19, 71, 36, 50));
    }

    #[test]
    fn the_link_only_screen_says_a_bigger_window_shows_the_code() {
        let screen = code_screen("person@example.com", 30, 40);
        assert!(!screen.iter().any(|l| l.contains('▀')));
        assert_eq!(visible(screen.last().unwrap()), "a bigger window shows a QR code");
        assert!(screen.last().unwrap().starts_with("\x1b[2m"));
        // Too short for the compact code too.
        assert!(!code_screen("person@example.com", 80, 22).iter().any(|l| l.contains('▀')));
    }

    /// Prints the screen for the live check: `KLACK_QR_OUT=file [KLACK_QR_SIZE=80x24] cargo test -p klack qr_print -- --ignored`
    /// (120 x 40 when no size is given).
    #[test]
    #[ignore]
    fn qr_print() {
        let out = std::env::var("KLACK_QR_OUT").unwrap();
        let size = std::env::var("KLACK_QR_SIZE").ok().and_then(|s| {
            let (w, h) = s.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        });
        let (w, h) = size.unwrap_or((120, 40));
        std::fs::write(out, code_screen("person@example.com", w, h).join("\n") + "\n").unwrap();
    }
}

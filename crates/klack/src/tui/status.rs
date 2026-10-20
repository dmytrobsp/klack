//! The status line: the mode pill, the info (or a note), and at the right the pending keys and
//! hints. Pure layout; `event_loop::draw` gathers the inputs.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// The mode shown in the pill at the left edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pill {
    Normal,
    Insert,
    /// A dialog, by name ("JUMP").
    Dialog(&'static str),
}

/// What the line says besides the mode.
#[derive(Debug, Clone, Default)]
pub struct Info {
    pub unread: usize,
    pub mentions: u64,
    /// The other workspaces' note (" · Alt+2 @3"), empty when all is read.
    pub others: String,
    /// Your status and paused notifications ("☕ lunch · 🔕 until 09:00"), empty when none.
    pub availability: String,
    /// Notifications are paused (the short forms keep the bell).
    pub paused: bool,
    /// You appear away (the dot is dim).
    pub away: bool,
    /// Connected; while not, the info says "connecting…" (at the barest a dim `…` after the dot).
    pub live: bool,
    /// A status message or the ringing text; takes the info's place.
    pub note: Option<String>,
}

/// The pill's colour: accent for NORMAL, green for INSERT, the pin yellow for a dialog.
fn pill_colour(pill: Pill) -> Color {
    let p = super::theme::p();
    match pill {
        Pill::Normal => p.accent,
        Pill::Insert => p.good,
        Pill::Dialog(_) => p.pin,
    }
}

/// `s` in at most `room` columns, with `…` where it was cut.
pub fn cut(s: &str, room: usize) -> String {
    if s.width() <= room {
        return s.to_string();
    }
    let mut out = String::new();
    for c in s.chars() {
        if out.width() + unicode_width::UnicodeWidthChar::width(c).unwrap_or(0) + 1 > room {
            break;
        }
        out.push(c);
    }
    if room > 0 { format!("{out}…") } else { out }
}

/// One way to lay the line out, from the roomiest to the barest.
#[derive(Clone, Copy)]
struct Shape {
    /// The whole mode name, not its first letter.
    full_pill: bool,
    /// 0 whole, 1 short (`3 · @1 ●`), 2 only the dot (your presence).
    info: u8,
    /// How many of the last hints stay.
    hints: usize,
    /// The last hint as its key only.
    key_only: bool,
}

fn pill_text(pill: Pill, full: bool) -> String {
    let name = match pill {
        Pill::Normal => "NORMAL",
        Pill::Insert => "INSERT",
        Pill::Dialog(n) => n,
    };
    if full { format!(" {name} ") } else { format!(" {} ", name.chars().next().unwrap_or(' ')) }
}

fn info_spans(info: &Info, level: u8, note_room: Option<usize>) -> Vec<Span<'static>> {
    let p = super::theme::p();
    let dim = Style::default().fg(p.dim);
    if let Some(note) = &info.note {
        let text = match note_room {
            Some(room) => cut(note, room),
            None => note.clone(),
        };
        return vec![Span::raw(text)];
    }
    // Your presence as in Slack: green active, dim away (paused notifications are the 🔕).
    let label = if level > 0 { "" } else if info.away { " away" } else { " active" };
    let dot = if info.away { Span::styled(format!("○{label}"), dim) } else { Span::styled(format!("●{label}"), Style::default().fg(p.good)) };
    let mut out = match level {
        0 => {
            let mut text = format!("{} unread · {} mention{}{}", info.unread, info.mentions, if info.mentions == 1 { "" } else { "s" }, info.others);
            if !info.availability.is_empty() {
                text.push_str(&format!(" · {}", info.availability));
            }
            text.push_str(" · ");
            vec![Span::raw(text), dot]
        }
        1 => vec![Span::raw(format!("{} · @{} {}", info.unread, info.mentions, if info.paused { "🔕 " } else { "" })), dot],
        _ if info.paused => vec![Span::raw("🔕 "), dot],
        _ => vec![dot],
    };
    // The connection shows only while it is down; at the barest as a dim `…` after the dot.
    if !info.live {
        out.push(Span::styled(match level { 0 => " · connecting…", 1 => " connecting…", _ => "…" }, dim));
    }
    out
}

fn right_spans(showcmd: &str, hints: &[(String, String)], key_only: bool) -> Vec<Span<'static>> {
    let p = super::theme::p();
    let dim = Style::default().fg(p.dim);
    let mut out = Vec::new();
    if !showcmd.is_empty() {
        out.push(Span::styled(showcmd.to_string(), Style::default().fg(p.accent).add_modifier(Modifier::BOLD)));
        out.push(Span::raw(" "));
    }
    for (i, (key, word)) in hints.iter().enumerate() {
        if i > 0 {
            out.push(Span::styled(" · ", dim));
        }
        out.push(Span::styled(key.clone(), Style::default().add_modifier(Modifier::BOLD)));
        if !(key_only && i + 1 == hints.len()) {
            out.push(Span::styled(format!(" {word}"), dim));
        }
    }
    out.push(Span::raw(" "));
    out
}

fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

/// The status line in `width` columns: the mode pill, the info (or the note) after it, and
/// flush right the pending keys and the hints. It shrinks as the mockup showed: the info goes
/// short, hints drop from the front, the pill becomes a letter, the hints end at `?` and the
/// info at `●`. The pending keys are never cut; a note is cut (with `…`) last.
pub fn line(pill: Pill, info: &Info, showcmd: &str, hints: &[(String, String)], width: u16) -> Line<'static> {
    let width = width as usize;
    let n = hints.len();
    let shapes = [
        Shape { full_pill: true, info: 0, hints: n, key_only: false },
        Shape { full_pill: true, info: 1, hints: n, key_only: false },
        Shape { full_pill: true, info: 1, hints: n.min(3), key_only: false },
        Shape { full_pill: false, info: 1, hints: n.min(3), key_only: false },
        Shape { full_pill: false, info: 1, hints: n.min(2), key_only: false },
        Shape { full_pill: false, info: 2, hints: n.min(1), key_only: true },
    ];
    let build = |s: Shape, note_room: Option<usize>| {
        let shown = &hints[n - s.hints..];
        let pill_style = Style::default().bg(pill_colour(pill)).fg(Color::Black).add_modifier(Modifier::BOLD);
        let mut left = vec![Span::styled(pill_text(pill, s.full_pill), pill_style), Span::raw(" ")];
        left.extend(info_spans(info, s.info, note_room));
        (left, right_spans(showcmd, shown, s.key_only))
    };
    let fits = |left: &[Span], right: &[Span]| spans_width(left) + spans_width(right) + 1 <= width;
    let mut chosen = None;
    for s in shapes {
        let (left, right) = build(s, None);
        if fits(&left, &right) {
            chosen = Some((left, right));
            break;
        }
    }
    let (left, right) = chosen.unwrap_or_else(|| {
        // Nothing fits whole: cut the note, else leave the info out; at the worst only the pill
        // and the pending keys remain.
        let last = shapes[shapes.len() - 1];
        let (mut left, mut right) = build(last, None);
        let room = width.saturating_sub(spans_width(&right) + 1 + 4);
        if info.note.is_some() && room >= 2 {
            (left, right) = build(last, Some(room));
        }
        if !fits(&left, &right) {
            left.truncate(1);
        }
        if !fits(&left, &right) {
            right = right_spans(showcmd, &[], false);
        }
        if !fits(&left, &right) {
            left.clear();
        }
        (left, right)
    });
    let used = spans_width(&left) + spans_width(&right);
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(width.saturating_sub(used))));
    spans.extend(right);
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.to_string()).collect()
    }

    fn info() -> Info {
        Info { unread: 3, mentions: 1, live: true, ..Default::default() }
    }

    fn hints() -> Vec<(String, String)> {
        [("↑/↓", "move"), ("Enter", "thread"), ("i", "type"), ("␣", "actions"), ("?", "help")].iter().map(|(k, w)| (k.to_string(), w.to_string())).collect()
    }

    fn at(width: u16) -> String {
        let s = plain(&line(Pill::Normal, &info(), "␣", &hints(), width));
        assert_eq!(s.width(), width as usize, "{s:?}");
        s
    }

    #[test]
    fn wide_shows_everything() {
        let s = at(110);
        assert!(s.starts_with(" NORMAL "), "{s:?}");
        assert!(s.contains(" 3 unread · 1 mention · ● active"), "{s:?}");
        assert!(s.ends_with("↑/↓ move · Enter thread · i type · ␣ actions · ? help "), "{s:?}");
        assert!(s.contains("␣ ↑/↓ move"), "the showcmd comes first on the right: {s:?}");
    }

    #[test]
    fn info_goes_short_first() {
        let s = at(80);
        assert!(s.contains("3 · @1 ●") && !s.contains("unread"), "{s:?}");
        assert!(s.contains("↑/↓ move") && s.ends_with("? help "), "all hints still there: {s:?}");
    }

    #[test]
    fn the_bell_survives_the_short_forms() {
        let paused = Info { paused: true, ..info() };
        let s = |w| plain(&line(Pill::Normal, &paused, "␣", &hints(), w));
        assert!(s(80).contains("3 · @1 🔕 ●"), "{:?}", s(80));
        let tiny = s(30);
        assert!(tiny.contains("🔕 ●") && !tiny.contains("unread"), "{tiny:?}");
        assert!(!at(30).contains('🔕'), "not paused: no bell");
    }

    #[test]
    fn hints_drop_from_the_front() {
        let s = at(60);
        assert!(!s.contains("↑/↓ move") && !s.contains("Enter thread"), "{s:?}");
        assert!(s.ends_with("i type · ␣ actions · ? help "), "{s:?}");
        assert!(s.starts_with(" NORMAL "), "{s:?}");
    }

    #[test]
    fn the_pill_shrinks_to_a_letter() {
        let s = at(40);
        assert!(s.starts_with(" N ") && !s.starts_with(" NORMAL"), "{s:?}");
        assert!(s.ends_with("? help "), "{s:?}");
        let d = plain(&line(Pill::Dialog("JUMP"), &info(), "", &hints(), 20));
        assert!(d.starts_with(" J "), "a dialog's first letter: {d:?}");
    }

    #[test]
    fn at_the_end_only_keys_and_dot_remain() {
        let s = at(24);
        assert!(s.contains('●') && s.contains('?') && s.contains('␣'), "{s:?}");
        assert!(s.starts_with(" N ") && !s.contains("help") && !s.contains("@1") && !s.contains("unread") && s.ends_with("␣ ? "), "{s:?}");
        let t = at(18);
        assert!(t.contains('●') && t.ends_with("␣ ? ") && !t.contains("help"), "{t:?}");
        let tiny = at(10);
        assert!(tiny.contains('␣'), "{tiny:?}");
        for w in 0..10 {
            let _ = plain(&line(Pill::Insert, &info(), "^␣h", &hints(), w));
        }
    }

    #[test]
    fn the_showcmd_is_never_cut() {
        let s = at(12);
        assert!(s.contains('␣'), "{s:?}");
        let long = plain(&line(Pill::Normal, &info(), "^␣hm", &hints(), 24));
        assert!(long.contains("^␣hm"), "{long:?}");
    }

    #[test]
    fn a_note_outranks_the_hints() {
        let note = Info { note: Some("copied a link to the message".into()), ..info() };
        let s = plain(&line(Pill::Normal, &note, "", &hints(), 60));
        assert_eq!(s.width(), 60, "{s:?}");
        assert!(s.contains("copied a link to the message") && !s.contains('…'), "{s:?}");
        assert!(!s.contains("↑/↓ move") && s.ends_with("? help "), "hints shrank first: {s:?}");
        let cut = plain(&line(Pill::Normal, &note, "", &hints(), 20));
        assert_eq!(cut.width(), 20, "{cut:?}");
        assert!(cut.contains('…') && cut.contains("copied"), "the note is cut last: {cut:?}");
    }

    #[test]
    fn connecting_and_the_extras() {
        let i = Info { live: false, others: " · Alt+2 @3".into(), availability: "☕ lunch".into(), away: true, ..info() };
        let s = plain(&line(Pill::Insert, &i, "", &[], 100));
        assert!(s.starts_with(" INSERT ") && s.contains("3 unread · 1 mention · Alt+2 @3 · ☕ lunch · ○ away · connecting…"), "{s:?}");
        let short = plain(&line(Pill::Insert, &Info { availability: String::new(), ..i.clone() }, "", &hints(), 80));
        assert!(short.contains("3 · @1 ○ connecting…"), "{short:?}");
        let up = plain(&line(Pill::Insert, &info(), "", &[], 100));
        assert!(!up.contains("connecting") && !up.contains("live"), "connected says nothing: {up:?}");
        let two = Info { mentions: 2, ..info() };
        assert!(plain(&line(Pill::Normal, &two, "", &[], 100)).contains("2 mentions"));
    }

    #[test]
    fn the_dot_is_your_presence() {
        let p = crate::tui::theme::p();
        let away = Info { away: true, ..info() };
        let dot = |i: &Info, w| line(Pill::Normal, i, "␣", &hints(), w).spans.into_iter().find(|s| s.content.starts_with('●') || s.content.starts_with('○')).unwrap();
        let wide = plain(&line(Pill::Normal, &away, "␣", &hints(), 110));
        assert!(wide.contains("3 unread · 1 mention · ○ away") && !wide.contains('●'), "{wide:?}");
        for w in [110, 80, 24] {
            assert!(dot(&away, w).content.starts_with('○') && dot(&away, w).style.fg == Some(p.dim), "away at {w}: {:?}", dot(&away, w));
            assert!(dot(&info(), w).content.starts_with('●') && dot(&info(), w).style.fg == Some(p.good), "active at {w}");
        }
        assert!(plain(&line(Pill::Normal, &away, "␣", &hints(), 80)).contains("3 · @1 ○"));
        let offline = Info { live: false, ..info() };
        assert!(dot(&offline, 80).content.starts_with('●'), "the connection doesn't dim your presence");
    }

    #[test]
    fn a_dropped_connection_shows_at_the_narrowest() {
        let p = crate::tui::theme::p();
        let offline = Info { live: false, ..info() };
        for w in [24, 18] {
            let l = line(Pill::Normal, &offline, "␣", &hints(), w);
            let s = plain(&l);
            assert_eq!(s.width(), w as usize, "{s:?}");
            assert!(s.contains("●…") && !s.contains("connecting"), "at {w}: {s:?}");
            assert!(l.spans.iter().any(|sp| sp.content == "…" && sp.style.fg == Some(p.dim)), "dim: {l:?}");
            assert!(!plain(&line(Pill::Normal, &info(), "␣", &hints(), w)).contains('…'), "connected: no mark at {w}");
        }
    }
}

//! Message document → wrapped, styled terminal lines.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use slack_core::format::{self, Block, Inline};
use unicode_width::UnicodeWidthStr;

use super::state::Workspace;

pub const MENTION: Color = Color::Rgb(0x1d, 0x9b, 0xd1);
pub const LINK: Color = Color::Rgb(0x5d, 0xa9, 0xf6);
pub const CODE_FG: Color = Color::Rgb(0xe8, 0x91, 0x2d);
pub const DIM: Color = Color::Rgb(0x8a, 0x8a, 0x8a);
pub const QUOTE_BAR: Color = Color::Rgb(0x6e, 0x6e, 0x6e);

/// Stable per-user colour for author names.
pub fn user_color(id: &str) -> Color {
    const PALETTE: [Color; 10] = [
        Color::Rgb(0xe0, 0x6c, 0x75),
        Color::Rgb(0x98, 0xc3, 0x79),
        Color::Rgb(0xe5, 0xc0, 0x7b),
        Color::Rgb(0x61, 0xaf, 0xef),
        Color::Rgb(0xc6, 0x78, 0xdd),
        Color::Rgb(0x56, 0xb6, 0xc2),
        Color::Rgb(0xd1, 0x9a, 0x66),
        Color::Rgb(0xbe, 0x50, 0x46),
        Color::Rgb(0x7f, 0xb9, 0x8a),
        Color::Rgb(0xa9, 0x8d, 0xe0),
    ];
    let h = id.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
    PALETTE[(h % PALETTE.len() as u32) as usize]
}

/// Emoji shortcode (optionally with unicode hint) → display text.
pub fn emoji(ws: &Workspace, name: &str, unicode: Option<&str>) -> String {
    if let Some(s) = unicode.and_then(format::decode_unicode) {
        return s;
    }
    let (base, tone) = match name.split_once("::") {
        Some((b, t)) => (b, Some(t)),
        None => (name, None),
    };
    if let Some(e) = emojis::get_by_shortcode(base) {
        let tone = tone.and_then(|t| match t {
            "skin-tone-2" => Some(emojis::SkinTone::Light),
            "skin-tone-3" => Some(emojis::SkinTone::MediumLight),
            "skin-tone-4" => Some(emojis::SkinTone::Medium),
            "skin-tone-5" => Some(emojis::SkinTone::MediumDark),
            "skin-tone-6" => Some(emojis::SkinTone::Dark),
            _ => None,
        });
        return tone.and_then(|t| e.with_skin_tone(t)).unwrap_or(e).as_str().to_string();
    }
    // Workspace aliases (`alias:thumbsup`) resolve to standard emoji.
    if let Some(alias) = ws.custom_emoji.get(name).and_then(|v| v.strip_prefix("alias:")) {
        if let Some(e) = emojis::get_by_shortcode(alias) {
            return e.as_str().to_string();
        }
    }
    format!(":{name}:")
}

fn text_style(s: format::Style) -> Style {
    let mut st = Style::default();
    if s.bold {
        st = st.add_modifier(Modifier::BOLD);
    }
    if s.italic {
        st = st.add_modifier(Modifier::ITALIC);
    }
    if s.strike {
        st = st.add_modifier(Modifier::CROSSED_OUT);
    }
    if s.code {
        st = st.fg(CODE_FG);
    }
    st
}

/// Spans for one inline element.
pub fn inline_spans(ws: &Workspace, i: &Inline) -> Vec<Span<'static>> {
    let mention = Style::default().fg(MENTION).add_modifier(Modifier::BOLD);
    match i {
        Inline::Text { text, style } => vec![Span::styled(text.clone(), text_style(*style))],
        Inline::User { id } => {
            let st = if *id == ws.self_id { mention.add_modifier(Modifier::REVERSED) } else { mention };
            vec![Span::styled(format!("@{}", ws.user_name(id)), st)]
        }
        Inline::Channel { id, name } => vec![Span::styled(format!("#{}", name.clone().unwrap_or_else(|| ws.channel_label(id))), mention)],
        Inline::Broadcast(b) => vec![Span::styled(format!("@{b}"), mention.add_modifier(Modifier::REVERSED))],
        Inline::Usergroup { id, handle } => vec![Span::styled(handle.clone().unwrap_or_else(|| format!("@{id}")), mention)],
        Inline::Link { url, label, style } => {
            let shown = label.clone().filter(|l| !l.is_empty()).unwrap_or_else(|| url.trim_start_matches("mailto:").to_string());
            vec![Span::styled(shown, text_style(*style).fg(LINK).add_modifier(Modifier::UNDERLINED))]
        }
        Inline::Emoji { name, unicode } => vec![Span::raw(emoji(ws, name, unicode.as_deref()))],
    }
}

/// Plain text of an inline element (notifications, previews).
pub fn inline_plain(ws: &Workspace, i: &Inline) -> String {
    inline_spans(ws, i).into_iter().map(|s| s.content.to_string()).collect()
}

/// Greedy word wrap of styled spans to `width` columns. `prefix` is repeated on every
/// line (quote bars, list bullets get a hanging indent via `cont_prefix`).
pub fn wrap(spans: Vec<Span<'static>>, width: usize, prefix: Vec<Span<'static>>, cont_prefix: Vec<Span<'static>>) -> Vec<Line<'static>> {
    let width = width.max(8);
    let prefix_w: usize = prefix.iter().map(|s| s.content.width()).sum();
    let cont_w: usize = cont_prefix.iter().map(|s| s.content.width()).sum();
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut cur: Vec<Span<'static>> = prefix.clone();
    let mut cur_w = prefix_w;
    let mut first = true;
    let new_line = |lines: &mut Vec<Line<'static>>, cur: &mut Vec<Span<'static>>, cur_w: &mut usize, first: &mut bool| {
        lines.push(Line::from(std::mem::take(cur)));
        *first = false;
        *cur = cont_prefix.clone();
        *cur_w = cont_w;
    };
    for span in spans {
        let style = span.style;
        // Explicit newlines inside text.
        for (li, segment) in span.content.split('\n').enumerate() {
            if li > 0 {
                new_line(&mut lines, &mut cur, &mut cur_w, &mut first);
            }
            // Split into words keeping whitespace attached to the following word boundary.
            let mut word = String::new();
            let mut tokens: Vec<String> = Vec::new();
            for ch in segment.chars() {
                word.push(ch);
                if ch == ' ' {
                    tokens.push(std::mem::take(&mut word));
                }
            }
            if !word.is_empty() {
                tokens.push(word);
            }
            for tok in tokens {
                let mut tok = tok;
                loop {
                    let w = tok.width();
                    let avail = width.saturating_sub(cur_w);
                    let base_w = if first { prefix_w } else { cont_w };
                    if w <= avail {
                        cur_w += w;
                        cur.push(Span::styled(tok, style));
                        break;
                    }
                    if cur_w > base_w && tok.trim_end().width() <= width - base_w {
                        // Move the whole word to the next line.
                        new_line(&mut lines, &mut cur, &mut cur_w, &mut first);
                        if tok.trim().is_empty() {
                            break;
                        }
                        continue;
                    }
                    // Hard-break a word longer than the line.
                    let mut head = String::new();
                    let mut head_w = 0;
                    let mut rest = String::new();
                    for ch in tok.chars() {
                        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                        if rest.is_empty() && head_w + cw <= avail.max(1) {
                            head.push(ch);
                            head_w += cw;
                        } else {
                            rest.push(ch);
                        }
                    }
                    cur.push(Span::styled(head, style));
                    new_line(&mut lines, &mut cur, &mut cur_w, &mut first);
                    if rest.is_empty() {
                        break;
                    }
                    tok = rest;
                }
            }
        }
    }
    if cur.len() > cont_prefix.len() || lines.is_empty() || (first && cur.len() > prefix.len()) {
        lines.push(Line::from(cur));
    }
    lines
}

/// Render a message document to lines of at most `width` columns.
pub fn render_blocks(ws: &Workspace, blocks: &[Block], width: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for b in blocks {
        match b {
            Block::Paragraph(items) => {
                let spans: Vec<Span> = items.iter().flat_map(|i| inline_spans(ws, i)).collect();
                if spans.is_empty() {
                    out.push(Line::raw(""));
                } else {
                    out.extend(wrap(spans, width, vec![], vec![]));
                }
            }
            Block::Quote(items) => {
                let bar = vec![Span::styled("▎ ", Style::default().fg(QUOTE_BAR))];
                let spans: Vec<Span> = items.iter().flat_map(|i| inline_spans(ws, i)).collect();
                out.extend(wrap(spans, width, bar.clone(), bar));
            }
            Block::Code(code) => {
                let st = Style::default().fg(CODE_FG);
                let bar = Span::styled("│ ", Style::default().fg(QUOTE_BAR));
                for line in code.lines() {
                    out.extend(wrap(vec![Span::styled(line.to_string(), st)], width, vec![bar.clone()], vec![bar.clone()]));
                }
            }
            Block::List { ordered, indent, items } => {
                let pad = "  ".repeat(*indent as usize);
                for (n, item) in items.iter().enumerate() {
                    let marker = if *ordered { format!("{pad}{}. ", n + 1) } else { format!("{pad}• ") };
                    let hang = " ".repeat(marker.width());
                    let spans: Vec<Span> = item.iter().flat_map(|i| inline_spans(ws, i)).collect();
                    out.extend(wrap(spans, width, vec![Span::raw(marker)], vec![Span::raw(hang)]));
                }
            }
        }
    }
    out
}

/// Human-friendly byte size.
pub fn human_size(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{n} B") } else { format!("{v:.1} {}", UNITS[u]) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.spans.iter().map(|s| s.content.to_string()).collect::<String>()).collect()
    }

    #[test]
    fn wraps_words() {
        let lines = wrap(vec![Span::raw("the quick brown fox jumps over the lazy dog")], 15, vec![], vec![]);
        let p = plain(&lines);
        assert!(p.iter().all(|l| l.trim_end().width() <= 15), "{p:?}");
        assert_eq!(p.join("").replace("  ", " "), "the quick brown fox jumps over the lazy dog");
    }

    #[test]
    fn hard_breaks_long_words() {
        let lines = wrap(vec![Span::raw("aaaaaaaaaaaaaaaaaaaa")], 8, vec![], vec![]);
        assert_eq!(plain(&lines), vec!["aaaaaaaa", "aaaaaaaa", "aaaa"]);
    }

    #[test]
    fn newlines_and_prefix() {
        let lines = wrap(vec![Span::raw("one\ntwo")], 20, vec![Span::raw("> ")], vec![Span::raw("> ")]);
        assert_eq!(plain(&lines), vec!["> one", "> two"]);
    }

    #[test]
    fn emoji_resolution() {
        let ws = Workspace::default();
        assert_eq!(emoji(&ws, "wave", None), "👋");
        assert_eq!(emoji(&ws, "+1", None), "👍");
        assert_eq!(emoji(&ws, "x", Some("1f44b")), "👋");
        assert_eq!(emoji(&ws, "partyparrot", None), ":partyparrot:");
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 KB");
    }
}

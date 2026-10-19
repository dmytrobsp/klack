//! Message document → wrapped, styled terminal lines.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use slack_core::format::{self, Block, Inline};
use unicode_width::UnicodeWidthStr;

use super::state::Workspace;


/// Lines drawn as a shaded box: one column of padding on each side, every row filled to
/// the widest line (at most `width`), so code stands apart from message text.
pub fn shaded(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    let bg = Style::default().bg(super::theme::p().code_bg);
    let inner = lines.iter().map(Line::width).max().unwrap_or(0).min(width.saturating_sub(2));
    lines
        .into_iter()
        .map(|line| {
            let pad = inner.saturating_sub(line.width()) + 1;
            let mut spans = vec![Span::styled(" ", bg)];
            spans.extend(line.spans.into_iter().map(|s| Span::styled(s.content, s.style.patch(bg))));
            spans.push(Span::styled(" ".repeat(pad), bg));
            Line::from(spans)
        })
        .collect()
}

/// Stable per-user colour for author names.
pub fn user_color(id: &str) -> Color {
    let palette = super::theme::p().authors;
    let h = id.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
    palette[(h % palette.len() as u32) as usize]
}

/// Slack's names for standard emoji that the emoji tables name otherwise.
const SLACK_NAMES: &[(&str, &str)] = &[("squirrel", "chipmunk")];

/// A standard emoji by its Slack name.
pub fn standard_emoji(name: &str) -> Option<&'static emojis::Emoji> {
    emojis::get_by_shortcode(name).or_else(|| SLACK_NAMES.iter().find(|(slack, _)| *slack == name).and_then(|(_, other)| emojis::get_by_shortcode(other)))
}

/// What a workspace's own emoji `name` shows as: an alias of a standard emoji shows it; a
/// picture has none (None).
pub fn custom_glyph(custom: &std::collections::HashMap<String, String>, name: &str) -> Option<String> {
    let alias = custom.get(name)?.strip_prefix("alias:")?;
    standard_emoji(alias).map(|e| e.as_str().to_string())
}

/// The first of the private-use characters standing for custom emoji pictures in built
/// message lines (one per numbered emoji, `Workspace::emoji_ids`), two cells each.
const EMOJI_CELL: u32 = 0xF0000;

/// The picture of the workspace's own emoji `name` (also through an alias of another): its URL.
pub fn custom_picture<'a>(ws: &'a Workspace, name: &str) -> Option<&'a str> {
    let value = ws.custom_emoji.get(name)?;
    let value = match value.strip_prefix("alias:") {
        Some(other) => ws.custom_emoji.get(other)?,
        None => value,
    };
    (!value.starts_with("alias:")).then_some(value.as_str())
}

/// Emoji `name` as the message pane shows it: a custom emoji's picture, when drawn, is two
/// placeholder cells (taken out by [`emoji_cells`]); anything else as [`emoji`].
pub fn emoji_cell(ws: &Workspace, name: &str, unicode: Option<&str>) -> String {
    let cell = ws.emoji_index.get(name).filter(|_| ws.emoji_pictures && unicode.is_none() && custom_picture(ws, name).is_some()).and_then(|n| char::from_u32(EMOJI_CELL + n));
    match cell {
        Some(c) => [c, c].iter().collect(),
        None => emoji(ws, name, unicode),
    }
}

/// Take the custom emoji placeholders out of `lines` (blanks in their place): where each
/// picture goes, as (line, column, emoji name).
pub fn emoji_cells(ws: &Workspace, lines: &mut [Line<'static>]) -> Vec<(usize, u16, String)> {
    let number = |c: char| (c as u32).checked_sub(EMOJI_CELL).filter(|n| (*n as usize) < ws.emoji_ids.len());
    let mut out = Vec::new();
    for (n, line) in lines.iter_mut().enumerate() {
        let mut col = 0u16;
        for span in line.spans.iter_mut() {
            if !span.content.chars().any(|c| number(c).is_some()) {
                col += span.content.width() as u16;
                continue;
            }
            // A picture's two cells carry the same number; the first places it.
            let (mut text, mut open) = (String::new(), None);
            for ch in span.content.chars() {
                match number(ch) {
                    Some(id) if open == Some(id) => open = None,
                    Some(id) => {
                        out.push((n, col, ws.emoji_ids[id as usize].clone()));
                        open = Some(id);
                    }
                    None => {
                        open = None;
                        col += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0) as u16;
                        text.push(ch);
                        continue;
                    }
                }
                col += 1;
                text.push(' ');
            }
            span.content = text.into();
        }
    }
    out
}

/// Emoji shortcode (optionally with unicode hint) → display text.
pub fn emoji(ws: &Workspace, name: &str, unicode: Option<&str>) -> String {
    if ws.settings.messages.emoji_as_names {
        return format!(":{name}:");
    }
    if let Some(s) = unicode.and_then(format::decode_unicode) {
        return s;
    }
    let (base, tone) = match name.split_once("::") {
        Some((b, t)) => (b, Some(t)),
        None => (name, None),
    };
    if let Some(e) = standard_emoji(base) {
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
    if let Some(glyph) = custom_glyph(&ws.custom_emoji, name) {
        return glyph;
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
        st = st.fg(super::theme::p().code_fg);
    }
    st
}

/// Spans for one inline element.
pub fn inline_spans(ws: &Workspace, i: &Inline) -> Vec<Span<'static>> {
    let mention = Style::default().fg(super::theme::p().mention).add_modifier(Modifier::BOLD);
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
            vec![Span::styled(shown, text_style(*style).fg(super::theme::p().link).add_modifier(Modifier::UNDERLINED))]
        }
        Inline::Emoji { name, unicode } => vec![Span::raw(emoji_cell(ws, name, unicode.as_deref()))],
    }
}

/// Plain text of an inline element (notifications, previews).
pub fn inline_plain(ws: &Workspace, i: &Inline) -> String {
    match i {
        // Never a picture's placeholder.
        Inline::Emoji { name, unicode } => emoji(ws, name, unicode.as_deref()),
        _ => inline_spans(ws, i).into_iter().map(|s| s.content.to_string()).collect(),
    }
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
                let bar = vec![Span::styled("▎ ", Style::default().fg(super::theme::p().quote_bar))];
                let spans: Vec<Span> = items.iter().flat_map(|i| inline_spans(ws, i)).collect();
                out.extend(wrap(spans, width, bar.clone(), bar));
            }
            Block::Code(code) => {
                let st = Style::default().fg(super::theme::p().code_fg);
                let rows: Vec<Line<'static>> = code.lines().flat_map(|line| wrap(vec![Span::styled(line.to_string(), st)], width.saturating_sub(2), vec![], vec![])).collect();
                out.extend(shaded(rows, width));
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

//! Syntax highlighting for snippets (syntect with its pure-Rust regex engine).

use std::sync::LazyLock;

use ratatui::style::{Color, Style};
use ratatui::text::Span;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;
use unicode_width::UnicodeWidthStr;

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEMES: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

/// The syntax theme of the current colour theme.
fn theme() -> &'static Theme {
    let name = super::theme::p().syntax;
    THEMES.themes.get(name).or_else(|| THEMES.themes.values().next()).expect("syntect ships themes")
}

/// Load the grammars and theme ahead of first use (it takes a moment).
pub fn warm_up() {
    LazyLock::force(&SYNTAXES);
    LazyLock::force(&THEMES);
}

/// Highlighted preview lines of a snippet file, computed once per file.
pub fn preview(file: &slack_core::types::File, plain: Style) -> Vec<Vec<Span<'static>>> {
    static CACHE: LazyLock<std::sync::Mutex<std::collections::HashMap<String, Vec<Vec<Span<'static>>>>>> = LazyLock::new(Default::default);
    let make = || highlight(file.preview.as_deref().unwrap_or_default(), &[&file.title, &file.name], &file.filetype, plain);
    if file.id.is_empty() {
        return make();
    }
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.entry(format!("{}/{}", super::theme::p().name, file.id)).or_insert_with(make).clone()
}

/// The language a snippet is highlighted as.
pub fn language(file: &slack_core::types::File) -> Option<&'static str> {
    syntax_for(&[&file.title, &file.name], &file.filetype).map(|s| s.name.as_str())
}

/// The grammar for a snippet: by the extension of its title or name (Slack may store
/// `hello.rs` as C++), else by Slack's filetype; None for plain text.
fn syntax_for(names: &[&str], filetype: &str) -> Option<&'static SyntaxReference> {
    let by_ext = names.iter().filter_map(|n| n.rsplit_once('.').map(|(_, ext)| ext)).find_map(|ext| SYNTAXES.find_syntax_by_extension(ext));
    by_ext.or_else(|| SYNTAXES.find_syntax_by_token(filetype)).filter(|s| s.name != "Plain Text")
}

/// `text` split into lines of coloured spans (tabs as four spaces). Unknown languages get
/// one plain span per line in `plain`.
pub fn highlight(text: &str, names: &[&str], filetype: &str, plain: Style) -> Vec<Vec<Span<'static>>> {
    let text = text.replace('\t', "    ");
    let plain_line = |l: &str| vec![Span::styled(l.trim_end_matches(['\n', '\r']).to_string(), plain)];
    let Some(syntax) = syntax_for(names, filetype) else {
        return text.lines().map(plain_line).collect();
    };
    let mut h = HighlightLines::new(syntax, theme());
    LinesWithEndings::from(&text)
        .map(|line| match h.highlight_line(line, &SYNTAXES) {
            Ok(parts) => parts
                .into_iter()
                .map(|(style, s)| (style.foreground, s.trim_end_matches(['\n', '\r'])))
                .filter(|(_, s)| !s.is_empty())
                .map(|(c, s)| Span::styled(s.to_string(), Style::default().fg(Color::Rgb(c.r, c.g, c.b))))
                .collect(),
            Err(_) => plain_line(line),
        })
        .collect()
}

/// `spans` cut to `width` columns, ending in `…` when something was cut.
pub fn clip_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    if spans.iter().map(|s| s.content.width()).sum::<usize>() <= width {
        return spans;
    }
    let mut out = Vec::new();
    let mut used = 0;
    for s in spans {
        let w = s.content.width();
        if used + w < width {
            used += w;
            out.push(s);
            continue;
        }
        // Something is cut from here on: keep one column for the ellipsis.
        let mut cut = String::new();
        for c in s.content.chars() {
            if used + cut.width() + unicode_width::UnicodeWidthChar::width(c).unwrap_or(0) + 1 > width {
                break;
            }
            cut.push(c);
        }
        cut.push('…');
        out.push(Span::styled(cut, s.style));
        return out;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(spans: &[Span]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn picks_the_language_by_extension_first() {
        assert_eq!(syntax_for(&["hello.rs", "hello.rs.cpp"], "cpp").map(|s| s.name.as_str()), Some("Rust"));
        assert_eq!(syntax_for(&["notes"], "python").map(|s| s.name.as_str()), Some("Python"));
        assert!(syntax_for(&["notes.txt"], "text").is_none());
    }

    #[test]
    fn highlighted_lines_keep_the_text() {
        let src = "fn main() {\n\tlet x = 1; // hi\n}\n";
        let lines = highlight(src, &["a.rs"], "", Style::default());
        assert_eq!(lines.iter().map(|l| text_of(l)).collect::<Vec<_>>(), vec!["fn main() {", "    let x = 1; // hi", "}"]);
        assert!(lines[0].len() > 1, "keywords are split into coloured spans");
        let plain = highlight("a\tb", &["x.txt"], "text", Style::default());
        assert_eq!(text_of(&plain[0]), "a    b");
    }

    #[test]
    fn clipping_spans() {
        let spans = vec![Span::raw("abc"), Span::raw("defgh")];
        assert_eq!(text_of(&clip_spans(spans.clone(), 5)), "abcd…");
        assert_eq!(text_of(&clip_spans(spans.clone(), 3)), "ab…");
        assert_eq!(text_of(&clip_spans(spans, 8)), "abcdefgh");
    }
}

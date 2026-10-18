//! Filterable emoji picker popup (used for reactions).

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};


/// Emoji people reach for most; shown first when the query is empty.
const FAVOURITES: &[&str] = &["+1", "heart", "joy", "tada", "eyes", "white_check_mark", "pray", "fire", "rocket", "thinking_face", "100", "raised_hands"];

#[derive(Debug, Clone)]
pub struct Candidate {
    pub name: String,
    pub glyph: String,
}

#[derive(Debug, Default)]
pub struct EmojiPicker {
    pub query: String,
    pub selected: usize,
}

impl EmojiPicker {
    /// Candidates for the current query: exact, then prefix, then substring matches.
    pub fn candidates(&self, custom: &[String]) -> Vec<Candidate> {
        let q = self.query.trim_matches(':').to_lowercase();
        let mut out: Vec<(u8, Candidate)> = Vec::new();
        if q.is_empty() {
            for name in FAVOURITES {
                if let Some(e) = emojis::get_by_shortcode(name) {
                    out.push((0, Candidate { name: name.to_string(), glyph: e.as_str().to_string() }));
                }
            }
            return out.into_iter().map(|(_, c)| c).collect();
        }
        for e in emojis::iter() {
            for code in e.shortcodes() {
                let rank = if code == q {
                    0
                } else if code.starts_with(&q) {
                    1
                } else if code.contains(&q) {
                    2
                } else {
                    continue;
                };
                out.push((rank, Candidate { name: code.to_string(), glyph: e.as_str().to_string() }));
            }
        }
        for name in custom.iter().filter(|n| n.contains(&q)) {
            out.push((3, Candidate { name: name.clone(), glyph: format!(":{name}:") }));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.name.len().cmp(&b.1.name.len())).then(a.1.name.cmp(&b.1.name)));
        out.dedup_by(|a, b| a.1.name == b.1.name);
        out.into_iter().take(200).map(|(_, c)| c).collect()
    }

    pub fn move_by(&mut self, delta: isize, len: usize) {
        if len == 0 {
            self.selected = 0;
            return;
        }
        self.selected = (self.selected as isize + delta).clamp(0, len as isize - 1) as usize;
    }

    pub fn draw(&self, f: &mut Frame, area: Rect, custom: &[String]) {
        let [popup] = Layout::horizontal([Constraint::Length(44)]).flex(Flex::Center).areas(area);
        let [popup] = Layout::vertical([Constraint::Length(16)]).flex(Flex::Center).areas(popup);
        f.render_widget(Clear, popup);
        let block = Block::default().borders(Borders::ALL).border_style(Style::default().fg(super::theme::p().accent)).title(format!(" {} ", super::i18n::tr("Add reaction")));
        let inner = block.inner(popup);
        f.render_widget(block, popup);
        let [input, list] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);
        f.render_widget(Paragraph::new(Line::from(vec![Span::styled("search: ", Style::default().fg(super::theme::p().dim)), Span::raw(self.query.clone())])), input);
        f.set_cursor_position((input.x + 8 + self.query.chars().count() as u16, input.y));
        let cands = self.candidates(custom);
        let height = list.height as usize;
        let start = self.selected.saturating_sub(height.saturating_sub(1));
        let lines: Vec<Line> = cands
            .iter()
            .enumerate()
            .skip(start)
            .take(height)
            .map(|(n, c)| {
                let line = Line::from(vec![Span::raw(format!(" {} ", c.glyph)), Span::styled(format!(":{}:", c.name), Style::default().fg(super::theme::p().dim))]);
                if n == self.selected { line.style(Style::default().bg(super::theme::p().selection).add_modifier(Modifier::BOLD)) } else { line }
            })
            .collect();
        let lines = if lines.is_empty() { vec![Line::styled(" no matching emoji", Style::default().fg(super::theme::p().dim))] } else { lines };
        f.render_widget(Paragraph::new(lines), list);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn favourites_when_empty() {
        let p = EmojiPicker::default();
        let c = p.candidates(&[]);
        assert_eq!(c[0].name, "+1");
        assert_eq!(c[0].glyph, "👍");
    }

    #[test]
    fn ranking_prefers_exact_then_prefix() {
        let p = EmojiPicker { query: "tada".into(), selected: 0 };
        assert_eq!(p.candidates(&[])[0].name, "tada");
        let p = EmojiPicker { query: "hea".into(), selected: 0 };
        let names: Vec<String> = p.candidates(&[]).into_iter().map(|c| c.name).collect();
        assert!(names.iter().take(5).any(|n| n == "heart"), "{names:?}");
    }

    #[test]
    fn includes_custom_emoji() {
        let p = EmojiPicker { query: "parrot".into(), selected: 0 };
        let c = p.candidates(&["partyparrot".into()]);
        assert!(c.iter().any(|c| c.name == "partyparrot"));
    }
}

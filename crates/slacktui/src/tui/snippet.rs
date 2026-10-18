//! Full view of a text snippet (`o` on a message with one): the whole file, scrollable.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use slack_core::types::File;

use super::text::{self, DIM};

pub struct SnippetView {
    pub file: File,
    /// The downloaded text, or why it could not be loaded.
    pub text: Option<Result<String, String>>,
    /// First line shown.
    pub scroll: usize,
    /// Rows of text on the last draw (for paging).
    pub height: usize,
}

impl SnippetView {
    pub fn new(file: File) -> SnippetView {
        SnippetView { file, text: None, scroll: 0, height: 0 }
    }

    fn line_count(&self) -> usize {
        match &self.text {
            Some(Ok(t)) => t.lines().count(),
            _ => 0,
        }
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let max = self.line_count().saturating_sub(self.height.max(1));
        self.scroll = (self.scroll as isize + delta).clamp(0, max as isize) as usize;
    }

    pub fn page(&self) -> isize {
        self.height.saturating_sub(1).max(1) as isize
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect) {
        f.render_widget(Clear, area);
        let name = if self.file.title.is_empty() { &self.file.name } else { &self.file.title };
        let lang = if self.file.pretty_type.is_empty() { String::new() } else { format!(" · {}", self.file.pretty_type) };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(0x61, 0xaf, 0xef)))
            .title(format!(" {name}{lang} "))
            .title_bottom(Line::styled(" ↑↓ PgUp PgDn g G scroll · y copy · Esc close ", Style::default().fg(DIM)).right_aligned());
        let inner = block.inner(area);
        self.height = inner.height as usize;
        let lines: Vec<Line> = match &self.text {
            None => vec![Line::styled("Loading…", Style::default().fg(DIM))],
            Some(Err(e)) => vec![Line::styled(format!("cannot load the snippet: {e}"), Style::default().fg(Color::Rgb(0xe0, 0x6c, 0x75)))],
            Some(Ok(t)) => {
                let total = t.lines().count();
                let digits = total.max(1).to_string().len();
                let room = (inner.width as usize).saturating_sub(digits + 3);
                t.lines()
                    .enumerate()
                    .skip(self.scroll)
                    .take(self.height)
                    .map(|(n, l)| {
                        Line::from(vec![
                            Span::styled(format!("{:>digits$} │ ", n + 1), Style::default().fg(DIM)),
                            Span::styled(text::clip(&l.replace('\t', "    "), room), Style::default().fg(text::CODE_FG)),
                        ])
                    })
                    .collect()
            }
        };
        f.render_widget(Paragraph::new(lines).block(block), area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrolling_stops_at_the_end() {
        let mut v = SnippetView::new(File::default());
        v.text = Some(Ok((1..=30).map(|n| n.to_string()).collect::<Vec<_>>().join("\n")));
        v.height = 10;
        v.scroll_by(100);
        assert_eq!(v.scroll, 20);
        v.scroll_by(-3);
        assert_eq!(v.scroll, 17);
        v.scroll_by(-100);
        assert_eq!(v.scroll, 0);
    }
}

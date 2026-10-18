//! Full view of a text snippet (`o` on a message with one): the whole file, scrollable.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use slack_core::types::File;


pub struct SnippetView {
    pub file: File,
    /// The downloaded text, or why it could not be loaded.
    pub text: Option<Result<String, String>>,
    /// The text's lines, highlighted.
    lines: Vec<Vec<Span<'static>>>,
    /// First line shown.
    pub scroll: usize,
    /// Rows of text on the last draw (for paging).
    pub height: usize,
}

impl SnippetView {
    pub fn new(file: File) -> SnippetView {
        SnippetView { file, text: None, lines: vec![], scroll: 0, height: 0 }
    }

    pub fn set_text(&mut self, result: Result<String, String>) {
        if let Ok(t) = &result {
            self.lines = super::highlight::highlight(t, &[&self.file.title, &self.file.name], &self.file.filetype, Style::default().fg(super::theme::p().code_fg));
        }
        self.text = Some(result);
    }

    fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let max = self.line_count().saturating_sub(self.height.max(1));
        self.scroll = (self.scroll as isize + delta).clamp(0, max as isize) as usize;
    }

    pub fn page(&self) -> isize {
        self.height.saturating_sub(1).max(1) as isize
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect) {
        super::media::clear_popup(f, area);
        let name = if self.file.title.is_empty() { &self.file.name } else { &self.file.title };
        let lang = super::highlight::language(&self.file).unwrap_or(&self.file.pretty_type);
        let lang = if lang.is_empty() { String::new() } else { format!(" · {lang}") };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(super::theme::p().accent))
            .title(format!(" {name}{lang} "));
        let inner = block.inner(area);
        self.height = inner.height as usize;
        let lines: Vec<Line> = match &self.text {
            None => vec![Line::styled("Loading…", Style::default().fg(super::theme::p().dim))],
            Some(Err(e)) => vec![Line::styled(format!("cannot load the snippet: {e}"), Style::default().fg(super::theme::p().danger))],
            Some(Ok(_)) => {
                let digits = self.lines.len().max(1).to_string().len();
                let room = (inner.width as usize).saturating_sub(digits + 3);
                self.lines
                    .iter()
                    .enumerate()
                    .skip(self.scroll)
                    .take(self.height)
                    .map(|(n, spans)| {
                        let mut line = vec![Span::styled(format!("{:>digits$} │ ", n + 1), Style::default().fg(super::theme::p().dim))];
                        line.extend(super::highlight::clip_spans(spans.clone(), room));
                        Line::from(line)
                    })
                    .collect()
            }
        };
        f.render_widget(Paragraph::new(lines).block(block), area);
        if matches!(self.text, Some(Ok(_))) {
            super::scroll::indicator(f, area, self.scroll, self.height, self.lines.len(), Style::default().fg(super::theme::p().accent));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrolling_stops_at_the_end() {
        let mut v = SnippetView::new(File::default());
        v.set_text(Ok((1..=30).map(|n| n.to_string()).collect::<Vec<_>>().join("\n")));
        v.height = 10;
        v.scroll_by(100);
        assert_eq!(v.scroll, 20);
        v.scroll_by(-3);
        assert_eq!(v.scroll, 17);
        v.scroll_by(-100);
        assert_eq!(v.scroll, 0);
    }
}

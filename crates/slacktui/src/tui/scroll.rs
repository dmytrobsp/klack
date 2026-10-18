//! Scroll indicators: a thumb on a pane's right border showing which part of the content is
//! on screen (nothing when everything fits).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState};

/// Draw the indicator over the right border of the bordered `area`: items `first..first + shown`
/// of `total` are on screen (lines, messages, entries — whatever the pane counts in).
pub fn indicator(f: &mut Frame, area: Rect, first: usize, shown: usize, total: usize, border: Style) {
    if total <= shown || shown == 0 || area.height < 3 || area.width < 2 {
        return;
    }
    // Between the corners of the border.
    let track = Rect { x: area.right() - 1, y: area.y + 1, width: 1, height: area.height - 2 };
    let mut state = ScrollbarState::new(total - shown + 1).position(first.min(total - shown)).viewport_content_length(shown);
    let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(Some("│"))
        .track_style(border)
        .thumb_symbol("┃")
        .thumb_style(Style::default().fg(super::theme::p().strong));
    f.render_stateful_widget(bar, track, &mut state);
}

#[cfg(test)]
mod tests {
    use ratatui::style::Style;

    /// The border column of a 3-wide, 12-high pane after drawing the indicator.
    fn column(first: usize, shown: usize, total: usize) -> String {
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(3, 12)).unwrap();
        terminal
            .draw(|f| {
                let area = f.area();
                f.render_widget(ratatui::widgets::Block::bordered(), area);
                super::indicator(f, area, first, shown, total, Style::default());
            })
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..12).map(|y| buf[(2, y)].symbol().to_string()).collect()
    }

    #[test]
    fn thumb_shows_the_visible_part() {
        assert_eq!(column(0, 10, 10), "┐││││││││││┘", "everything fits: just the border");
        assert_eq!(column(0, 5, 10), "┐┃┃┃┃┃│││││┘", "the top half");
        assert_eq!(column(5, 5, 10), "┐│││││┃┃┃┃┃┘", "the bottom half");
        assert_eq!(column(0, 1, 100), "┐┃│││││││││┘", "a small thumb stays one cell");
        assert_eq!(column(9, 1, 100), "┐│┃││││││││┘", "9 % down: the second of ten cells");
        assert_eq!(column(99, 1, 100), "┐│││││││││┃┘", "at the end");
        assert_eq!(column(500, 10, 100), column(90, 10, 100), "past the end: at the end");
    }
}

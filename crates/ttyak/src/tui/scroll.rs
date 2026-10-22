//! Scroll indicators: a thumb on a pane's right border showing which part of the content is
//! on screen (nothing when everything fits).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;

/// The thumb is never shorter than this (when the track allows), however long the content.
pub const MIN_THUMB: usize = 2;

/// Draw the indicator over the right border of the bordered `area`: items `first..first + shown`
/// of `total` are on screen (lines, messages, entries — whatever the pane counts in).
pub fn indicator(f: &mut Frame, area: Rect, first: usize, shown: usize, total: usize, border: Style) {
    if area.height < 3 || area.width < 2 {
        return;
    }
    // Between the corners of the border.
    let (x, top, track) = (area.right() - 1, area.y + 1, (area.height - 2) as usize);
    let Some((start, len)) = thumb(track, first, shown, total) else { return };
    let buf = f.buffer_mut();
    for i in 0..track {
        let cell = &mut buf[(x, top + i as u16)];
        if (start..start + len).contains(&i) {
            cell.set_symbol("┃").set_style(Style::default().fg(super::theme::p().strong));
        } else {
            cell.set_symbol("│").set_style(border);
        }
    }
}

/// [`indicator`] on the right border of the bordered `popup`, beside the list in `rows` only.
pub fn indicator_beside(f: &mut Frame, popup: Rect, rows: Rect, first: usize, shown: usize, total: usize, border: Style) {
    let area = Rect { x: popup.x, y: rows.y.saturating_sub(1), width: popup.width, height: rows.height + 2 };
    indicator(f, area, first, shown, total, border);
}

/// Where the thumb goes on a track of `track` cells: (first cell, length), or None when
/// everything fits. Its length follows the part on screen but is at least [`MIN_THUMB`]; it
/// touches the top only at the top and the bottom only at the end.
fn thumb(track: usize, first: usize, shown: usize, total: usize) -> Option<(usize, usize)> {
    if total <= shown || shown == 0 || track == 0 {
        return None;
    }
    let len = ((shown * track + total / 2) / total).max(MIN_THUMB).min(track);
    let (room, last) = (track - len, total - shown);
    let first = first.min(last);
    // Rounded, but off the ends unless really there.
    let mut start = (first * room + last / 2) / last;
    if first > 0 && start == 0 && room > 0 {
        start = 1;
    }
    if first < last && start == room && room > 0 {
        start = room - 1;
    }
    Some((start, len))
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
        assert_eq!(column(0, 1, 100), "┐┃┃││││││││┘", "never shorter than the minimum");
        assert_eq!(column(99, 1, 100), "┐││││││││┃┃┘", "at the end");
        assert_eq!(column(1, 1, 100), "┐│┃┃│││││││┘", "one step down leaves the top");
        assert_eq!(column(98, 1, 100), "┐│││││││┃┃│┘", "one step short of the end leaves the bottom");
        assert_eq!(column(500, 10, 100), column(90, 10, 100), "past the end: at the end");
    }

    #[test]
    fn thumb_sizes() {
        use super::{MIN_THUMB, thumb};
        assert_eq!(thumb(10, 0, 10, 10), None);
        assert_eq!(thumb(10, 0, 1, 10_000), Some((0, MIN_THUMB)));
        assert_eq!(thumb(1, 0, 1, 10_000), Some((0, 1)), "a one-cell track: one cell");
        assert_eq!(thumb(20, 5, 10, 20), Some((5, 10)), "proportional when large enough");
        assert_eq!(thumb(0, 0, 1, 2), None);
    }
}

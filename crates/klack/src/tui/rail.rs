//! The workspace rail (with two or more workspaces): a tile per workspace left of the
//! sidebar — its initials in its own colour, the shown one marked, below it what needs you
//! there — and a click on one switches, as Alt+1…9 do.

use std::collections::{HashMap, HashSet};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use ratatui_image::sliced::{SignedPosition, SlicedImage};
use slack_core::types::File;

use super::media::{Media, Thumb};
use super::others::Badge;

/// Columns taken: a marker, a four-cell tile, a space.
pub const WIDTH: u16 = 6;

/// Draw the rail in `area`: a workspace's logo over its tile where it has one and the
/// terminal draws pictures, else its initials; a signed-out one (`signed_out`) dim, with `⊘`
/// where its badge goes. Returns each workspace's tile (for clicks) and the logos to load.
#[allow(clippy::too_many_arguments)]
pub fn draw(f: &mut Frame, area: Rect, labels: &[String], shown: usize, badges: &HashMap<usize, Badge>, logos: &HashMap<usize, File>, media: &Media, signed_out: &HashSet<usize>) -> (Vec<Rect>, Vec<(File, (u16, u16))>) {
    let p = super::theme::p();
    let mut tiles = Vec::new();
    let mut wanted = Vec::new();
    for (i, label) in labels.iter().enumerate() {
        // Three rows each: the tile (two) and its badge.
        let y = area.y + 1 + i as u16 * 3;
        if y + 2 > area.bottom() {
            break;
        }
        // Two accounts in one workspace: the same initials, their own colours.
        let team = label.split(" · ").next().unwrap_or(label);
        let out = signed_out.contains(&i);
        let tile = if out { Style::default().fg(p.dim) } else { Style::default().bg(super::text::user_color(label)).fg(Color::Black).add_modifier(Modifier::BOLD) };
        let marker = if i == shown { Span::styled("▌", Style::default().fg(p.accent)) } else { Span::raw(" ") };
        let badge = match badges.get(&i).filter(|_| i != shown) {
            _ if out => Span::styled("  ⊘", Style::default().fg(p.dim)),
            Some(Badge { attention: n, .. }) if *n > 0 => Span::styled(format!(" @{n}"), Style::default().fg(p.badge_fg).bg(p.badge_bg).add_modifier(Modifier::BOLD)),
            Some(Badge { unread: true, .. }) => Span::styled("  •", Style::default().fg(p.strong).add_modifier(Modifier::BOLD)),
            _ => Span::raw(""),
        };
        let lines = vec![
            Line::from(vec![marker.clone(), Span::styled(format!(" {} ", super::messages::initials(team)), tile)]),
            Line::from(vec![marker, Span::styled(format!(" {:<3}", i + 1), tile.remove_modifier(Modifier::BOLD))]),
            Line::from(badge),
        ];
        let rows = Rect { x: area.x, y, width: area.width, height: 3.min(area.bottom() - y) };
        f.render_widget(Paragraph::new(lines), rows);
        let tile = Rect { x: area.x + 1, y, width: 4, height: 2 };
        // The initials stand in while the logo loads.
        if let Some(logo) = logos.get(&i).filter(|_| media.graphics() && !out) {
            match media.thumbs.get(&logo.id) {
                Some(Thumb::Ready(proto)) => f.render_widget(SlicedImage::new(proto, SignedPosition { x: 0, y: 0 }), tile),
                Some(_) => {}
                None => wanted.push((logo.clone(), super::media::AVATAR_SIZE_LARGE)),
            }
        }
        tiles.push(tile);
    }
    (tiles, wanted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_marks_and_badges() {
        let labels: Vec<String> = ["Test Bed · ann", "Acme Corp", "Test Bed · bob"].map(String::from).into();
        let badges = HashMap::from([(1, Badge { attention: 7, unread: true }), (2, Badge { attention: 0, unread: true }), (0, Badge { attention: 3, unread: true })]);
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(WIDTH, 12)).unwrap();
        let mut tiles = vec![];
        let mut wanted = vec![];
        // A logo for the second workspace: wanted with real graphics only.
        let logos = HashMap::from([(1, File { id: "avatar:team:T2".into(), ..Default::default() })]);
        let halfblocks = Media::with(ratatui_image::picker::Picker::halfblocks());
        terminal.draw(|f| (tiles, wanted) = draw(f, f.area(), &labels, 0, &badges, &logos, &halfblocks, &HashSet::new())).unwrap();
        assert!(wanted.is_empty(), "no pictures: the initials");
        let buf = terminal.backend().buffer();
        let row = |y: u16| (0..WIDTH).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>();
        assert_eq!([row(1), row(2), row(3)], ["▌ TB  ", "▌ 1   ", "      "], "the shown one marked, no badge for it");
        assert_eq!([row(4), row(5), row(6)], ["  AC  ", "  2   ", " @7   "]);
        assert_eq!([row(7), row(9)], ["  TB  ", "  •   "], "two accounts in one workspace: the same initials");
        assert_ne!(buf[(1, 7)].bg, buf[(1, 1)].bg, "but their own colours");
        assert_eq!(tiles, [Rect::new(1, 1, 4, 2), Rect::new(1, 4, 4, 2), Rect::new(1, 7, 4, 2)]);
        // No room for the third.
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(WIDTH, 7)).unwrap();
        terminal.draw(|f| tiles = draw(f, f.area(), &labels, 0, &badges, &HashMap::new(), &halfblocks, &HashSet::new()).0).unwrap();
        assert_eq!(tiles.len(), 2);
        let mut kitty = Media::with(ratatui_image::picker::Picker::halfblocks());
        kitty.picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
        terminal.draw(|f| wanted = draw(f, f.area(), &labels, 0, &badges, &logos, &kitty, &HashSet::new()).1).unwrap();
        assert_eq!(wanted.iter().map(|(f, size)| (f.id.as_str(), *size)).collect::<Vec<_>>(), [("avatar:team:T2", (4, 2))], "pictures: its logo, over the tile");
    }

    #[test]
    fn a_signed_out_workspace_is_dim_with_a_mark() {
        let labels: Vec<String> = ["Test Bed", "Acme Corp", "Widgets"].map(String::from).into();
        let badges = HashMap::from([(1, Badge { attention: 7, unread: true })]);
        let logos = HashMap::from([(1, File { id: "avatar:team:T2".into(), ..Default::default() })]);
        let mut kitty = Media::with(ratatui_image::picker::Picker::halfblocks());
        kitty.picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(WIDTH, 12)).unwrap();
        let mut tiles = vec![];
        let mut wanted = vec![];
        terminal.draw(|f| (tiles, wanted) = draw(f, f.area(), &labels, 0, &badges, &logos, &kitty, &HashSet::from([1]))).unwrap();
        let buf = terminal.backend().buffer();
        let row = |y: u16| (0..WIDTH).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>();
        assert_eq!([row(4), row(5), row(6)], ["  AC  ", "  2   ", "  ⊘   "], "the mark instead of its badge, the rail as wide");
        let dim = super::super::theme::p().dim;
        for x in 1..5 {
            assert_eq!((buf[(x, 4)].fg, buf[(x, 4)].bg), (dim, Color::Reset), "a dim tile, not its colour");
        }
        assert_eq!(buf[(2, 6)].fg, dim);
        assert_ne!(buf[(1, 7)].bg, Color::Reset, "the others keep their colours");
        assert!(wanted.is_empty(), "no logo fetched for it");
        assert_eq!(tiles.len(), 3, "it can still be clicked (to be told why not)");
    }
}

//! Filterable emoji picker popup (used for reactions).

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use std::collections::{HashMap, HashSet};


/// Emoji people reach for most; shown first when the query is empty.
const FAVOURITES: &[&str] = &["+1", "heart", "joy", "tada", "eyes", "white_check_mark", "pray", "fire", "rocket", "thinking_face", "100", "raised_hands"];

#[derive(Debug, Clone)]
pub struct Candidate {
    pub name: String,
    /// The emoji itself; blank for a workspace's own picture.
    pub glyph: String,
    /// One of the workspace's own (a picture or an alias).
    pub custom: bool,
    /// Already on the message: you reacted with it (choosing it removes it), and how many did.
    pub mine: bool,
    pub on_message: bool,
    pub count: u64,
    /// Who reacted (user ids), yours first; empty for an emoji not on the message.
    pub users: Vec<String>,
}

/// A reaction already on the message the picker is for.
#[derive(Debug, Clone)]
struct Present {
    name: String,
    count: u64,
    mine: bool,
    /// Who reacted, in Slack's order (may hold fewer than `count` on a big reaction).
    users: Vec<String>,
}

/// "you, alice, +2": the people that fit in `width` columns, yours first; the rest as "+N" (of `total`).
fn people_line(names: &[String], total: u64, width: usize) -> String {
    let w = |s: &str| s.chars().count();
    for k in (0..=names.len()).rev() {
        let rest = total.saturating_sub(k as u64);
        let mut parts: Vec<String> = names[..k].to_vec();
        if rest > 0 {
            parts.push(format!("+{rest}"));
        }
        let text = parts.join(", ");
        if w(&text) <= width && !text.is_empty() {
            return text;
        }
    }
    String::new()
}

#[derive(Debug, Default)]
pub struct EmojiPicker {
    pub query: String,
    pub selected: usize,
    /// The whole popup and its list as last drawn, and the first candidate shown (for the mouse).
    pub area: Rect,
    list: Rect,
    first: usize,
    /// The row the pointer is over (not the selection), at the last `Moved`.
    pub hover: Option<usize>,
    /// The message's reactions when the picker opened, in its order.
    present: Vec<Present>,
}

impl EmojiPicker {
    /// Candidates for the current query: the message's reactions that match (yours first, in
    /// its order, then the others'), then the favourites (empty query) or the exact, prefix and
    /// substring matches, each name once.
    /// `custom` is the workspace's own emoji: name → picture URL or `alias:name`.
    pub fn candidates(&self, custom: &HashMap<String, String>) -> Vec<Candidate> {
        let q = self.query.trim_matches(':').to_lowercase();
        let mut out: Vec<Candidate> = Vec::new();
        for mine in [true, false] {
            for r in self.present.iter().filter(|r| r.mine == mine && r.name.contains(&q)) {
                let glyph = super::text::glyph(custom, &r.name).unwrap_or_else(|| "  ".into());
                let base = r.name.split("::").next().unwrap_or_default();
                out.push(Candidate { name: r.name.clone(), glyph, custom: custom.contains_key(base), mine, on_message: true, count: r.count, users: r.users.clone() });
            }
        }
        let mut seen: HashSet<String> = out.iter().map(|c| c.name.clone()).collect();
        out.extend(Self::matches(&q, custom).into_iter().filter(|c| seen.insert(c.name.clone())));
        out.truncate(200);
        out
    }

    /// The favourites (empty `q`), or the emoji matching `q`: exact, then prefix, then substring.
    fn matches(q: &str, custom: &HashMap<String, String>) -> Vec<Candidate> {
        let mut out: Vec<(u8, Candidate)> = Vec::new();
        if q.is_empty() {
            for name in FAVOURITES {
                if let Some(e) = emojis::get_by_shortcode(name) {
                    out.push((0, Candidate { name: name.to_string(), glyph: e.as_str().to_string(), custom: false, mine: false, on_message: false, count: 0, users: vec![] }));
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
                out.push((rank, Candidate { name: code.to_string(), glyph: e.as_str().to_string(), custom: false, mine: false, on_message: false, count: 0, users: vec![] }));
            }
        }
        for name in custom.keys() {
            let rank = if *name == q {
                0
            } else if name.starts_with(&q) {
                1
            } else if name.contains(&q) {
                2
            } else {
                continue;
            };
            let glyph = super::text::custom_glyph(custom, name).unwrap_or_else(|| "  ".into());
            out.push((rank, Candidate { name: name.clone(), glyph, custom: true, mine: false, on_message: false, count: 0, users: vec![] }));
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

    /// A picker for a message with `reactions`; `me` is your user id.
    pub fn for_message(reactions: &[slack_core::types::Reaction], me: &str) -> EmojiPicker {
        let present = reactions.iter().map(|r| Present { name: r.name.clone(), count: r.count, mine: r.users.iter().any(|u| u == me), users: r.users.iter().filter(|u| *u == me).chain(r.users.iter().filter(|u| *u != me)).cloned().collect() }).collect();
        EmojiPicker { present, ..Default::default() }
    }

    /// A picker already searching for `query`.
    pub fn searching(query: &str) -> EmojiPicker {
        EmojiPicker { query: query.to_string(), ..Default::default() }
    }

    /// The candidate drawn at a terminal position.
    pub fn row_at(&self, column: u16, row: u16) -> Option<usize> {
        self.list.contains((column, row).into()).then(|| self.first + (row - self.list.y) as usize)
    }

    /// The pointer moved to a position: hover the candidate there (none off the list).
    pub fn hover_at(&mut self, column: u16, row: u16) {
        self.hover = self.row_at(column, row);
    }

    /// `name` gives a user's display name; `me` is your user id.
    pub fn draw(&mut self, f: &mut Frame, area: Rect, custom: &HashMap<String, String>, me: &str, name: &dyn Fn(&str) -> String) {
        let [popup] = Layout::horizontal([Constraint::Length(44)]).flex(Flex::Center).areas(area);
        let [popup] = Layout::vertical([Constraint::Length(16)]).flex(Flex::Center).areas(popup);
        self.area = popup;
        super::media::clear_modal(f, popup);
        let block = Block::default().borders(Borders::ALL).border_style(Style::default().fg(super::theme::p().accent)).title(format!(" {} ", super::i18n::tr("Add reaction")));
        let inner = block.inner(popup);
        f.render_widget(block, popup);
        let [input, list] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);
        f.render_widget(Paragraph::new(Line::from(vec![Span::styled("search: ", Style::default().fg(super::theme::p().dim)), Span::raw(self.query.clone())])), input);
        f.set_cursor_position((input.x + 8 + self.query.chars().count() as u16, input.y));
        let cands = self.candidates(custom);
        let height = list.height as usize;
        let start = self.selected.saturating_sub(height.saturating_sub(1));
        (self.list, self.first) = (Rect { height: list.height.min(cands.len().saturating_sub(start) as u16), ..list }, start);
        let lines: Vec<Line> = cands
            .iter()
            .enumerate()
            .skip(start)
            .take(height)
            .map(|(n, c)| {
                // Yours: a ✓ and the name in the accent colour, "remove" at the end; others' on the message: their count.
                let pal = super::theme::p();
                let mark = if c.mine { Span::styled("✓", Style::default().fg(pal.accent).add_modifier(Modifier::BOLD)) } else { Span::raw(" ") };
                let mut spans = vec![mark, Span::raw(format!("{} ", c.glyph)), Span::styled(format!(":{}:", c.name), Style::default().fg(if c.mine { pal.accent } else { pal.dim }))];
                let lead = if c.mine { super::i18n::tr("remove").into_owned() } else if c.on_message { format!("· {}", c.count) } else { String::new() };
                let mut hint = lead;
                if c.on_message && !c.users.is_empty() {
                    let names: Vec<String> = c.users.iter().map(|u| if u == me { "you".to_string() } else { name(u) }).collect();
                    let room = (list.width as usize).saturating_sub(spans.iter().map(|s| s.width()).sum::<usize>() + hint.chars().count() + 4);
                    let people = people_line(&names, c.count, room);
                    if !people.is_empty() {
                        hint = format!("{hint} · {people}");
                    }
                }
                if !hint.is_empty() {
                    let used: usize = spans.iter().map(|s| s.width()).sum::<usize>() + hint.chars().count();
                    spans.push(Span::raw(" ".repeat((list.width as usize).saturating_sub(used + 1).max(1))));
                    spans.push(Span::styled(hint, Style::default().fg(pal.dim)));
                }
                let line = Line::from(spans);
                if n == self.selected { line.style(Style::default().bg(super::theme::p().selection).add_modifier(Modifier::BOLD)) } else if self.hover == Some(n) { line.style(super::theme::hover_style()) } else { line }
            })
            .collect();
        let lines = if lines.is_empty() { vec![Line::styled(" no matching emoji", Style::default().fg(super::theme::p().dim))] } else { lines };
        f.render_widget(Paragraph::new(lines), list);
        super::scroll::indicator_beside(f, popup, list, start, height, cands.len(), Style::default().fg(super::theme::p().accent));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicks_find_the_emoji() {
        let mut p = EmojiPicker::default();
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 30)).unwrap();
        terminal.draw(|f| p.draw(f, f.area(), &HashMap::new(), "U1", &|u| u.to_string())).unwrap();
        // 44×16 centred: rows 7–22, the border, the search row, then the list from row 9.
        assert_eq!(p.area, Rect::new(8, 7, 44, 16));
        assert_eq!((p.row_at(20, 9), p.row_at(20, 10), p.row_at(20, 8), p.row_at(2, 9)), (Some(0), Some(1), None, None));
        let n = p.candidates(&HashMap::new()).len() as u16;
        assert_eq!((p.row_at(20, 9 + n - 1), p.row_at(20, 9 + n)), (Some(n as usize - 1), None), "the last favourite, then nothing below it");
        // Thirteen rows of list: with the 21st selected, the list starts at the 9th.
        p.query = "e".into();
        p.selected = 20;
        terminal.draw(|f| p.draw(f, f.area(), &HashMap::new(), "U1", &|u| u.to_string())).unwrap();
        assert_eq!(p.row_at(20, 9), Some(8), "a scrolled list counts from the first row shown");
    }

    #[test]
    fn favourites_when_empty() {
        let p = EmojiPicker::default();
        let c = p.candidates(&HashMap::new());
        assert_eq!(c[0].name, "+1");
        assert_eq!(c[0].glyph, "👍");
    }

    #[test]
    fn ranking_prefers_exact_then_prefix() {
        let p = EmojiPicker { query: "tada".into(), ..Default::default() };
        assert_eq!(p.candidates(&HashMap::new())[0].name, "tada");
        let p = EmojiPicker { query: "hea".into(), ..Default::default() };
        let names: Vec<String> = p.candidates(&HashMap::new()).into_iter().map(|c| c.name).collect();
        assert!(names.iter().take(5).any(|n| n == "heart"), "{names:?}");
    }

    #[test]
    fn includes_custom_emoji() {
        let p = EmojiPicker { query: "parrot".into(), ..Default::default() };
        let custom = HashMap::from([("partyparrot".to_string(), "https://emoji.slack-edge.com/T1/partyparrot/1.gif".to_string()), ("shipit".into(), "alias:squirrel".into())]);
        let c = p.candidates(&custom);
        assert!(c.iter().any(|c| c.name == "partyparrot" && c.custom && c.glyph.trim().is_empty()), "a picture: no glyph");
        let p = EmojiPicker { query: "shipit".into(), ..Default::default() };
        let c = &p.candidates(&custom)[0];
        assert_eq!((c.name.as_str(), c.glyph.as_str()), ("shipit", "🐿️"), "an alias shows its emoji (Slack's name for it)");
    }

    /// A message with reactions from you (U1) and others.
    fn on_message() -> Vec<slack_core::types::Reaction> {
        let r = |name: &str, users: &[&str]| slack_core::types::Reaction { name: name.into(), users: users.iter().map(|u| u.to_string()).collect(), count: users.len() as u64 };
        vec![r("eyes", &["U2"]), r("tada", &["U2", "U1"]), r("partyparrot", &["U3"]), r("+1::skin-tone-2", &["U1"])]
    }

    #[test]
    fn your_reactions_come_first_then_others_then_favourites() {
        let custom = HashMap::from([("partyparrot".to_string(), "https://emoji.slack-edge.com/T1/partyparrot/1.gif".to_string())]);
        let p = EmojiPicker::for_message(&on_message(), "U1");
        let c = p.candidates(&custom);
        let names: Vec<&str> = c.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(&names[..6], ["tada", "+1::skin-tone-2", "eyes", "partyparrot", "+1", "heart"], "yours in the message's order, then the others', then the favourites");
        assert_eq!(names.iter().filter(|n| **n == "eyes" || **n == "tada").count(), 2, "a favourite on the message is listed once: {names:?}");
        let marks: Vec<(bool, bool, u64)> = c.iter().take(5).map(|c| (c.mine, c.on_message, c.count)).collect();
        assert_eq!(marks, [(true, true, 2), (true, true, 1), (false, true, 1), (false, true, 1), (false, false, 0)]);
        assert_eq!((c[1].glyph.as_str(), c[3].custom, c[3].glyph.trim()), ("👍🏻", true, ""), "a skin tone shows; a picture has no glyph");
    }

    #[test]
    fn yours_are_marked_in_the_list() {
        let mut p = EmojiPicker::for_message(&on_message(), "U1");
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 30)).unwrap();
        terminal.draw(|f| p.draw(f, f.area(), &HashMap::new(), "U1", &|u| u.to_string())).unwrap();
        let buf = terminal.backend().buffer();
        let row = |y: u16| (9..51).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>();
        // The list starts at row 9, column 9 (inside the border of the 44-wide popup at column 8).
        assert!(row(9).starts_with("✓") && row(9).contains(":tada:") && row(9).trim_end().ends_with("remove · you, U2"), "{:?}", row(9));
        assert_eq!(buf[(9, 9)].fg, crate::tui::theme::p().accent, "the mark is in the accent colour");
        assert!(row(11).starts_with(" ") && row(11).contains(":eyes:") && row(11).trim_end().ends_with("· 1 · U2"), "{:?}", row(11));
        assert!(!row(13).contains("remove") && !row(13).contains('·'), "a favourite has no hint: {:?}", row(13));
    }

    #[test]
    fn picker_shows_who_reacted() {
        let r = |name: &str, users: &[&str], count: u64| slack_core::types::Reaction { name: name.into(), users: users.iter().map(|u| u.to_string()).collect(), count };
        let reactions = vec![r("tada", &["U2", "U1", "U9"], 3), r("eyes", &["U2", "U3", "U4", "U5", "U6", "U8"], 12), r("fire", &["U7"], 1)];
        let names = |u: &str| match u { "U2" => "alice".to_string(), "U3" => "bob".to_string(), o => o.to_string() };
        let mut p = EmojiPicker::for_message(&reactions, "U1");
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 30)).unwrap();
        terminal.draw(|f| p.draw(f, f.area(), &HashMap::new(), "U1", &names)).unwrap();
        let buf = terminal.backend().buffer();
        let row = |y: u16| (9..51).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>().trim_end().to_string();
        assert!(row(9).contains(":tada:") && row(9).ends_with("remove · you, alice, U9"), "you first, then Slack's order, an unknown user by id: {:?}", row(9));
        assert!(row(10).contains(":eyes:") && row(10).ends_with("· 12 · alice, bob, U4, U5, +8"), "narrow: cut with +N: {:?}", row(10));
        assert!(row(11).contains(":fire:") && row(11).ends_with("· 1 · U7"), "{:?}", row(11));
        assert_eq!(people_line(&["you".into(), "alice".into(), "bob".into()], 3, 9), "you, +2");
        assert_eq!(people_line(&["you".into()], 1, 1), "");
        assert_eq!(people_line(&["a".into()], 5, 20), "a, +4", "users fewer than the count");
    }

    #[test]
    fn a_query_keeps_the_grouping() {
        let mut p = EmojiPicker::for_message(&on_message(), "U1");
        p.query = "a".into();
        let names: Vec<String> = p.candidates(&HashMap::new()).into_iter().map(|c| c.name).collect();
        assert_eq!(&names[..3], ["tada", "partyparrot", "a"], "matching reactions on the message before the best match");
        assert_eq!(names.iter().filter(|n| *n == "tada").count(), 1, "{names:?}");
    }

    #[test]
    fn hover_follows_the_pointer_and_leaves_the_selection() {
        let mut p = EmojiPicker::default();
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 30)).unwrap();
        terminal.draw(|f| p.draw(f, f.area(), &HashMap::new(), "U1", &|u| u.to_string())).unwrap();
        p.hover_at(20, 10);
        assert_eq!((p.hover, p.selected), (Some(1), 0));
        p.hover_at(20, 8);
        assert_eq!(p.hover, None, "the search row");
    }
}

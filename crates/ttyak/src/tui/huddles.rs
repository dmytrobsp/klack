//! Huddles, a view in the main pane: the huddles happening now in your conversations ("Live now": the one you
//! are in first, where, for how long and who is in it) and the recent ones from `huddles.history` ("Recent",
//! newest first, in pages of 30: where, when, how long, who took part, a missed one in the alert colour, how many
//! replies its thread has). Enter on a live one opens its conversation (the huddle key joins it); on a recent one,
//! its thread.

use std::collections::{HashMap, HashSet};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use slack_core::types::PastHuddle;

use super::i18n::tr;
use super::media::Media;
use super::state::Workspace;

/// Recent huddles asked for at a time (`huddles.history` takes at most 30).
pub const PAGE: u32 = 30;

/// What a row of the list belongs to: a live huddle (its channel) or a recent one (an index into the list loaded).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Live(String),
    Past(usize),
}

const LIVE_NOW: &str = "Live now";
const RECENT: &str = "Recent";
const LIVE: &str = "Live";
const YOU_ARE_IN_IT: &str = "you are in it";
const MISSED: &str = "Missed huddle";
const NONE_LIVE: &str = "No huddles going on in your conversations.";
const NONE_RECENT: &str = "No recent huddles.";
const LOADING: &str = "Loading…";
const LOADING_MORE: &str = "Loading more…";
const FAILED: &str = "The recent huddles could not be loaded";
const JUST_NOW: &str = "just now";
const MINUTES_AGO: &str = "{n} min ago";
const HOURS_AGO: &str = "{n} h ago";
const DAYS_AGO: &str = "{n} d ago";
const SECONDS: &str = "{n} s";
const MINUTES: &str = "{n} min";
const HOURS: &str = "{h} h {m} min";
const ONE_REPLY: &str = "1 reply";
const REPLIES: &str = "{n} replies";

/// The view's words, for the locale template.
pub const WORDS: &[&str] = &[
    "Huddles", LIVE_NOW, RECENT, LIVE, YOU_ARE_IN_IT, MISSED, NONE_LIVE, NONE_RECENT, LOADING, LOADING_MORE, FAILED, JUST_NOW, MINUTES_AGO, HOURS_AGO, DAYS_AGO, SECONDS, MINUTES, HOURS, ONE_REPLY, REPLIES,
];

#[derive(Default)]
pub struct Huddles {
    /// The recent huddles loaded (as `huddles.history` gives them; the list leaves out those not ended yet).
    pub past: Option<Vec<PastHuddle>>,
    /// Where the next page starts (none: it is all there), and whether a page is being loaded.
    pub cursor: Option<String>,
    pub loading: bool,
    /// Why the recent huddles could not be loaded (said in the view).
    pub failed: Option<String>,
    /// The load the pages that come belong to: one for an older load is dropped.
    pub generation: u64,
    /// The first page is being loaded again with the list kept on screen meanwhile.
    refreshing: bool,
    /// The selected row; none: the first.
    pub selected: Option<Row>,
    /// The recent huddle (its id) selected when the list was loaded afresh: selected again once it is back.
    keep: Option<String>,
    /// Lines of the list above the view.
    pub scroll: usize,
    /// Bring the selection into view at the next draw (it moved by the keys).
    follow: bool,
    /// Scrolled since the last draw (the keys and the wheel move the selection instead): only then does the selection
    /// follow the view.
    scrolled: bool,
    /// The selection and the row its first line was on at the last draw (above the view: negative).
    spot: Option<(Row, isize)>,
    /// Last drawn area (empty while the view is not shown), for the mouse.
    pub area: Rect,
    /// The list's rows at the last draw, and what each row belongs to.
    pub list: Rect,
    rows: Vec<Option<Row>>,
    /// The rows that can be selected, in the order drawn.
    order: Vec<Row>,
    /// The row the pointer is over (not the selection), at the last `Moved`.
    pub hover: Option<Row>,
    /// The end of the list was on screen at the last draw.
    at_end: bool,
    /// Reply counts of the recent huddles' threads, by (channel, thread ts), and those asked for.
    pub replies: HashMap<(String, String), u64>,
    pub requested: HashSet<(String, String)>,
}

impl Huddles {
    /// Load the recent huddles afresh: the list goes (the selection stays where it can) and the reply counts are
    /// asked for again (those known show meanwhile); returns the new load's generation.
    pub fn reload(&mut self) -> u64 {
        self.generation += 1;
        if let Some(Row::Past(n)) = self.selected {
            self.keep = self.past_at(n).map(|h| h.id.clone());
        }
        (self.past, self.cursor, self.loading, self.failed, self.at_end, self.refreshing) = (None, None, true, None, false, false);
        self.requested.clear();
        self.generation
    }

    /// Load the recent huddles again (a huddle it concerns ended) keeping the list, the selection and the reply
    /// counts on screen until the first page comes; returns the new load's generation.
    pub fn refresh(&mut self) -> u64 {
        if self.past.is_none() {
            return self.reload();
        }
        self.generation += 1;
        if let Some(Row::Past(n)) = self.selected {
            self.keep = self.past_at(n).map(|h| h.id.clone());
        }
        (self.loading, self.refreshing) = (true, true);
        self.generation
    }

    /// A huddle that ended in one of `channels` concerns the view: it was live there, or the view lists recent
    /// huddles of its channel.
    pub fn concerns(&self, channels: &[String]) -> bool {
        channels.iter().any(|c| self.order.contains(&Row::Live(c.clone())) || self.past.as_ref().is_some_and(|p| p.iter().any(|h| h.channel == *c)))
    }

    /// A page of load `generation` came (`more`: a next page): listed (those listed already left out).
    pub fn add_page(&mut self, generation: u64, more: bool, page: Vec<PastHuddle>, cursor: Option<String>) {
        if generation != self.generation || (more && !self.loading) {
            return;
        }
        let past = self.past.get_or_insert_with(Vec::new);
        if !more {
            past.clear();
        }
        for h in page {
            if !past.iter().any(|o| o.id == h.id) {
                past.push(h);
            }
        }
        if let Some(id) = self.keep.take() {
            self.selected = past.iter().position(|h| h.id == id).map(Row::Past).or(self.selected.take());
        }
        (self.cursor, self.loading, self.failed, self.refreshing) = (cursor, false, None, false);
    }

    /// Load `generation` failed: said when nothing is listed; what is listed stays.
    pub fn fail(&mut self, generation: u64, error: String) {
        if generation != self.generation {
            return;
        }
        if self.past.is_none() {
            self.failed = Some(error);
        }
        (self.loading, self.refreshing) = (false, false);
    }

    /// Where the next page starts, when the end of the list was on screen and there is more (once: it is loading then).
    pub fn next_page(&mut self) -> Option<String> {
        let cursor = self.cursor.clone().filter(|_| self.at_end && !self.loading && self.past.is_some())?;
        self.loading = true;
        Some(cursor)
    }

    /// Move the selection by `delta` rows.
    pub fn move_by(&mut self, delta: isize) {
        if self.order.is_empty() {
            return;
        }
        let at = self.selected.as_ref().and_then(|s| self.order.iter().position(|o| o == s)).unwrap_or(0) as isize;
        self.selected = Some(self.order[(at + delta).clamp(0, self.order.len() as isize - 1) as usize].clone());
        self.follow = true;
    }

    /// Rows a page moves (those on screen at the last draw).
    pub fn page(&self) -> isize {
        let mut shown: Vec<&Row> = self.rows.iter().flatten().collect();
        shown.dedup();
        shown.len().max(1) as isize
    }

    /// What is drawn at a terminal position (none on a header or a gap).
    pub fn row_at(&self, column: u16, row: u16) -> Option<Row> {
        self.list.contains((column, row).into()).then(|| self.rows.get((row - self.list.y) as usize).cloned().flatten()).flatten()
    }

    /// The pointer moved to a position: hover the row there (none off the list).
    pub fn hover_at(&mut self, column: u16, row: u16) {
        self.hover = self.row_at(column, row);
    }

    /// The selected row (none chosen: the first one drawn).
    pub fn target(&self) -> Option<Row> {
        self.selected.clone().filter(|s| self.order.contains(s)).or_else(|| self.order.first().cloned())
    }

    /// The recent huddle of a row.
    pub fn past_at(&self, n: usize) -> Option<&PastHuddle> {
        self.past.as_ref()?.get(n)
    }

    /// The threads of the recent huddles on screen whose replies are not counted (nor asked for) yet.
    pub fn wanted(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for n in self.rows.iter().flatten().filter_map(|r| if let Row::Past(n) = r { Some(*n) } else { None }) {
            let Some(key) = self.past_at(n).and_then(|h| Some((h.channel.clone(), h.thread_ts.clone()?))) else { continue };
            if !self.requested.contains(&key) && !out.contains(&key) {
                out.push(key);
            }
        }
        out
    }

    /// Not drawn this time: nothing of it is under the mouse.
    pub fn hide(&mut self) {
        (self.area, self.list, self.hover) = (Rect::default(), Rect::default(), None);
        self.rows.clear();
    }

    /// Draw the view into `area` (the main pane), its frame lit while it has the focus; `me_in`: the conversation
    /// whose huddle you are in; `now`: unix seconds.
    pub fn draw(&mut self, f: &mut Frame, area: Rect, ws: &Workspace, focused: bool, media: &Media, me_in: Option<&str>, now: i64) {
        let p = super::theme::p();
        self.area = area;
        self.rows.clear();
        self.at_end = false;
        let border = Style::default().fg(if focused { p.accent } else { p.dim });
        let block = Block::default().borders(Borders::ALL).border_style(border).title(format!(" {} ", tr("Huddles")));
        let inner = block.inner(area);
        f.render_widget(block, area);
        self.list = inner;
        let live = ws.live_list(me_in);
        let note = match &self.past {
            None if self.failed.is_some() => Some(Line::styled(format!("{}: {}", tr(FAILED), self.failed.as_deref().unwrap_or_default()), Style::default().fg(p.danger))),
            None => Some(Line::styled(tr(LOADING).into_owned(), Style::default().fg(p.dim))),
            Some(_) => None,
        };
        let (mut lines, owners) = build(ws, &live, self.past.as_deref().unwrap_or_default(), note, &self.replies, me_in, inner.width, now);
        self.order = owners.iter().flatten().cloned().collect();
        self.order.dedup();
        // (While the recent ones load, a recent one selected stays chosen.)
        if self.past.is_some() && self.selected.as_ref().is_none_or(|s| !self.order.contains(s)) {
            self.selected = self.order.first().cloned();
        }
        let height = inner.height as usize;
        let scrolled = std::mem::take(&mut self.scrolled);
        // Not scrolled (the list changed, or it was drawn at another size): the selection on its row.
        if let Some(sel) = self.selected.as_ref().filter(|_| !self.follow && !scrolled) {
            let spot = self.spot.as_ref().map(|(s, row)| (s, *row));
            (self.scroll, self.follow) = super::threads::keep_spot(&owners, self.scroll, height, sel, spot);
        }
        if std::mem::take(&mut self.follow) {
            if let Some(sel) = &self.selected {
                if let (Some(first), Some(last)) = (owners.iter().position(|o| o.as_ref() == Some(sel)), owners.iter().rposition(|o| o.as_ref() == Some(sel))) {
                    self.scroll = super::threads::bring_into_view(self.scroll, height, first, last);
                }
            }
        }
        let more = usize::from(self.loading && !self.refreshing && self.past.is_some());
        let total = lines.len() + more;
        self.scroll = self.scroll.min(total.saturating_sub(height));
        // (No room at all leaves `total` one past the lines: start at most at their end.)
        let start = self.scroll.min(lines.len());
        let end = (start + height).min(lines.len());
        // Scrolled off the selection: it follows onto the screen; the view stays.
        if let Some(next) = self.selected.as_ref().filter(|_| scrolled).and_then(|s| super::threads::follow_view(&owners, start, end, height, s)) {
            self.selected = Some(next);
        }
        self.spot = self.selected.clone().and_then(|s| Some((owners.iter().position(|o| o.as_ref() == Some(&s))? as isize - start as isize, s))).map(|(row, s)| (s, row));
        let mut shown: Vec<Line<'static>> = lines.drain(start..end).collect();
        let mut rows: Vec<Option<Row>> = owners[start..end].to_vec();
        if more > 0 && shown.len() < height {
            shown.push(Line::styled(tr(LOADING_MORE).into_owned(), Style::default().fg(p.dim)));
            rows.push(None);
        }
        let last_past = self.order.iter().rev().find(|r| matches!(r, Row::Past(_)));
        self.at_end = (height > 0 && end == owners.len()) || (last_past.is_some() && self.selected.as_ref() == last_past);
        let selected = self.selected.as_ref().filter(|_| focused);
        if let Some(sel) = selected {
            let highlight = Style::default().bg(p.selection);
            for (line, owner) in shown.iter_mut().zip(&rows) {
                if owner.as_ref() == Some(sel) {
                    *line = std::mem::take(line).patch_style(highlight);
                }
            }
        }
        super::messages::fill_rows(f, inner, &rows, &[selected], self.hover.as_ref(), &[], start, 0, media);
        f.render_widget(Paragraph::new(shown), inner);
        super::scroll::indicator(f, area, start, end - start, total, border);
        self.rows = rows;
    }
}

/// The list `width` columns wide: "Live now" with the huddles in `live` (channels, in order), then "Recent" with the
/// ended ones of `past` (not one still in the live list), and the row each line belongs to (none: headers, notes,
/// gaps). `replies`: reply counts known by (channel, thread ts); `now`: unix seconds. [`build`] with the notes
/// and your own huddle is what the view draws.
#[cfg(test)]
pub fn layout(ws: &Workspace, live: &[String], past: &[PastHuddle], replies: &HashMap<(String, String), u64>, width: u16, now: i64) -> (Vec<Line<'static>>, Vec<Option<Row>>) {
    build(ws, live, past, None, replies, None, width, now)
}

/// [`layout`]; `recent_note`: said in place of the recent huddles (loading, failed); `me_in`: the conversation
/// whose huddle you are in.
#[allow(clippy::too_many_arguments)]
pub fn build(ws: &Workspace, live: &[String], past: &[PastHuddle], recent_note: Option<Line<'static>>, replies: &HashMap<(String, String), u64>, me_in: Option<&str>, width: u16, now: i64) -> (Vec<Line<'static>>, Vec<Option<Row>>) {
    let p = super::theme::p();
    let dim = Style::default().fg(p.dim);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut owners: Vec<Option<Row>> = Vec::new();
    let mut push = |line: Line<'static>, owner: Option<Row>| {
        lines.push(line);
        owners.push(owner);
    };
    push(header(tr(LIVE_NOW).as_ref(), width), None);
    let mut any = false;
    for c in live {
        let Some(h) = ws.huddles.get(c) else { continue };
        any = true;
        let live_style = Style::default().fg(p.good).add_modifier(Modifier::BOLD);
        let mut first = vec![Span::raw(" "), Span::styled("● ", live_style), Span::styled(tr(LIVE).into_owned(), live_style), Span::raw("  "), Span::styled(super::feed::place(ws, c), Style::default().fg(p.strong).add_modifier(Modifier::BOLD))];
        if h.start > 0 {
            first.push(Span::styled(format!(" · {}", length(now - h.start)), dim));
        }
        if me_in == Some(c.as_str()) {
            first.push(Span::styled(format!(" · {}", tr(YOU_ARE_IN_IT)), Style::default().fg(p.own)));
        }
        push(Line::from(first), Some(Row::Live(c.clone())));
        push(Line::from(vec![Span::raw("   "), Span::styled(people(ws, &h.participants, now), Style::default().fg(p.read))]), Some(Row::Live(c.clone())));
    }
    if !any {
        push(Line::styled(format!(" {}", tr(NONE_LIVE)), dim), None);
    }
    push(Line::raw(""), None);
    push(header(tr(RECENT).as_ref(), width), None);
    // Ended ones only: one still going (also in the live list) shows there.
    let live_rooms: Vec<&str> = ws.huddles.values().map(|h| h.room.as_str()).collect();
    let shown: Vec<(usize, &PastHuddle)> = past.iter().enumerate().filter(|(_, h)| h.ended && !live_rooms.contains(&h.id.as_str())).collect();
    if let Some(note) = recent_note {
        push(note, None);
    } else if shown.is_empty() {
        push(Line::styled(format!(" {}", tr(NONE_RECENT)), dim), None);
    }
    for (n, h) in shown {
        let missed = h.missed.contains(&ws.self_id);
        let mut first = vec![Span::raw(" "), Span::styled(super::feed::place(ws, &h.channel), Style::default().fg(p.strong).add_modifier(Modifier::BOLD))];
        if missed {
            first.push(Span::styled(" · ", dim));
            first.push(Span::styled(tr(MISSED).into_owned(), Style::default().fg(p.danger).add_modifier(Modifier::BOLD)));
        }
        if h.start > 0 {
            first.push(Span::styled(format!(" · {}", ago(now - h.start)), dim));
        }
        if h.end > h.start && h.start > 0 {
            first.push(Span::styled(format!(" · {}", length(h.end - h.start)), dim));
        }
        push(Line::from(first), Some(Row::Past(n)));
        let mut second = vec![Span::raw("   "), Span::styled(people(ws, &h.people, now), Style::default().fg(p.read))];
        let count = h.thread_ts.as_ref().and_then(|t| replies.get(&(h.channel.clone(), t.clone()))).copied().unwrap_or(0);
        if count > 0 {
            let words = if count == 1 { tr(ONE_REPLY).into_owned() } else { tr(REPLIES).replace("{n}", &count.to_string()) };
            second.push(Span::styled(format!(" · {words}"), dim));
        }
        push(Line::from(second), Some(Row::Past(n)));
    }
    (lines, owners)
}

/// A section's header: its name, then a rule to the edge (" Live now ────").
fn header(name: &str, width: u16) -> Line<'static> {
    let p = super::theme::p();
    let label = format!(" {name} ");
    let rule = (width as usize).saturating_sub(unicode_width::UnicodeWidthStr::width(label.as_str()));
    Line::from(vec![Span::styled(label, Style::default().fg(p.accent).add_modifier(Modifier::BOLD)), Span::styled("─".repeat(rule), Style::default().fg(p.dim))])
}

/// Who: each person's name with their status emoji after it ("Bob 🌴, Cy").
fn people(ws: &Workspace, ids: &[String], now: i64) -> String {
    ids.iter()
        .map(|u| match ws.status_of(u, now) {
            Some(s) => format!("{} {}", ws.person_label(u), super::text::emoji(ws, &s.emoji, None)),
            None => ws.person_label(u),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// How long ago, `secs` seconds: "just now", "12 min ago", "3 h ago", "2 d ago".
fn ago(secs: i64) -> String {
    let n = |d: i64| (secs / d).to_string();
    match secs {
        ..60 => tr(JUST_NOW).into_owned(),
        60..3600 => tr(MINUTES_AGO).replace("{n}", &n(60)),
        3600..86400 => tr(HOURS_AGO).replace("{n}", &n(3600)),
        _ => tr(DAYS_AGO).replace("{n}", &n(86400)),
    }
}

/// How long, `secs` seconds: "40 s", "12 min", "1 h 5 min".
fn length(secs: i64) -> String {
    let secs = secs.max(0);
    match secs {
        ..60 => tr(SECONDS).replace("{n}", &secs.to_string()),
        60..3600 => tr(MINUTES).replace("{n}", &(secs / 60).to_string()),
        _ => tr(HOURS).replace("{h}", &(secs / 3600).to_string()).replace("{m}", &(secs % 3600 / 60).to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::state::Huddle;
    use slack_core::types::{Conversation, User};

    fn plain(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.to_string()).collect()
    }

    const NOW: i64 = 1_800_000_000;

    fn ws() -> Workspace {
        let mut ws = Workspace { self_id: "UME".into(), ..Default::default() };
        ws.conversations.insert("C1".into(), Conversation { id: "C1".into(), name: "social".into(), is_channel: true, is_member: true, ..Default::default() });
        ws.conversations.insert("D1".into(), Conversation { id: "D1".into(), is_im: true, user: Some("U2".into()), ..Default::default() });
        for (id, name, emoji) in [("UME", "Me", ""), ("U2", "Bob", ":palm_tree:"), ("U3", "Cy", "")] {
            let mut u = User { id: id.into(), name: name.into(), ..Default::default() };
            u.profile.status_emoji = emoji.into();
            ws.users.insert(id.into(), u);
        }
        ws.huddles.insert("C1".into(), Huddle { room: "R1".into(), participants: vec!["U2".into(), "U3".into()], thread: None, start: NOW - 12 * 60 });
        ws
    }

    fn past(id: &str, channel: &str, start: i64, length: i64, people: &[&str], missed: &[&str], thread: Option<&str>) -> PastHuddle {
        let ids = |l: &[&str]| l.iter().map(|s| s.to_string()).collect();
        PastHuddle { id: id.into(), channel: channel.into(), start, end: start + length, ended: true, thread_ts: thread.map(String::from), people: ids(people), missed: ids(missed), ..Default::default() }
    }

    #[test]
    fn live_first_then_recent() {
        let ws = ws();
        let list = vec![
            past("R2", "C1", NOW - 3 * 3600, 25 * 60, &["U2", "UME"], &[], Some("5.000100")),
            // Still going: shown live only.
            PastHuddle { ended: false, ..past("R1", "C1", NOW - 12 * 60, 0, &["U2"], &[], None) },
            past("R3", "D1", NOW - 2 * 86400, 40, &["U2"], &["UME"], None),
        ];
        let replies = HashMap::from([(("C1".to_string(), "5.000100".to_string()), 3)]);
        let (lines, rows) = layout(&ws, &["C1".to_string()], &list, &replies, 60, NOW);
        let text: Vec<String> = lines.iter().map(plain).collect();
        let at = |what: &str| text.iter().position(|l| l.contains(what)).unwrap_or_else(|| panic!("{what:?} in {text:#?}"));
        assert!(text[0].starts_with(" Live now ─"), "{text:#?}");
        let live = at("● Live");
        assert_eq!(text[live], " ● Live  #social · 12 min", "where, how long");
        assert_eq!(lines[live].spans[1].style.fg, Some(crate::tui::theme::p().good), "marked live");
        assert_eq!(text[live + 1], "   Bob 🌴, Cy", "who is in it, with their status emoji");
        assert_eq!((rows[live].clone(), rows[live + 1].clone()), (Some(Row::Live("C1".into())), Some(Row::Live("C1".into()))));
        let recent = at(" Recent ─");
        assert!(live < recent && rows[recent].is_none());
        assert_eq!(text[recent + 1], " #social · 3 h ago · 25 min");
        assert_eq!(text[recent + 2], "   Bob 🌴, Me (You) · 3 replies");
        assert_eq!(rows[recent + 1], Some(Row::Past(0)));
        let missed = at("Missed huddle");
        assert_eq!(text[missed], " Bob · Missed huddle · 2 d ago · 40 s");
        let span = lines[missed].spans.iter().find(|s| s.content == "Missed huddle").unwrap();
        assert_eq!(span.style.fg, Some(crate::tui::theme::p().danger), "in the alert colour");
        assert_eq!(rows[missed], Some(Row::Past(2)));
        assert!(!text.iter().any(|l| l.contains("12 min ago")), "the one still going is not recent: {text:#?}");
        assert_eq!(text.len(), missed + 2, "nothing after");
        // None live, none recent: said so.
        let (lines, rows) = layout(&Workspace::default(), &[], &[], &HashMap::new(), 60, NOW);
        let text: Vec<String> = lines.iter().map(plain).collect();
        assert!(text[1].contains("No huddles going on") && text.last().unwrap().contains("No recent huddles"), "{text:#?}");
        assert!(rows.iter().all(Option::is_none));
    }

    #[test]
    fn times_read_plainly() {
        assert_eq!((ago(30), ago(125), ago(3 * 3600 + 5), ago(86400 * 3)), ("just now".into(), "2 min ago".into(), "3 h ago".into(), "3 d ago".into()));
        assert_eq!((length(40), length(12 * 60 + 3), length(3600 + 5 * 60)), ("40 s".into(), "12 min".into(), "1 h 5 min".into()));
    }

    /// The view drawn `width`×`height` (focused), you in no huddle.
    fn drawn(h: &mut Huddles, ws: &Workspace, width: u16, height: u16) -> Vec<String> {
        let media = Media::with(ratatui_image::picker::Picker::halfblocks());
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| h.draw(f, f.area(), ws, true, &media, None, NOW)).unwrap();
        let buf = terminal.backend().buffer();
        (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>()).collect()
    }

    #[test]
    fn a_tiny_area_draws_without_panicking() {
        let ws = ws();
        let mut h = Huddles::default();
        let first = h.reload();
        h.add_page(first, false, vec![past("R2", "C1", NOW - 3600, 60, &["U2"], &[], None), past("R3", "C1", NOW - 7200, 60, &["U2"], &[], None)], Some("next".into()));
        h.scroll = 99;
        h.loading = true;
        for height in 0..=3 {
            drawn(&mut h, &ws, 40, height);
        }
        // With no room the end is not on screen: no page is asked for.
        h.loading = false;
        drawn(&mut h, &ws, 40, 2);
        assert_eq!(h.next_page(), None);
    }

    #[test]
    fn a_refresh_keeps_the_list_and_the_reply_counts() {
        let ws = ws();
        let mut h = Huddles::default();
        let first = h.reload();
        let page = vec![past("R2", "C1", NOW - 3600, 60, &["U2"], &[], Some("2.000100")), past("R3", "D1", NOW - 7200, 60, &["U2"], &[], None)];
        h.add_page(first, false, page.clone(), None);
        drawn(&mut h, &ws, 60, 12);
        h.replies.insert(("C1".into(), "2.000100".into()), 4);
        h.requested.insert(("C1".into(), "2.000100".into()));
        h.selected = Some(Row::Past(1));
        // An end concerns the view when it was live there or is in a channel it lists.
        assert!(h.concerns(&["C1".to_string()]) && h.concerns(&["D1".to_string()]));
        assert!(!h.concerns(&["C9".to_string()]));
        let again = h.refresh();
        let screen = drawn(&mut h, &ws, 60, 12);
        assert!(screen.iter().any(|l| l.contains("4 replies")) && !screen.iter().any(|l| l.contains("Loading")), "the list stays meanwhile: {screen:#?}");
        assert!(h.wanted().is_empty(), "the counts known are kept");
        let mut newer = page.clone();
        newer.insert(0, past("R4", "C1", NOW - 60, 30, &["U2"], &[], None));
        h.add_page(again, false, newer, None);
        assert_eq!((h.past.as_ref().map(Vec::len), h.selected.clone()), (Some(3), Some(Row::Past(2))), "replaced, the selection on the same huddle");
    }

    #[test]
    fn the_view_selects_pages_and_hits_rows() {
        let ws = ws();
        let mut h = Huddles::default();
        let first = h.reload();
        let screen = drawn(&mut h, &ws, 60, 12);
        assert!(screen.iter().any(|l| l.contains("Loading…")) && screen.iter().any(|l| l.contains("● Live")), "the live ones show while the recent load: {screen:#?}");
        let page: Vec<PastHuddle> = (0..6).map(|n| past(&format!("R{}", n + 10), "C1", NOW - 3600 * (n + 1), 600, &["U2"], &[], Some(&format!("{n}.000100")))).collect();
        h.add_page(first, false, page.clone(), Some("next".into()));
        let screen = drawn(&mut h, &ws, 60, 12);
        assert_eq!(h.target(), Some(Row::Live("C1".into())), "the first row");
        let y = screen.iter().position(|l| l.contains("1 h ago")).unwrap() as u16;
        assert_eq!(h.row_at(5, y), Some(Row::Past(0)));
        assert_eq!(h.row_at(5, 1), None, "the header");
        h.hover_at(5, y);
        assert_eq!(h.hover, Some(Row::Past(0)));
        let mut on_screen: Vec<&Row> = h.rows.iter().flatten().filter(|r| matches!(r, Row::Past(_))).collect();
        on_screen.dedup();
        assert_eq!(h.wanted().len(), on_screen.len(), "the replies of those on screen");
        assert_eq!(h.next_page(), None, "the end is not on screen yet");
        h.move_by(99);
        drawn(&mut h, &ws, 60, 12);
        assert_eq!(h.target(), Some(Row::Past(5)), "the last");
        assert_eq!(h.next_page().as_deref(), Some("next"), "its end on screen: the next page");
        assert_eq!(h.next_page(), None, "once");
        // A page of an older load is dropped; this one's goes on the end.
        h.add_page(first - 1, true, vec![past("R99", "C1", NOW - 99_000, 60, &["U2"], &[], None)], None);
        h.add_page(first, true, vec![page[0].clone(), past("R20", "C1", NOW - 90_000, 60, &["U2"], &[], None)], None);
        assert_eq!(h.past.as_ref().map(|p| p.iter().map(|p| p.id.as_str()).collect::<Vec<_>>()).unwrap().len(), 7, "the one listed already left out");
        // Narrow (an 80-column terminal's main pane): one line per row, cut not wrapped.
        let screen = drawn(&mut h, &ws, 24, 12);
        let starts: Vec<usize> = (1..11).filter(|&y| screen[y].starts_with("│ #social")).collect();
        assert!(!starts.is_empty() && starts.iter().all(|&y| y == 10 || screen[y + 1].starts_with("│   Bob")), "{screen:#?}");
        assert!(screen.iter().all(|l| l.ends_with('┐') || l.ends_with('│') || l.ends_with('┘') || l.ends_with('┃')), "cut at the frame, not wrapped: {screen:#?}");
        // A reload keeps the recent one selected, wherever it is in the new list.
        h.selected = Some(Row::Past(2));
        let again = h.reload();
        drawn(&mut h, &ws, 60, 12);
        let mut newer = page.clone();
        newer.insert(0, past("R30", "C1", NOW - 60, 30, &["U2"], &[], None));
        h.add_page(again, false, newer, None);
        assert_eq!(h.selected, Some(Row::Past(3)), "the same huddle, one further down");
        // A reload asks for the reply counts again.
        for k in h.wanted() {
            h.requested.insert(k);
        }
        assert!(h.wanted().is_empty());
        // A failure with nothing listed says so.
        let second = h.reload();
        h.add_page(second, false, page.clone(), None);
        drawn(&mut h, &ws, 60, 12);
        assert!(!h.wanted().is_empty(), "asked for again");
        let second = h.reload();
        h.fail(second, "boom".into());
        let screen = drawn(&mut h, &ws, 60, 12);
        assert!(screen.iter().any(|l| l.contains("could not be loaded: boom")));
    }

    #[test]
    fn the_selection_follows_any_scroll_onto_the_screen() {
        let ws = ws();
        let mut h = Huddles::default();
        let first = h.reload();
        h.add_page(first, false, (1..=30).map(|n| past(&format!("R{n}"), "C1", NOW - 3600 * n as i64, 60, &["U2"], &[], None)).collect(), None);
        drawn(&mut h, &ws, 60, 12);
        let on_screen = |h: &Huddles| -> Vec<Row> {
            let mut v: Vec<Row> = h.rows.iter().flatten().cloned().collect();
            v.dedup();
            v
        };
        // Scrolled down (a scroll of yours): the top row on screen; ↓ moves from there.
        (h.scroll, h.scrolled) = (30, true);
        drawn(&mut h, &ws, 60, 12);
        // (The top or bottom one on screen may be cut by the edge: the next one in is shown whole.)
        let shown = on_screen(&h);
        let sel = h.selected.clone().unwrap();
        assert!(shown[..2].contains(&sel), "the top one: {sel:?} of {shown:?}");
        h.move_by(1);
        drawn(&mut h, &ws, 60, 12);
        assert_eq!(h.selected.as_ref(), shown.iter().skip_while(|r| **r != sel).nth(1), "the next one");
        // Scrolled up: the bottom one.
        (h.scroll, h.scrolled) = (0, true);
        drawn(&mut h, &ws, 60, 12);
        let shown = on_screen(&h);
        assert!(shown[shown.len() - 2..].contains(h.selected.as_ref().unwrap()), "the bottom one");
    }

    #[test]
    fn a_row_taller_than_the_view_scrolls_line_by_line() {
        use crate::tui::threads::{Act, OnScreen};
        // Two lines each in a view one line high; no huddle live (its row would take the selection at the top).
        let mut ws = ws();
        ws.huddles.clear();
        let media = Media::with(ratatui_image::picker::Picker::halfblocks());
        let mut h = Huddles::default();
        let first = h.reload();
        h.add_page(first, false, (1..=6).map(|n| past(&format!("R{n}"), "C1", NOW - 3600 * n as i64, 60, &["U2"], &[], None)).collect(), None);
        let tall = Row::Past(1);
        let mut width = 60;
        let mut act = |h: &mut Huddles, act: Act| -> OnScreen {
            match act {
                Act::Draw => {}
                Act::Wheel(lines) => (h.scroll, h.scrolled) = (h.scroll.saturating_add_signed(lines), true),
                Act::Key(by) => h.move_by(by),
                Act::Rewrap => width = 50,
                Act::Arrive => _ = ws.huddles.insert("D1".into(), Huddle { room: "R9".into(), participants: vec!["U2".into()], thread: None, start: NOW - 60 }),
            }
            let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 3)).unwrap();
            terminal.draw(|f| h.draw(f, f.area(), &ws, true, &media, None, NOW)).unwrap();
            let text = crate::tui::threads::rows_text(terminal.backend().buffer(), h.list);
            // (Above the first row only headers show: the selection stays where it is.)
            let selection_shown = h.rows.contains(&h.selected) || h.rows.iter().all(Option::is_none);
            let lines = build(&ws, &ws.live_list(None), h.past.as_deref().unwrap_or_default(), None, &h.replies, None, width, NOW).0.len();
            let tall_shown = h.rows.contains(&Some(tall.clone()));
            OnScreen { text, top: Some(h.scroll), at_top: h.scroll == 0, at_end: h.scroll + 1 >= lines, tall_shown, tall_selected: h.selected.as_ref() == Some(&tall), selection_shown }
        };
        act(&mut h, Act::Draw);
        h.selected = Some(tall.clone());
        act(&mut h, Act::Key(0));
        crate::tui::threads::wheel_through_tall(&mut h, &mut act);
        // ↑/↓ onto it; its second line on screen: a re-wrap (a resize), a huddle starting above.
        h.selected = Some(tall.clone());
        crate::tui::threads::keys_and_changes_on_tall(&mut h, &mut act, " #social · 2 h ago", 1, false);
    }

    #[test]
    fn a_rewrap_keeps_the_selection_on_screen() {
        let ws = ws();
        let mut h = Huddles::default();
        let first = h.reload();
        h.add_page(first, false, (1..=30).map(|n| past(&format!("R{n}"), "C1", NOW - 3600 * n as i64, 60, &["U2"], &[], None)).collect(), None);
        drawn(&mut h, &ws, 60, 30);
        while h.rows.iter().position(|r| r.as_ref() == h.selected.as_ref()).is_none_or(|row| row < 20) {
            h.move_by(1);
            drawn(&mut h, &ws, 60, 30);
        }
        let sel = h.selected.clone();
        for (width, height) in [(60, 12), (30, 12)] {
            drawn(&mut h, &ws, width, height);
            assert_eq!(h.selected, sel, "{width}×{height}: the same row");
            assert!(h.rows.contains(&sel), "{width}×{height}: on screen");
        }
    }

    #[test]
    fn growth_above_the_selection_never_moves_it() {
        // Not a scroll: a huddle starting (the live list above grows).
        let mut ws = ws();
        let mut h = Huddles::default();
        let first = h.reload();
        h.add_page(first, false, (1..=30).map(|n| past(&format!("R{n}"), "C1", NOW - 3600 * n as i64, 60, &["U2"], &[], None)).collect(), None);
        drawn(&mut h, &ws, 60, 30);
        let row_of = |h: &Huddles| h.rows.iter().position(|r| r.as_ref() == h.selected.as_ref());
        while row_of(&h).is_none_or(|row| row < 20) {
            h.move_by(1);
            drawn(&mut h, &ws, 60, 30);
        }
        let (sel, at) = (h.selected.clone(), row_of(&h));
        ws.huddles.insert("D1".into(), Huddle { room: "R99".into(), participants: vec!["U2".into()], thread: None, start: NOW - 60 });
        drawn(&mut h, &ws, 60, 30);
        assert_eq!((h.selected.clone(), row_of(&h)), (sel, at), "the same row selected, where it was");
    }
}

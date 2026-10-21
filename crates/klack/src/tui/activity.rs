//! Activity (Ctrl+A): mentions, reactions to your messages and thread updates from
//! `activity.feed`, newest first. Message texts are fetched per item as they are shown.

use std::collections::HashMap;


use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use serde_json::Value;
use slack_core::format;
use slack_core::types::Message;

use super::state::Workspace;
use super::text;

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// `at_user`, `at_channel`, … : a message mentioning you (or everyone).
    Mention(String),
    Reaction { name: String, user: String },
    /// New replies in a thread you follow.
    Thread { unread: u64, latest: String },
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub key: String,
    pub kind: Kind,
    pub channel: String,
    /// The message the item is about (for threads: the parent).
    pub ts: String,
    pub author: Option<String>,
    pub unread: bool,
    pub when: f64,
}

/// Mark as read the items opened in this run since they came (`seen`: see
/// `threads::Seen`; a thread's own key counts for its replies).
pub fn apply_seen(items: &mut [Item], seen: &super::threads::Seen) {
    for it in items.iter_mut().filter(|it| it.unread) {
        let opened = |key: &str, t: f64| seen.get(key).is_some_and(|at| *at >= t);
        it.unread = match &it.kind {
            Kind::Thread { latest, .. } => !opened(&super::state::thread_key(&it.channel, &it.ts), latest.parse().unwrap_or(f64::MAX)) && !opened(&it.key, it.when),
            _ => !opened(&it.key, it.when),
        };
    }
}

impl Item {
    fn from_json(v: &Value) -> Option<Item> {
        let it = &v["item"];
        let ty = it["type"].as_str()?;
        let thread = &it["bundle_info"]["payload"]["thread_entry"];
        let (kind, channel, ts) = match ty {
            "thread_v2" => (
                Kind::Thread { unread: thread["unread_msg_count"].as_u64().unwrap_or(0), latest: thread["latest_ts"].as_str().unwrap_or_default().to_string() },
                thread["channel_id"].as_str()?,
                thread["thread_ts"].as_str()?,
            ),
            "message_reaction" => (
                Kind::Reaction { name: it["reaction"]["name"].as_str().unwrap_or_default().to_string(), user: it["reaction"]["user"].as_str().unwrap_or_default().to_string() },
                it["message"]["channel"].as_str()?,
                it["message"]["ts"].as_str()?,
            ),
            t if t.starts_with("at_") || t == "keyword" => (Kind::Mention(t.to_string()), it["message"]["channel"].as_str()?, it["message"]["ts"].as_str()?),
            t => (Kind::Other(t.to_string()), it["message"]["channel"].as_str().unwrap_or_default(), it["message"]["ts"].as_str().unwrap_or_default()),
        };
        Some(Item {
            key: v["key"].as_str().unwrap_or_default().to_string(),
            kind,
            channel: channel.to_string(),
            ts: ts.to_string(),
            author: it["message"]["author_user_id"].as_str().map(String::from),
            unread: v["is_unread"].as_bool().unwrap_or(false),
            when: v["feed_ts"].as_str().and_then(|t| t.parse().ok()).unwrap_or(0.0),
        })
    }
}

pub fn parse(v: &Value) -> Vec<Item> {
    v["items"].as_array().into_iter().flatten().filter_map(Item::from_json).collect()
}

#[derive(Default)]
pub struct Activity {
    pub items: Option<Vec<Item>>,
    pub selected: usize,
    /// Messages fetched for items, by (channel, ts); None when it is gone.
    pub messages: HashMap<(String, String), Option<Message>>,
    /// Messages asked for (loaded or not).
    pub requested: std::collections::HashSet<(String, String)>,
    /// Items per page on the last draw.
    pub height: usize,
    /// Last drawn area, for clicks.
    pub area: Rect,
    /// The list's rows at the last draw, the first item shown and how many are.
    pub list: Rect,
    pub first: usize,
    pub shown: usize,
    /// The row the pointer is over (not the selection), at the last `Moved`.
    pub hover: Option<usize>,
}

impl Activity {
    pub fn move_by(&mut self, delta: isize) {
        let len = self.items.as_ref().map_or(0, Vec::len);
        self.selected = if len == 0 { 0 } else { (self.selected as isize + delta).clamp(0, len as isize - 1) as usize };
    }

    /// The item drawn at a terminal position (two rows and a gap each; the gap is no item).
    pub fn row_at(&self, column: u16, row: u16) -> Option<usize> {
        let r = row.checked_sub(self.list.y)? as usize;
        (self.list.contains((column, row).into()) && r % 3 < 2).then(|| self.first + r / 3).filter(|n| *n < self.first + self.shown)
    }

    /// The pointer moved to a position: hover the item there (none off the list).
    pub fn hover_at(&mut self, column: u16, row: u16) {
        self.hover = self.row_at(column, row);
    }

    pub fn selected_item(&self) -> Option<&Item> {
        self.items.as_ref()?.get(self.selected)
    }

    /// Where Enter on the selected item goes: (channel, message ts, thread parent).
    pub fn target(&self) -> Option<(String, String, Option<String>)> {
        let it = self.selected_item()?;
        match &it.kind {
            Kind::Thread { latest, .. } => Some((it.channel.clone(), latest.clone(), Some(it.ts.clone()))),
            _ => {
                let parent = self.messages.get(&(it.channel.clone(), it.ts.clone())).and_then(|m| m.as_ref()).and_then(|m| m.thread_ts.clone()).filter(|t| *t != it.ts);
                Some((it.channel.clone(), it.ts.clone(), parent))
            }
        }
    }

    /// Items around the selection whose message has not been requested yet.
    pub fn wanted(&self) -> Vec<(String, String)> {
        let Some(items) = &self.items else { return vec![] };
        let start = self.selected.saturating_sub(self.height);
        let mut out: Vec<(String, String)> = Vec::new();
        for it in items.iter().skip(start).take(self.height.max(1) * 3) {
            let k = (it.channel.clone(), it.ts.clone());
            if !self.requested.contains(&k) && !out.contains(&k) && !it.ts.is_empty() {
                out.push(k);
            }
        }
        out
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, ws: &Workspace) {
        let [popup] = Layout::horizontal([Constraint::Length(90.min(area.width))]).flex(Flex::Center).areas(area);
        let [popup] = Layout::vertical([Constraint::Length(30.min(area.height))]).flex(Flex::Center).areas(popup);
        self.area = popup;
        super::media::clear_modal(f, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(super::theme::p().accent))
            .title(format!(" {} ", super::i18n::tr("Activity")));
        let inner = block.inner(popup);
        f.render_widget(block, popup);
        self.height = (inner.height / 3) as usize;
        let mut marked = None;
        let lines = match &self.items {
            None => vec![Line::styled("Loading…", Style::default().fg(super::theme::p().dim))],
            Some(items) if items.is_empty() => vec![Line::styled("Nothing yet: mentions, reactions to your messages and thread replies show up here.", Style::default().fg(super::theme::p().dim))],
            Some(items) => {
                let start = self.selected.saturating_sub(self.height.saturating_sub(1));
                let width = inner.width as usize;
                let mut lines = Vec::new();
                for (n, it) in items.iter().enumerate().skip(start).take(self.height) {
                    let msg = self.messages.get(&(it.channel.clone(), it.ts.clone()));
                    if n == self.selected {
                        marked = Some(Rect { y: inner.y + lines.len() as u16, height: 2, ..inner });
                    }
                    let mut row = vec![headline(ws, it, msg.and_then(Option::as_ref)), body(ws, msg, width), Line::raw("")];
                    for l in row.iter_mut().take(2) {
                        l.spans.insert(0, super::sidebar::unread_mark(it.unread));
                        if n == self.selected {
                            *l = std::mem::take(l).style(Style::default().bg(super::theme::p().selection));
                        } else if self.hover == Some(n) {
                            *l = std::mem::take(l).style(super::theme::hover_style());
                        }
                    }
                    lines.extend(row);
                }
                lines
            }
        };
        // The selected item's rows are filled edge to edge first; the text's own backgrounds draw over them.
        if let Some(rect) = marked {
            f.buffer_mut().set_style(rect.intersection(inner), Style::default().bg(super::theme::p().selection));
        }
        f.render_widget(Paragraph::new(lines), inner);
        self.list = inner;
        (self.first, self.shown) = (0, 0);
        if let Some(items) = &self.items {
            let start = self.selected.saturating_sub(self.height.saturating_sub(1));
            (self.first, self.shown) = (start, items.len().saturating_sub(start).min(self.height));
            super::scroll::indicator_beside(f, popup, inner, start, self.height, items.len(), Style::default().fg(super::theme::p().accent));
        }
    }
}

fn when(ws: &Workspace, t: f64) -> String {
    let Some(d) = super::clock::at(t as i64) else { return String::new() };
    let time = ws.settings.time_format();
    if d.date_naive() == super::clock::now().date_naive() { d.format(time).to_string() } else { d.format(&format!("%b %-d {time}")).to_string() }
}

/// "Bob mentioned you in #general · 14:02" and the like.
pub fn headline(ws: &Workspace, it: &Item, msg: Option<&Message>) -> Line<'static> {
    let place = match ws.conversations.get(&it.channel) {
        Some(c) if c.is_im => "a direct message".to_string(),
        Some(c) if c.is_mpim => "a group message".to_string(),
        Some(c) => format!("#{}", c.name),
        None => it.channel.clone(),
    };
    let author = it.author.clone().or_else(|| msg.and_then(|m| m.user.clone())).map(|u| ws.user_name(&u)).unwrap_or_else(|| "someone".into());
    let (icon, what) = match &it.kind {
        Kind::Mention(t) if t == "at_user" => ("@", format!("{author} mentioned you in {place}")),
        Kind::Mention(t) if t == "keyword" => ("🔑", format!("{author} used one of your keywords in {place}")),
        Kind::Mention(t) => ("📣", format!("{author} wrote to {} in {place}", t.trim_start_matches("at_"))),
        Kind::Reaction { name, user } => ("", format!("{} {} reacted to your message in {place}", text::emoji(ws, name, None), ws.user_name(user))),
        Kind::Thread { unread, .. } if *unread > 0 => ("💬", format!("{unread} new {} in a thread in {place}", if *unread == 1 { "reply" } else { "replies" })),
        Kind::Thread { .. } => ("💬", format!("Thread in {place}")),
        Kind::Other(t) => ("•", format!("{t} in {place}")),
    };
    let style = if it.unread { Style::default().add_modifier(Modifier::BOLD).fg(super::theme::p().strong) } else { Style::default().fg(super::theme::p().read) };
    let mut spans = Vec::new();
    if !icon.is_empty() {
        spans.push(Span::raw(format!("{icon} ")));
    }
    spans.push(Span::styled(what, style));
    spans.push(Span::styled(format!("  {}", when(ws, it.when)), Style::default().fg(super::theme::p().dim)));
    Line::from(spans)
}

fn body(ws: &Workspace, msg: Option<&Option<Message>>, width: usize) -> Line<'static> {
    match msg {
        None => Line::styled("  …", Style::default().fg(super::theme::p().dim)),
        Some(None) => Line::styled("  (message not available)", Style::default().fg(super::theme::p().dim)),
        Some(Some(m)) => {
            let plain = format::plain_text(&format::message_blocks(&m.text, &m.blocks), &|i| text::inline_plain(ws, i)).replace('\n', " ");
            let plain = if plain.is_empty() && !m.files.is_empty() { format!("[{}]", m.files[0].name) } else { plain };
            let cut: String = plain.chars().take(width.saturating_sub(5)).collect();
            Line::styled(format!("  {cut}"), Style::default().fg(super::theme::p().dim))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_opened_here_are_seen() {
        let item = |key: &str, kind: Kind, when: f64| Item { key: key.into(), kind, channel: "C1".into(), ts: "1.0".into(), author: None, unread: true, when };
        let mut items = vec![
            item("m1", Kind::Mention("at_user".into()), 5.0),
            item("m2", Kind::Mention("at_user".into()), 9.0),
            item("t1", Kind::Thread { unread: 2, latest: "6.0".into() }, 6.0),
            item("t2", Kind::Thread { unread: 1, latest: "12.0".into() }, 12.0),
        ];
        items[3].ts = "2.0".into();
        let seen = super::super::threads::Seen::from([("m1".to_string(), 7.0), ("m2".to_string(), 7.0), ("C1/1.0".to_string(), 7.0), ("C1/2.0".to_string(), 7.0)]);
        apply_seen(&mut items, &seen);
        assert_eq!(items.iter().map(|i| i.unread).collect::<Vec<_>>(), [false, true, false, true], "opened after it came: seen; newer: still unread; a thread opened anywhere counts");
    }

    #[test]
    fn parses_real_items() {
        // Shapes captured from activity.feed in the testbed.
        let v = serde_json::json!({"items": [
            {"feed_ts":"1792326270.026269","is_unread":true,"item":{"bundle_info":{"payload":{"thread_entry":{"channel_id":"D1","latest_ts":"1792326270.026269","min_unread_ts":"1792326270.026269","thread_ts":"1792325783.787339","unread_msg_count":1}}},"type":"thread_v2"},"key":"thread_v2-D1-1792325783.787339"},
            {"feed_ts":"1792326101.733899","is_unread":false,"item":{"message":{"author_user_id":"U2","channel":"C1","is_broadcast":false,"ts":"1792326101.733899"},"type":"at_user"},"key":"at_user-C1-1792326101.733899"},
            {"feed_ts":"1792323624.000000","is_unread":false,"item":{"message":{"channel":"D1","ts":"1792323487.950999"},"reaction":{"name":"heart","user":"U2"},"type":"message_reaction"},"key":"k3"}
        ]});
        let items = parse(&v);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].kind, Kind::Thread { unread: 1, latest: "1792326270.026269".into() });
        assert_eq!((items[0].channel.as_str(), items[0].ts.as_str(), items[0].unread), ("D1", "1792325783.787339", true));
        assert_eq!((items[1].kind.clone(), items[1].author.as_deref()), (Kind::Mention("at_user".into()), Some("U2")));
        assert_eq!(items[2].kind, Kind::Reaction { name: "heart".into(), user: "U2".into() });
    }

    #[test]
    fn wants_each_message_once() {
        let mut a = Activity { height: 5, ..Default::default() };
        let item = |ts: &str| Item { key: ts.into(), kind: Kind::Other("x".into()), channel: "C1".into(), ts: ts.into(), author: None, unread: false, when: 0.0 };
        a.items = Some(vec![item("1"), item("2"), item("1")]);
        assert_eq!(a.wanted().len(), 2);
        a.requested.insert(("C1".into(), "1".into()));
        assert_eq!(a.wanted(), vec![("C1".to_string(), "2".to_string())]);
    }

    #[test]
    fn a_click_maps_to_the_item_drawn_there() {
        // Three rows per item (the last is a gap); item 5 is the first shown, two are.
        let a = Activity { list: Rect::new(5, 10, 88, 28), first: 5, shown: 2, ..Default::default() };
        assert_eq!((a.row_at(6, 10), a.row_at(6, 11)), (Some(5), Some(5)));
        assert_eq!(a.row_at(6, 12), None, "the gap");
        assert_eq!(a.row_at(6, 13), Some(6));
        assert_eq!(a.row_at(6, 16), None, "below the last item");
        assert_eq!(a.row_at(6, 9), None, "the border / title");
    }

    #[test]
    fn the_selection_fills_the_whole_row() {
        let item = |ts: &str| Item { key: ts.into(), kind: Kind::Other("x".into()), channel: "C1".into(), ts: ts.into(), author: None, unread: false, when: 0.0 };
        let mut a = Activity { items: Some(vec![item("1.0"), item("2.0")]), ..Default::default() };
        let ws = Workspace::default();
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 30)).unwrap();
        terminal.draw(|f| a.draw(f, f.area(), &ws)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let p = super::super::theme::p();
        let (x0, x1) = (a.list.x, a.list.right() - 1);
        for y in a.list.y..a.list.y + 2 {
            assert_eq!((buf[(x0, y)].bg, buf[(x1, y)].bg), (p.selection, p.selection), "row {y}: edge to edge");
        }
        for y in a.list.y + 2..a.list.y + 6 {
            assert!((x0..=x1).all(|x| buf[(x, y)].bg != p.selection), "row {y}: not the selected item");
        }
        assert_ne!(buf[(x1 + 1, a.list.y)].bg, p.selection, "not the border");
    }

    #[test]
    fn hover_follows_the_pointer_and_leaves_the_selection() {
        let mut a = Activity { list: Rect::new(5, 10, 80, 20), first: 5, shown: 2, selected: 5, ..Default::default() };
        a.hover_at(6, 13);
        assert_eq!((a.hover, a.selected), (Some(6), 5));
        a.hover_at(6, 12);
        assert_eq!(a.hover, None, "the gap");
    }
}

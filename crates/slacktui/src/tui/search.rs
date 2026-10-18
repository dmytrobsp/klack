//! Message search (Ctrl+F) via `search.messages`.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use serde_json::Value;
use slack_core::format;

use super::state::Workspace;
use super::text;

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub channel: String,
    pub ts: String,
    /// Set for thread replies (taken from the permalink).
    pub thread_ts: Option<String>,
    pub user: Option<String>,
    pub text: String,
}

/// Matches of a `search.messages` response, newest first as Slack returns them.
pub fn parse_hits(v: &Value) -> Vec<Hit> {
    v["messages"]["matches"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let thread_ts = m["permalink"].as_str().and_then(|p| p.split_once("thread_ts=")).map(|(_, rest)| rest.split('&').next().unwrap_or(rest).to_string());
            Some(Hit {
                channel: m["channel"]["id"].as_str()?.to_string(),
                ts: m["ts"].as_str()?.to_string(),
                thread_ts: thread_ts.filter(|t| Some(t.as_str()) != m["ts"].as_str()),
                user: m["user"].as_str().map(String::from),
                text: m["text"].as_str().unwrap_or_default().to_string(),
            })
        })
        .collect()
}

/// Matches per `search.messages` page.
pub const PAGE_SIZE: u32 = 50;

/// The next page is loaded when the selection comes this close to the last hit.
const PREFETCH: usize = 5;

#[derive(Debug, Default)]
pub struct Search {
    pub query: String,
    /// The query the current results belong to.
    pub searched: String,
    pub hits: Vec<Hit>,
    pub total: u64,
    pub selected: usize,
    pub busy: bool,
    /// Pages loaded and pages there are.
    pub page: u32,
    pub pages: u32,
    /// The next page is on its way.
    pub loading_more: bool,
}

impl Search {
    pub fn move_by(&mut self, delta: isize) {
        self.selected = if self.hits.is_empty() { 0 } else { (self.selected as isize + delta).clamp(0, self.hits.len() as isize - 1) as usize };
    }

    /// The page to load next, when the selection is near the end of the hits and there is
    /// more (marks it as loading).
    pub fn next_page(&mut self) -> Option<u32> {
        if self.busy || self.loading_more || self.page >= self.pages || self.selected + PREFETCH < self.hits.len() {
            return None;
        }
        self.loading_more = true;
        Some(self.page + 1)
    }

    /// Take in page `page` of the results for `query`: page 1 replaces them, the next page is
    /// appended (without hits already listed); anything else (an old query, a page out of
    /// order) is dropped.
    pub fn add_page(&mut self, query: String, page: u32, v: &Value) {
        let hits = parse_hits(v);
        let total = v["messages"]["total"].as_u64().unwrap_or(hits.len() as u64);
        let pages = v["messages"]["paging"]["pages"].as_u64().unwrap_or(1) as u32;
        if page <= 1 {
            (self.hits, self.searched, self.selected) = (hits, query, 0);
        } else if query == self.searched && page == self.page + 1 {
            self.loading_more = false;
            let new: Vec<Hit> = hits.into_iter().filter(|h| !self.hits.iter().any(|o| o.channel == h.channel && o.ts == h.ts)).collect();
            self.hits.extend(new);
        } else {
            return;
        }
        (self.page, self.pages, self.total) = (page, pages, total);
    }

    pub fn draw(&self, f: &mut Frame, area: Rect, ws: &Workspace) {
        let [popup] = Layout::horizontal([Constraint::Length(90.min(area.width))]).flex(Flex::Center).areas(area);
        let [popup] = Layout::vertical([Constraint::Length(24.min(area.height))]).flex(Flex::Center).areas(popup);
        f.render_widget(Clear, popup);
        let status = if self.busy {
            " searching… ".to_string()
        } else if self.searched.is_empty() {
            String::new()
        } else {
            format!(" {} of {}{} ", self.hits.len(), self.total, if self.loading_more { " · loading more…" } else { "" })
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(super::theme::p().accent))
            .title(format!(" {} ", super::i18n::tr("Search messages")))
            .title_bottom(Line::styled(status, Style::default().fg(super::theme::p().dim)).right_aligned());
        let inner = block.inner(popup);
        f.render_widget(block, popup);
        let [input, list] = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).areas(inner);
        f.render_widget(Paragraph::new(Line::from(vec![Span::styled("🔍 ", Style::default().fg(super::theme::p().dim)), Span::raw(self.query.clone())])), input);
        f.set_cursor_position((input.x + 3 + self.query.chars().count() as u16, input.y));
        let width = list.width as usize;
        // Two rows per hit: where/who/when, then the text.
        let per_page = (list.height as usize / 2).max(1);
        let start = self.selected.saturating_sub(per_page - 1);
        let mut lines = Vec::new();
        for (n, hit) in self.hits.iter().enumerate().skip(start).take(per_page) {
            let when = hit.ts.parse::<f64>().ok().and_then(|t| super::clock::at(t as i64)).map(|d| d.format(&format!("%b %e {}", ws.settings.time_format())).to_string()).unwrap_or_default();
            let who = hit.user.as_deref().map(|u| ws.user_name(u)).unwrap_or_else(|| "bot".into());
            let place = ws.channel_label(&hit.channel);
            let place = if ws.conversations.get(&hit.channel).is_some_and(|c| c.is_dm()) { place } else { format!("#{place}") };
            let thread = if hit.thread_ts.is_some() { " · in thread" } else { "" };
            let body = format::plain_text(&format::message_blocks(&hit.text, &[]), &|i| text::inline_plain(ws, i)).replace('\n', " ");
            let body: String = body.chars().take(width.saturating_sub(2)).collect();
            let style = if n == self.selected { Style::default().bg(super::theme::p().selection) } else { Style::default() };
            lines.push(Line::from(vec![Span::styled(format!(" {who}"), Style::default().fg(text::user_color(hit.user.as_deref().unwrap_or("")))), Span::styled(format!(" · {place} · {when}{thread}"), Style::default().fg(super::theme::p().dim))]).style(style));
            lines.push(Line::raw(format!("   {body}")).style(style));
        }
        if lines.is_empty() && !self.searched.is_empty() && !self.busy {
            lines.push(Line::styled(" no messages found", Style::default().fg(super::theme::p().dim)));
        }
        f.render_widget(Paragraph::new(lines), list);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_with_thread_from_permalink() {
        let v = serde_json::json!({"messages":{"total":2,"matches":[
            {"channel":{"id":"D1","name":"U2"},"ts":"11.5","user":"U2","text":"reply","permalink":"https://x.slack.com/archives/D1/p1150?thread_ts=10.0&cid=D1"},
            {"channel":{"id":"C1","name":"general"},"ts":"9.0","user":"U3","text":"top","permalink":"https://x.slack.com/archives/C1/p900"},
            {"channel":{},"ts":"1.0"}
        ]}});
        let hits = parse_hits(&v);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].thread_ts.as_deref(), Some("10.0"));
        assert_eq!(hits[1].thread_ts, None);
        assert_eq!(hits[1].channel, "C1");
    }

    /// A page as `search.messages` returns it (fields trimmed; paging values from a real
    /// query with 130 matches in #history-paging).
    fn page(page: u32, texts: &[(&str, &str)]) -> Value {
        let matches: Vec<Value> = texts.iter().map(|(ts, text)| serde_json::json!({"channel":{"id":"C0CA5R2EX1Q","name":"history-paging"},"ts":ts,"user":"U0CA4NBE06N","text":text,"permalink":format!("https://rslack-testbed.slack.com/archives/C0CA5R2EX1Q/p{}", ts.replace('.', ""))})).collect();
        serde_json::json!({"ok":true,"messages":{"total":130,"matches":matches,"paging":{"count":50,"page":page,"pages":3,"total":130},"pagination":{"first":1,"last":50,"page":page,"page_count":3,"per_page":50,"total_count":130}}})
    }

    #[test]
    fn paging() {
        let mut s = Search::default();
        s.add_page("paging test message".into(), 1, &page(1, &[("1792344759.290079", "paging test message 128"), ("1792344700.000100", "paging test message 59")]));
        assert_eq!((s.hits.len(), s.total, s.page, s.pages), (2, 130, 1, 3));
        assert_eq!(s.next_page(), Some(2), "two hits: the selection is near the end");
        assert_eq!(s.next_page(), None, "already loading");
        // A page for an older query is dropped and keeps the request open.
        s.add_page("other".into(), 2, &page(2, &[("1.0", "x")]));
        assert_eq!(s.hits.len(), 2);
        // Page 2 is appended without a hit already listed.
        s.add_page("paging test message".into(), 2, &page(2, &[("1792344700.000100", "paging test message 59"), ("1792344600.000200", "paging test message 3")]));
        assert_eq!(s.hits.iter().map(|h| h.text.as_str()).collect::<Vec<_>>(), ["paging test message 128", "paging test message 59", "paging test message 3"]);
        assert!(!s.loading_more);
        assert_eq!(s.page, 2);
        // Far from the end nothing more is loaded; after the last page never.
        s.hits.extend((0..20).map(|i| Hit { channel: "C".into(), ts: format!("{i}.0"), thread_ts: None, user: None, text: String::new() }));
        assert_eq!(s.next_page(), None);
        s.selected = s.hits.len() - 1;
        assert_eq!(s.next_page(), Some(3));
        s.add_page("paging test message".into(), 3, &page(3, &[]));
        assert_eq!(s.next_page(), None, "no page 4");
        // A new search starts over.
        s.add_page("new".into(), 1, &page(1, &[("5.0", "n")]));
        assert_eq!((s.hits.len(), s.selected, s.searched.as_str()), (1, 0, "new"));
    }
}

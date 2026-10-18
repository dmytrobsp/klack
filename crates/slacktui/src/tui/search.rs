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

#[derive(Debug, Default)]
pub struct Search {
    pub query: String,
    /// The query the current results belong to.
    pub searched: String,
    pub hits: Vec<Hit>,
    pub total: u64,
    pub selected: usize,
    pub busy: bool,
}

impl Search {
    pub fn move_by(&mut self, delta: isize) {
        self.selected = if self.hits.is_empty() { 0 } else { (self.selected as isize + delta).clamp(0, self.hits.len() as isize - 1) as usize };
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
            format!(" {} of {} ", self.hits.len(), self.total)
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
}

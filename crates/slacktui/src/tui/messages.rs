//! Message timeline rendering and scrolling.

use chrono::{Local, TimeZone};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use slack_core::format;
use slack_core::types::Message;

use super::state::{Timeline, Workspace};
use super::text::{self, DIM};

/// Messages from the same author within this many seconds share one header.
const GROUP_WINDOW: f64 = 300.0;

#[derive(Debug, Default)]
pub struct MessagePane {
    /// Lines scrolled up from the bottom (0 = newest visible).
    pub scroll: usize,
    pub area: Rect,
    /// Total rendered lines on the last draw (to detect reaching the top).
    pub total_lines: usize,
    /// Selected message ts (message focus).
    pub selected: Option<String>,
    /// Message ts owning each visible row of the last draw (for clicks).
    rows: Vec<Option<String>>,
    /// Thread mode: show the parent and every reply instead of top-level messages.
    pub thread: bool,
}

impl MessagePane {
    /// A pane in thread mode.
    pub fn thread() -> Self {
        MessagePane { thread: true, ..Default::default() }
    }

    pub fn scroll_up(&mut self, n: usize) {
        self.scroll = self.scroll.saturating_add(n);
    }

    pub fn scroll_down(&mut self, n: usize) {
        self.scroll = self.scroll.saturating_sub(n);
    }

    /// True once the top of the loaded history is on screen.
    pub fn at_top(&self) -> bool {
        let height = self.area.height.saturating_sub(2) as usize;
        self.total_lines <= height || self.scroll + height >= self.total_lines
    }

    /// Move the selection by `delta` messages among the shown ones (`order` = ts list).
    pub fn move_selection(&mut self, order: &[String], delta: isize) {
        if order.is_empty() {
            self.selected = None;
            return;
        }
        let pos = self.selected.as_ref().and_then(|s| order.iter().position(|t| t == s)).map(|p| p as isize).unwrap_or(order.len() as isize);
        let next = (pos + delta).clamp(0, order.len() as isize - 1) as usize;
        self.selected = Some(order[next].clone());
    }

    /// Select the message drawn at terminal row `row`; true if one was hit.
    pub fn click(&mut self, column: u16, row: u16) -> bool {
        let inner = self.area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
        if !inner.contains((column, row).into()) {
            return false;
        }
        match self.rows.get((row - inner.y) as usize).cloned().flatten() {
            Some(ts) => {
                self.selected = Some(ts);
                true
            }
            None => false,
        }
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, ws: &Workspace, title: &str, timeline: Option<&Timeline>, focused: bool) {
        self.area = area;
        let title = if title.is_empty() { String::new() } else { format!(" {title} ") };
        let border = if focused { Style::default().fg(Color::Rgb(0x61, 0xaf, 0xef)) } else { Style::default().fg(DIM) };
        let block = Block::default().borders(Borders::ALL).border_style(border).title(title);
        let inner = block.inner(area);
        let width = inner.width.saturating_sub(1) as usize;
        let (mut lines, mut owners) = match timeline {
            Some(t) if t.loaded || !t.messages.is_empty() => build_lines_owned(ws, t, width, self.thread),
            Some(_) => (vec![Line::styled("Loading messages…", Style::default().fg(DIM))], vec![None]),
            None => (vec![], vec![]),
        };
        if let Some(t) = timeline.filter(|_| !self.thread) {
            let note = if t.loading && t.loaded {
                Some("Loading older messages…")
            } else if t.loaded && !t.has_more && !t.messages.is_empty() {
                Some("— beginning of conversation —")
            } else {
                None
            };
            if let Some(note) = note {
                lines.insert(0, Line::styled(note, Style::default().fg(DIM)));
                owners.insert(0, None);
            }
        }
        // Highlight the selected message's rows.
        let highlight = Style::default().bg(Color::Rgb(0x2c, 0x3e, 0x5c));
        if focused {
            if let Some(sel) = &self.selected {
                for (line, owner) in lines.iter_mut().zip(&owners) {
                    if owner.as_ref() == Some(sel) {
                        *line = std::mem::take(line).patch_style(highlight);
                    }
                }
            }
        }
        self.total_lines = lines.len();
        let height = inner.height as usize;
        // Keep the selection on screen.
        if let (true, Some(sel)) = (focused, &self.selected) {
            if let (Some(first), Some(last)) = (owners.iter().position(|o| o.as_ref() == Some(sel)), owners.iter().rposition(|o| o.as_ref() == Some(sel))) {
                let end = self.total_lines.saturating_sub(self.scroll);
                let start = end.saturating_sub(height);
                if last >= end {
                    self.scroll = self.total_lines - (last + 1);
                } else if first < start {
                    self.scroll = self.total_lines.saturating_sub(first + height);
                }
            }
        }
        self.scroll = self.scroll.min(self.total_lines.saturating_sub(height));
        let end = self.total_lines.saturating_sub(self.scroll);
        let start = end.saturating_sub(height);
        let mut visible: Vec<Line> = lines[start..end].to_vec();
        let mut rows: Vec<Option<String>> = owners[start..end].to_vec();
        // Anchor short timelines to the bottom like a chat.
        if visible.len() < height {
            let pad = height - visible.len();
            let mut padded = vec![Line::raw(""); pad];
            padded.append(&mut visible);
            visible = padded;
            let mut prow = vec![None; pad];
            prow.append(&mut rows);
            rows = prow;
        }
        self.rows = rows;
        f.render_widget(Paragraph::new(visible).block(block), area);
    }
}

fn day_label(ts: f64) -> String {
    Local.timestamp_opt(ts as i64, 0).single().map(|d| d.format("%A, %B %-d").to_string()).unwrap_or_default()
}

fn time_label(ts: f64) -> String {
    Local.timestamp_opt(ts as i64, 0).single().map(|d| d.format("%H:%M").to_string()).unwrap_or_default()
}

fn same_day(a: f64, b: f64) -> bool {
    let d = |t: f64| Local.timestamp_opt(t as i64, 0).single().map(|d| d.date_naive());
    d(a) == d(b)
}

/// System messages (joins, topic changes) rendered as one dim line.
fn is_system(m: &Message) -> bool {
    matches!(
        m.subtype.as_deref(),
        Some("channel_join" | "channel_leave" | "channel_topic" | "channel_purpose" | "channel_name" | "group_join" | "pinned_item")
    )
}

/// Messages shown in the pane, oldest first (selection order): top-level ones, or in
/// `thread` mode all of them (parent first).
pub fn shown_messages(t: &Timeline, thread: bool) -> impl Iterator<Item = &Message> {
    t.messages.iter().filter(move |m| !m.hidden && (thread || !m.is_reply() || m.subtype.as_deref() == Some("thread_broadcast")))
}

/// Render every message in the timeline, oldest first.
#[cfg(test)]
pub fn build_lines(ws: &Workspace, t: &Timeline, width: usize) -> Vec<Line<'static>> {
    build_lines_owned(ws, t, width, false).0
}

/// Lines plus the ts of the message each line belongs to.
pub fn build_lines_owned(ws: &Workspace, t: &Timeline, width: usize, thread: bool) -> (Vec<Line<'static>>, Vec<Option<String>>) {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut owners: Vec<Option<String>> = Vec::new();
    let mut prev: Option<&Message> = None;
    let mut last_day: Option<f64> = None;
    for (n, m) in shown_messages(t, thread).enumerate() {
        let ts = m.time();
        owners.resize(out.len(), None);
        if thread && n == 1 {
            let replies = t.messages.len() - 1;
            let label = format!(" {replies} {} ", if replies == 1 { "reply" } else { "replies" });
            out.push(Line::styled(format!("{label}{}", "─".repeat(width.saturating_sub(label.chars().count()))), Style::default().fg(DIM)));
            prev = None;
        } else if !thread && last_day.is_none_or(|d| !same_day(d, ts)) {
            last_day = Some(ts);
            let label = format!(" {} ", day_label(ts));
            let side = width.saturating_sub(label.chars().count()) / 2;
            out.push(Line::styled(format!("{}{}{}", "─".repeat(side), label, "─".repeat(side)), Style::default().fg(DIM)));
            prev = None;
        }
        if is_system(m) {
            let body = format::plain_text(&format::message_blocks(&m.text, &m.blocks), &|i| text::inline_plain(ws, i));
            out.push(Line::styled(format!("{}  {}", time_label(ts), body), Style::default().fg(DIM).add_modifier(Modifier::ITALIC)));
            owners.resize(out.len(), Some(m.ts.clone()));
            prev = None;
            continue;
        }
        let grouped = prev.is_some_and(|p| p.user == m.user && p.bot_id == m.bot_id && ts - p.time() < GROUP_WINDOW && !is_system(p));
        if !grouped {
            let color = m.user.as_deref().map(text::user_color).unwrap_or(Color::Gray);
            out.push(Line::from(vec![
                Span::styled(ws.author(m), Style::default().fg(color).add_modifier(Modifier::BOLD)),
                Span::styled(format!("  {}", time_label(ts)), Style::default().fg(DIM)),
            ]));
        }
        out.extend(message_body(ws, m, width, !thread));
        owners.resize(out.len(), Some(m.ts.clone()));
        prev = Some(m);
    }
    owners.resize(out.len(), None);
    (out, owners)
}

/// Text, files, reactions and (with `summary`) the thread summary of one message.
pub fn message_body(ws: &Workspace, m: &Message, width: usize, summary: bool) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let blocks = format::message_blocks(&m.text, &m.blocks);
    let mut body = text::render_blocks(ws, &blocks, width);
    if m.edited.is_some() {
        let mark = Span::styled(" (edited)", Style::default().fg(DIM));
        match body.last_mut() {
            Some(last) if last.width() + mark.width() <= width => last.spans.push(mark),
            _ => body.push(Line::from(Span::styled("(edited)", Style::default().fg(DIM)))),
        }
    }
    if m.subtype.as_deref() == Some("thread_broadcast") {
        out.push(Line::styled("↳ replied to a thread", Style::default().fg(DIM)));
    }
    out.extend(body);
    for att in &m.attachments {
        let title = att["title"].as_str().or_else(|| att["fallback"].as_str()).unwrap_or("");
        let detail = att["text"].as_str().unwrap_or("");
        let bar = Span::styled("▍ ", Style::default().fg(Color::Rgb(0x6e, 0x6e, 0x6e)));
        if !title.is_empty() {
            out.extend(text::wrap(vec![Span::styled(format::unescape(title), Style::default().add_modifier(Modifier::BOLD))], width, vec![bar.clone()], vec![bar.clone()]));
        }
        if !detail.is_empty() {
            let doc = format::from_mrkdwn(detail);
            for line in text::render_blocks(ws, &doc, width.saturating_sub(2)) {
                let mut spans = vec![bar.clone()];
                spans.extend(line.spans);
                out.push(Line::from(spans));
            }
        }
    }
    for file in &m.files {
        let icon = if file.is_image() {
            "🖼 "
        } else if file.is_video() {
            "🎞 "
        } else if file.is_audio() {
            "🎵 "
        } else {
            "📎 "
        };
        let name = if file.title.is_empty() { &file.name } else { &file.title };
        out.push(Line::from(vec![
            Span::raw(icon),
            Span::styled(name.clone(), Style::default().fg(text::LINK)),
            Span::styled(format!("  {}", text::human_size(file.size)), Style::default().fg(DIM)),
        ]));
    }
    if !m.reactions.is_empty() {
        let mut spans = Vec::new();
        for r in &m.reactions {
            let mine = r.users.iter().any(|u| *u == ws.self_id);
            let style = if mine { Style::default().fg(text::MENTION).add_modifier(Modifier::BOLD) } else { Style::default() };
            spans.push(Span::styled(format!("[{} {}]", text::emoji(ws, &r.name, None), r.count), style));
            spans.push(Span::raw(" "));
        }
        out.extend(text::wrap(spans, width, vec![], vec![]));
    }
    if summary && m.reply_count > 0 {
        let last = m.latest_reply.as_deref().and_then(|t| t.parse::<f64>().ok()).map(|t| format!(" · last {}", time_label(t))).unwrap_or_default();
        let s = if m.reply_count == 1 { "reply" } else { "replies" };
        out.push(Line::styled(format!("💬 {} {s}{last}", m.reply_count), Style::default().fg(text::MENTION)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use slack_core::types::Reaction;

    fn msg(ts: &str, user: &str, text: &str) -> Message {
        Message { ts: ts.into(), user: Some(user.into()), text: text.into(), ..Default::default() }
    }

    fn plain(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.spans.iter().map(|s| s.content.to_string()).collect()).collect()
    }

    #[test]
    fn groups_consecutive_messages() {
        let ws = Workspace::default();
        let t = Timeline { messages: vec![msg("1000.0", "U1", "first"), msg("1010.0", "U1", "second"), msg("1020.0", "U2", "third")], loaded: true, ..Default::default() };
        let p = plain(&build_lines(&ws, &t, 60));
        // day separator, U1 header, first, second, U2 header, third
        assert_eq!(p.len(), 6, "{p:?}");
        assert_eq!(p[2], "first");
        assert_eq!(p[3], "second");
        assert!(p[4].starts_with("U2"));
    }

    #[test]
    fn one_day_separator_around_system_messages() {
        let ws = Workspace::default();
        let mut join = msg("1000.0", "U1", "joined");
        join.subtype = Some("channel_join".into());
        let t = Timeline { messages: vec![join, msg("1010.0", "U2", "hello")], loaded: true, ..Default::default() };
        let p = plain(&build_lines(&ws, &t, 60));
        assert_eq!(p.iter().filter(|l| l.contains('─')).count(), 1, "{p:?}");
    }

    #[test]
    fn edited_mark_wraps_instead_of_overflowing() {
        let ws = Workspace::default();
        let mut m = msg("1000.0", "U1", "exactly ten");
        m.edited = Some(Default::default());
        assert_eq!(plain(&message_body(&ws, &m, 30, true)), vec!["exactly ten (edited)"]);
        assert_eq!(plain(&message_body(&ws, &m, 12, true)), vec!["exactly ten", "(edited)"]);
    }

    #[test]
    fn thread_mode_shows_parent_then_replies() {
        let ws = Workspace::default();
        let mut parent = msg("1000.0", "U1", "parent");
        parent.reply_count = 1;
        parent.thread_ts = Some("1000.0".into());
        let mut reply = msg("1001.0", "U2", "reply");
        reply.thread_ts = Some("1000.0".into());
        let t = Timeline { messages: vec![parent, reply], loaded: true, ..Default::default() };
        let p = plain(&build_lines_owned(&ws, &t, 30, true).0);
        assert_eq!(p[1], "parent", "{p:?}");
        assert!(p[2].starts_with(" 1 reply ─"), "{p:?}");
        assert_eq!(p[4], "reply", "{p:?}");
        assert!(!p.iter().any(|l| l.starts_with("💬")), "{p:?}");
    }

    #[test]
    fn hides_thread_replies_and_shows_summary() {
        let ws = Workspace::default();
        let mut parent = msg("1000.0", "U1", "parent");
        parent.reply_count = 2;
        parent.reactions = vec![Reaction { name: "+1".into(), users: vec!["U2".into()], count: 1 }];
        let mut reply = msg("1001.0", "U2", "reply");
        reply.thread_ts = Some("1000.0".into());
        let t = Timeline { messages: vec![parent, reply], loaded: true, ..Default::default() };
        let p = plain(&build_lines(&ws, &t, 60));
        assert!(!p.iter().any(|l| l == "reply"), "{p:?}");
        assert!(p.iter().any(|l| l.contains("[👍 1]")));
        assert!(p.iter().any(|l| l.starts_with("💬 2 replies")));
    }
}

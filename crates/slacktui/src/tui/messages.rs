//! Message timeline rendering and scrolling.


use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use slack_core::format;
use slack_core::types::{File, Message};

use ratatui_image::sliced::{SignedPosition, SlicedImage};

use super::media::{Media, Thumb};
use super::state::{Timeline, Workspace};
use super::text;

/// Rows reserved for an image thumbnail, starting at line `line` of the rendered timeline.
#[derive(Debug, Clone)]
pub struct ImageSlot {
    pub line: usize,
    /// Column after the gutter (avatars sit before the author name).
    pub col: u16,
    pub file: File,
    pub size: (u16, u16),
}

/// Thumbnail box for a file (None: no inline image).
/// A file's thumbnail size in cells when at most `max_cols` wide (None: no thumbnail).
pub type ThumbSize<'a> = &'a dyn Fn(&File, u16) -> Option<(u16, u16)>;

/// Messages from the same author within this many seconds share one header.
const GROUP_WINDOW: f64 = 300.0;

/// Columns left of every message row; your own messages get a bar there.
const GUTTER: u16 = 2;

/// The fewest messages built for a draw (more when the view or the selection needs them).
const BUILD_CHUNK: usize = 100;

#[derive(Debug, Default)]
pub struct MessagePane {
    /// Lines scrolled up from the bottom (0 = newest visible).
    pub scroll: usize,
    pub area: Rect,
    /// Total rendered lines on the last draw (to detect reaching the top).
    pub total_lines: usize,
    /// The last draw built only the newest messages (older ones are further up).
    partial: bool,
    /// Selected message ts (message focus).
    pub selected: Option<String>,
    /// Message ts owning each visible row of the last draw (for clicks).
    rows: Vec<Option<String>>,
    /// Thread mode: show the parent and every reply instead of top-level messages.
    pub thread: bool,
    /// Message under the mouse pointer.
    pub hover: Option<String>,
    /// The message being edited in this pane's composer: marked whatever has focus.
    pub editing: Option<String>,
    /// Thumbnails that were on screen but not loaded at the last draw.
    pub wanted: Vec<(File, (u16, u16))>,
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
        !self.partial && (self.total_lines <= height || self.scroll + height >= self.total_lines)
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

    /// The message drawn at a terminal position.
    pub fn message_at(&self, column: u16, row: u16) -> Option<String> {
        let inner = self.area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
        if !inner.contains((column, row).into()) {
            return None;
        }
        self.rows.get((row - inner.y) as usize).cloned().flatten()
    }

    /// Select the message drawn at terminal row `row`; true if one was hit.
    pub fn click(&mut self, column: u16, row: u16) -> bool {
        match self.message_at(column, row) {
            Some(ts) => {
                self.selected = Some(ts);
                true
            }
            None => false,
        }
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, ws: &Workspace, title: &str, timeline: Option<&Timeline>, focused: bool, media: &Media) {
        self.area = area;
        let title = if title.is_empty() { String::new() } else { format!(" {title} ") };
        let border = if focused { Style::default().fg(super::theme::p().accent) } else { Style::default().fg(super::theme::p().dim) };
        let block = Block::default().borders(Borders::ALL).border_style(border).title(title);
        let inner = block.inner(area);
        let width = inner.width.saturating_sub(1 + GUTTER) as usize;
        let thumb_size = |file: &File, max_cols: u16| media.thumb_size(file, max_cols).filter(|_| ws.settings.messages.inline_images);
        let avatars = media.avatars && ws.settings.messages.avatars;
        let order: Vec<&str> = timeline.map(|t| shown_messages(t, self.thread).map(|m| m.ts.as_str()).collect()).unwrap_or_default();
        let (mut lines, mut owners, mut slots) = match timeline {
            Some(t) if t.loaded || !t.messages.is_empty() => {
                // Building costs time for every line, on every frame: only the newest messages
                // that cover the view (with a screen to spare), the selection and the message
                // being edited are built. A thread is built whole.
                let shown = &order;
                let need = self.scroll + 2 * inner.height as usize;
                let kept = [&self.selected, &self.editing].into_iter().flatten().filter_map(|ts| shown.iter().position(|s| *s == ts.as_str())).min();
                let mut take = BUILD_CHUNK.max(kept.map_or(0, |i| shown.len() - i));
                loop {
                    let from = if self.thread { 0 } else { shown.len().saturating_sub(take) };
                    let built = build_lines_from(ws, t, from, width, self.thread, &thumb_size, avatars);
                    if from == 0 || built.0.len() >= need {
                        self.partial = from > 0;
                        break built;
                    }
                    take *= 2;
                }
            }
            Some(_) => {
                self.partial = false;
                (vec![Line::styled("Loading messages…", Style::default().fg(super::theme::p().dim))], vec![None], vec![])
            }
            None => {
                self.partial = false;
                (vec![], vec![], vec![])
            }
        };
        if let Some(t) = timeline.filter(|_| !self.thread && !self.partial) {
            let note = if t.loading && t.loaded {
                Some("Loading older messages…")
            } else if t.loaded && !t.has_more && !t.messages.is_empty() {
                Some("— beginning of conversation —")
            } else {
                None
            };
            if let Some(note) = note {
                lines.insert(0, Line::styled(note, Style::default().fg(super::theme::p().dim)));
                owners.insert(0, None);
                for slot in &mut slots {
                    slot.line += 1;
                }
            }
        }
        if let Some(t) = timeline {
            let own: std::collections::HashSet<&str> = t.messages.iter().filter(|m| m.user.as_deref() == Some(ws.self_id.as_str()) && !is_system(m)).map(|m| m.ts.as_str()).collect();
            let system: std::collections::HashSet<&str> = t.messages.iter().filter(|m| is_system(m)).map(|m| m.ts.as_str()).collect();
            add_gutter(&mut lines, &owners, &own, &system);
        }
        // Shade the message under the pointer, then highlight the selected one.
        if let Some(h) = &self.hover {
            for (line, owner) in lines.iter_mut().zip(&owners) {
                if owner.as_ref() == Some(h) {
                    *line = std::mem::take(line).patch_style(Style::default().bg(super::theme::p().hover));
                }
            }
        }
        let highlight = Style::default().bg(super::theme::p().selection);
        if focused {
            if let Some(sel) = &self.selected {
                for (line, owner) in lines.iter_mut().zip(&owners) {
                    if owner.as_ref() == Some(sel) {
                        *line = std::mem::take(line).patch_style(highlight);
                    }
                }
            }
        }
        if let Some(ts) = &self.editing {
            mark_editing(&mut lines, &owners, ts, highlight);
        }
        self.total_lines = lines.len();
        let height = inner.height as usize;
        // Keep the selection (or, from the composer, the message being edited) on screen.
        let keep = if focused { self.selected.as_ref() } else { self.editing.as_ref() };
        if let Some(sel) = keep {
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
        let pad = height.saturating_sub(visible.len());
        if pad > 0 {
            let mut padded = vec![Line::raw(""); pad];
            padded.append(&mut visible);
            visible = padded;
            let mut prow = vec![None; pad];
            prow.append(&mut rows);
            rows = prow;
        }
        self.rows = rows;
        f.render_widget(Paragraph::new(visible).block(block), area);
        // Where we are, counted in messages (only part of a long history is built).
        let mut on_screen: Vec<&str> = self.rows.iter().flatten().map(String::as_str).collect();
        on_screen.dedup();
        let index = |ts: &&str| order.iter().rposition(|o| o == ts);
        if let (Some(top), Some(bottom)) = (on_screen.first().and_then(index), on_screen.last().and_then(index)) {
            super::scroll::indicator(f, area, top, bottom + 1 - top, order.len(), border);
        }
        // Thumbnails over their reserved rows, sliced when partly scrolled out.
        self.wanted.clear();
        for slot in slots {
            let top = slot.line as isize - start as isize + pad as isize;
            if top + slot.size.1 as isize <= 0 || top >= height as isize {
                continue;
            }
            match media.thumbs.get(&slot.file.id) {
                Some(Thumb::Ready(proto)) => {
                    let position = SignedPosition { x: slot.col as i16, y: top as i16 };
                    let beside = Rect { x: inner.x + GUTTER, width: inner.width.saturating_sub(GUTTER), ..inner };
                    f.render_widget(SlicedImage::new(proto, position), beside);
                }
                // Say why instead of leaving the rows blank (not for profile pictures).
                Some(Thumb::Failed(reason)) if !slot.file.id.starts_with(super::media::AVATAR_PREFIX) => {
                    let x = inner.x + GUTTER + slot.col;
                    let row = Rect { x, y: inner.y + top.max(0) as u16, width: inner.right().saturating_sub(x), height: 1 };
                    f.render_widget(Paragraph::new(failed_thumb(reason)).style(Style::default().fg(super::theme::p().dim)), row);
                }
                Some(_) => {}
                None => self.wanted.push((slot.file, slot.size)),
            }
        }
    }
}

/// Lines of a snippet shown under its file line.
const SNIPPET_PREVIEW: usize = 5;

/// A snippet's first lines in a code box (clipped, not wrapped), then how many are left.
fn snippet_preview(file: &File, width: usize) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = super::highlight::preview(file, Style::default().fg(super::theme::p().code_fg))
        .into_iter()
        .take(SNIPPET_PREVIEW)
        .map(|spans| Line::from(super::highlight::clip_spans(spans, width.saturating_sub(2))))
        .collect();
    let shown = out.len() as u64;
    // Slack's own count first: `lines` includes a trailing newline as a line.
    let more = file.lines_more.or(file.lines.map(|n| n.saturating_sub(shown))).unwrap_or(0);
    if more > 0 {
        let note = Span::styled(format!("+{more} more line{} · o to view", if more == 1 { "" } else { "s" }), Style::default().fg(super::theme::p().dim));
        out.push(Line::from(super::highlight::clip_spans(vec![note], width.saturating_sub(2))));
    }
    text::shaded(out, width)
}

/// Prefix every line with the gutter, which marks where each message starts (`•`, or a
/// heavier `▌` for your own) and runs a bar down your own messages (`own`); system
/// messages get no marker.
/// Highlight the lines of message `ts` and tag its first line "✎ editing".
fn mark_editing(lines: &mut [Line<'static>], owners: &[Option<String>], ts: &str, highlight: Style) {
    let mut first = true;
    for (line, owner) in lines.iter_mut().zip(owners) {
        if owner.as_deref() == Some(ts) {
            *line = std::mem::take(line).patch_style(highlight);
            if first {
                line.spans.push(Span::styled("  ✎ editing", Style::default().fg(super::theme::p().accent).add_modifier(Modifier::ITALIC)));
                first = false;
            }
        }
    }
}

fn add_gutter(lines: &mut [Line<'static>], owners: &[Option<String>], own: &std::collections::HashSet<&str>, system: &std::collections::HashSet<&str>) {
    let mut previous: Option<&str> = None;
    for (line, owner) in lines.iter_mut().zip(owners.iter().chain(std::iter::repeat(&None))) {
        let owner = owner.as_deref();
        let first = owner.is_some() && owner != previous;
        previous = owner;
        let mine = owner.is_some_and(|ts| own.contains(ts));
        let gutter = match (mine, first) {
            (true, true) => Span::styled("▌ ", Style::default().fg(super::theme::p().own)),
            (true, false) => Span::styled("▎ ", Style::default().fg(super::theme::p().own)),
            (false, true) if !owner.is_some_and(|ts| system.contains(ts)) => Span::styled("• ", Style::default().fg(super::theme::p().dim)),
            _ => Span::raw("  "),
        };
        line.spans.insert(0, gutter);
    }
}

fn day_label(ws: &Workspace, ts: f64) -> String {
    super::clock::at(ts as i64).map(|d| d.format(&ws.settings.region.date_format).to_string()).unwrap_or_default()
}

fn time_label(ws: &Workspace, ts: f64) -> String {
    super::clock::at(ts as i64).map(|d| d.format(ws.settings.time_format()).to_string()).unwrap_or_default()
}

fn same_day(a: f64, b: f64) -> bool {
    let d = |t: f64| super::clock::at(t as i64).map(|d| d.date_naive());
    d(a) == d(b)
}

/// System messages (joins, topic changes, archiving) rendered as one dim line; they cannot
/// be edited or deleted.
pub fn is_system(m: &Message) -> bool {
    matches!(
        m.subtype.as_deref(),
        Some(
            "channel_join"
                | "channel_leave"
                | "channel_topic"
                | "channel_purpose"
                | "channel_name"
                | "channel_archive"
                | "channel_unarchive"
                | "group_join"
                | "group_leave"
                | "group_topic"
                | "group_purpose"
                | "group_name"
                | "group_archive"
                | "group_unarchive"
                | "pinned_item"
        )
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
    build_lines_owned(ws, t, width, false, &|_, _| None, false).0
}

/// Lines plus the ts of the message each line belongs to.
/// What stands in the rows of a picture that could not be loaded.
fn failed_thumb(reason: &str) -> String {
    format!("⚠ preview unavailable: {reason}")
}

#[cfg(test)]
pub fn build_lines_owned(ws: &Workspace, t: &Timeline, width: usize, thread: bool, thumb_size: ThumbSize, avatars: bool) -> (Vec<Line<'static>>, Vec<Option<String>>, Vec<ImageSlot>) {
    build_lines_from(ws, t, 0, width, thread, thumb_size, avatars)
}

/// [`build_lines_owned`] from the `from`-th shown message on: the same lines as the end of the
/// whole build (grouping and day separators carry on from the message before it).
pub fn build_lines_from(ws: &Workspace, t: &Timeline, from: usize, width: usize, thread: bool, thumb_size: ThumbSize, avatars: bool) -> (Vec<Line<'static>>, Vec<Option<String>>, Vec<ImageSlot>) {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut owners: Vec<Option<String>> = Vec::new();
    let mut slots: Vec<ImageSlot> = Vec::new();
    let shown: Vec<&Message> = shown_messages(t, thread).collect();
    let before = from.checked_sub(1).and_then(|i| shown.get(i).copied());
    // What the whole build would carry over: a system message ends a group.
    let mut prev: Option<&Message> = before.filter(|p| !is_system(p));
    let mut last_day: Option<f64> = before.map(|p| p.time());
    for (n, m) in shown.into_iter().enumerate().skip(from) {
        let ts = m.time();
        owners.resize(out.len(), None);
        if thread && n == 1 {
            let replies = t.messages.len() - 1;
            let label = format!(" {replies} {} ", if replies == 1 { "reply" } else { "replies" });
            out.push(Line::styled(format!("{label}{}", "─".repeat(width.saturating_sub(label.chars().count()))), Style::default().fg(super::theme::p().dim)));
            owners.resize(out.len(), None);
            prev = None;
        }
        // A day separator where the day changes; in a thread not above the parent.
        if last_day.is_none_or(|d| !same_day(d, ts)) && !(thread && n == 0) {
            last_day = Some(ts);
            let label = format!(" {} ", day_label(ws, ts));
            let side = width.saturating_sub(label.chars().count()) / 2;
            out.push(Line::styled(format!("{}{}{}", "─".repeat(side), label, "─".repeat(side)), Style::default().fg(super::theme::p().dim)));
            owners.resize(out.len(), None);
            prev = None;
        }
        last_day.get_or_insert(ts);
        if is_system(m) {
            let body = format::plain_text(&format::message_blocks(&m.text, &m.blocks), &|i| text::inline_plain(ws, i));
            out.push(Line::styled(format!("{}  {}", time_label(ws, ts), body), Style::default().fg(super::theme::p().dim).add_modifier(Modifier::ITALIC)));
            owners.resize(out.len(), Some(m.ts.clone()));
            prev = None;
            continue;
        }
        let grouped = prev.is_some_and(|p| p.user == m.user && p.bot_id == m.bot_id && ts - p.time() < GROUP_WINDOW && !is_system(p));
        if !grouped {
            // A blank row between author groups (not in compact mode).
            if prev.is_some() && !ws.settings.appearance.compact {
                out.push(Line::raw(""));
                owners.resize(out.len(), None);
            }
            let color = m.user.as_deref().map(text::user_color).unwrap_or(super::theme::p().dim);
            let mut header = vec![
                Span::styled(ws.author(m), Style::default().fg(color).add_modifier(Modifier::BOLD)),
                Span::styled(format!("  {}", time_label(ws, ts)), Style::default().fg(super::theme::p().dim)),
            ];
            if let Some(file) = m.user.as_deref().and_then(|u| ws.users.get(u)).and_then(super::media::avatar_file).filter(|_| avatars) {
                slots.push(ImageSlot { line: out.len(), col: 0, file, size: super::media::AVATAR_SIZE });
                header.insert(0, Span::raw(" ".repeat(super::media::AVATAR_SIZE.0 as usize + 1)));
            }
            out.push(Line::from(header));
        }
        let base = out.len();
        let (body, body_slots) = message_body(ws, m, width, !thread, thumb_size);
        out.extend(body);
        slots.extend(body_slots.into_iter().map(|mut s| {
            s.line += base;
            s
        }));
        owners.resize(out.len(), Some(m.ts.clone()));
        prev = Some(m);
    }
    owners.resize(out.len(), None);
    (out, owners, slots)
}

/// Text, files (with room for thumbnails), reactions and (with `summary`) the thread summary
/// of one message. Slot lines are relative to the returned lines.
pub fn message_body(ws: &Workspace, m: &Message, width: usize, summary: bool, thumb_size: ThumbSize) -> (Vec<Line<'static>>, Vec<ImageSlot>) {
    let mut slots = Vec::new();
    let mut out = Vec::new();
    let blocks = format::message_blocks(&m.text, &m.blocks);
    let mut body = text::render_blocks(ws, &blocks, width);
    if m.edited.is_some() {
        let mark = Span::styled(" (edited)", Style::default().fg(super::theme::p().dim));
        match body.last_mut() {
            Some(last) if last.width() + mark.width() <= width => last.spans.push(mark),
            _ => body.push(Line::from(Span::styled("(edited)", Style::default().fg(super::theme::p().dim)))),
        }
    }
    if !m.pinned_to.is_empty() {
        out.push(Line::styled("📌 pinned", Style::default().fg(super::theme::p().pin)));
    }
    if summary && m.subtype.as_deref() == Some("thread_broadcast") {
        out.push(Line::styled("↳ replied to a thread", Style::default().fg(super::theme::p().dim)));
    }
    out.extend(body);
    for att in &m.attachments {
        let title = att["title"].as_str().or_else(|| att["fallback"].as_str()).unwrap_or("");
        let detail = att["text"].as_str().unwrap_or("");
        let bar = Span::styled("▍ ", Style::default().fg(super::theme::p().quote_bar));
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
            "🖼\u{fe0f} "
        } else if file.is_video() {
            "🎞\u{fe0f} "
        } else if file.is_audio() {
            "🎵 "
        } else {
            "📎 "
        };
        let name = if file.title.is_empty() { &file.name } else { &file.title };
        out.push(Line::from(vec![
            Span::raw(icon),
            Span::styled(name.clone(), Style::default().fg(super::theme::p().link)),
            Span::styled(format!("  {}", text::human_size(file.size)), Style::default().fg(super::theme::p().dim)),
        ]));
        if file.is_snippet() {
            out.extend(snippet_preview(file, width));
        }
        // Fitted to the pane (the thread pane is narrow); a smaller fit is cached separately.
        if let Some(size) = thumb_size(file, width.min(u16::MAX as usize) as u16).filter(|(cols, _)| (*cols as usize) <= width) {
            let mut file = file.clone();
            if thumb_size(&file, u16::MAX) != Some(size) {
                file.id = format!("{}@{}x{}", file.id, size.0, size.1);
            }
            slots.push(ImageSlot { line: out.len(), col: 0, file, size });
            out.extend(std::iter::repeat_n(Line::raw(""), size.1 as usize));
        }
    }
    if !m.reactions.is_empty() {
        let mut spans = Vec::new();
        for r in &m.reactions {
            let mine = r.users.iter().any(|u| *u == ws.self_id);
            let style = if mine { Style::default().fg(super::theme::p().mention).add_modifier(Modifier::BOLD) } else { Style::default() };
            spans.push(Span::styled(format!("[{} {}]", text::emoji(ws, &r.name, None), r.count), style));
            spans.push(Span::raw(" "));
        }
        out.extend(text::wrap(spans, width, vec![], vec![]));
    }
    if summary && m.reply_count > 0 {
        let last = m.latest_reply.as_deref().and_then(|t| t.parse::<f64>().ok()).map(|t| format!(" · last {}", time_label(ws, t))).unwrap_or_default();
        let s = if m.reply_count == 1 { "reply" } else { "replies" };
        out.push(Line::styled(format!("💬 {} {s}{last}", m.reply_count), Style::default().fg(super::theme::p().mention)));
    }
    (out, slots)
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
    fn edited_message_is_marked() {
        let mut lines = vec![Line::raw("Bob 10:00"), Line::raw("first"), Line::raw("second"), Line::raw("other")];
        let owners = vec![Some("1.0".to_string()), Some("2.0".into()), Some("2.0".into()), Some("3.0".into())];
        let hl = Style::default().bg(ratatui::style::Color::Blue);
        mark_editing(&mut lines, &owners, "2.0", hl);
        assert_eq!(plain(&lines), vec!["Bob 10:00", "first  ✎ editing", "second", "other"]);
        assert_eq!((lines[1].style.bg, lines[2].style.bg, lines[0].style.bg, lines[3].style.bg), (hl.bg, hl.bg, None, None));
    }

    #[test]
    fn thumbnails_fit_narrow_panes() {
        let ws = Workspace::default();
        let file = File { id: "F1".into(), mimetype: "image/png".into(), thumb_360: Some("https://x/t.png".into()), original_w: Some(800), original_h: Some(500), ..Default::default() };
        let m = Message { ts: "1.0".into(), user: Some("U1".into()), files: vec![file], ..Default::default() };
        // 10×20 px cells, at most 36×10: an 800×500 image is 32×10 at full size.
        let sizes = |f: &File, max: u16| super::super::media::thumb_cells(f, ratatui_image::FontSize::new(10, 20), 36u16.min(max), 10);
        let (_, full) = message_body(&ws, &m, 60, true, &sizes);
        assert_eq!((full[0].size, full[0].file.id.as_str()), ((32, 10), "F1"));
        // The thread pane at 30 columns: smaller, and cached under its own id.
        let (_, narrow) = message_body(&ws, &m, 30, true, &sizes);
        assert_eq!(narrow.len(), 1, "a narrow pane still shows the thumbnail");
        assert_eq!(narrow[0].size.0, 30);
        assert!(narrow[0].file.id.starts_with("F1@30x"));
    }

    #[test]
    fn archive_notices_are_system_messages() {
        // Subtypes as Slack returned them in #history-paging.
        for subtype in ["channel_archive", "channel_unarchive", "group_archive", "channel_join"] {
            let m = Message { subtype: Some(subtype.into()), user: Some("U1".into()), ..Default::default() };
            assert!(is_system(&m), "{subtype}");
        }
        for subtype in [None, Some("thread_broadcast"), Some("file_share")] {
            let m = Message { subtype: subtype.map(String::from), ..Default::default() };
            assert!(!is_system(&m), "{subtype:?}");
        }
    }

    #[test]
    fn snippet_preview_is_clipped_and_counted() {
        let f = File { mode: "snippet".into(), preview: Some("fn main() {\n\tlet a_very_long_name = 1;\n}".into()), lines: Some(9), ..Default::default() };
        let rows = plain(&snippet_preview(&f, 20));
        assert_eq!(rows, vec![" fn main() {        ", "     let a_very_lo… ", " }                  ", " +6 more lines · o… "], "a shaded box, every row as wide");
        assert!(snippet_preview(&f, 20).iter().all(|l| l.spans.iter().all(|s| s.style.bg == Some(crate::tui::theme::p().code_bg))));
        let short = File { mode: "snippet".into(), preview: Some("one".into()), lines: Some(1), ..Default::default() };
        assert_eq!(plain(&snippet_preview(&short, 20)), vec![" one "]);
        // Real values for a 3-line file ending in a newline.
        let trailing = File { mode: "snippet".into(), preview: Some("a\nb\nc\n".into()), lines: Some(4), lines_more: Some(0), ..Default::default() };
        assert_eq!(plain(&snippet_preview(&trailing, 20)).len(), 3);
    }

    #[test]
    fn icons_are_two_cells_everywhere() {
        // Narrow by Unicode, but drawn two cells wide by emoji fonts (kitty paints the glyph over
        // the following space); with the emoji selector both sides count two.
        use unicode_width::UnicodeWidthStr;
        for icon in ["🖼\u{fe0f}", "🎞\u{fe0f}", "🖥\u{fe0f}", "🎙\u{fe0f}"] {
            assert_eq!(icon.width(), 2, "{icon:?}");
            assert_eq!(ratatui::text::Span::raw(icon).width(), 2, "{icon:?}");
        }
        assert_eq!("🖼".width(), 1, "without the selector: one cell");
    }

    #[test]
    fn failed_thumbnails_say_why() {
        // As `load_thumb` reports a corrupt upload (`{:#}` of its error).
        let e = "not a picture that can be shown: The image format could not be determined";
        assert_eq!(super::failed_thumb(super::super::media::short_reason(e)), "⚠ preview unavailable: not a picture that can be shown");
        assert_eq!(super::super::media::short_reason("no preview"), "no preview");
    }

    /// How long building the lines of a big conversation takes (it happens on every frame):
    /// `cargo test --release -p slacktui build_lines_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn build_lines_cost() {
        let mut ws = Workspace::default();
        for (id, name) in [("U1", "ann"), ("U2", "bob"), ("U3", "carol")] {
            ws.users.insert(id.into(), slack_core::types::User { id: id.into(), name: name.into(), ..Default::default() });
        }
        let texts = [
            "short one",
            "a *bold* and _italic_ line with a <https://example.com|link> and <@U2>",
            "```fn main() {\n    println!(\"hi\");\n}```",
            "a longer message that wraps over several rows of the pane because it goes on and on about nothing in particular, like real chat does",
        ];
        for n in [1_000usize, 5_000, 20_000] {
            let mut t = Timeline { loaded: true, ..Default::default() };
            t.messages = (0..n).map(|i| msg(&format!("{}.000100", 1_700_000_000 + i * 97), ["U1", "U2", "U3"][i % 3], texts[i % texts.len()])).collect();
            let start = std::time::Instant::now();
            let runs = 5;
            let mut rows = 0;
            for _ in 0..runs {
                rows = build_lines_owned(&ws, &t, 100, false, &|_, _| None, false).0.len();
            }
            let whole = start.elapsed().as_secs_f64() * 1000.0 / runs as f64;
            // What a draw at the bottom builds now: the newest BUILD_CHUNK messages.
            let start = std::time::Instant::now();
            for _ in 0..runs {
                build_lines_from(&ws, &t, n - super::BUILD_CHUNK, 100, false, &|_, _| None, false);
            }
            let newest = start.elapsed().as_secs_f64() * 1000.0 / runs as f64;
            println!("{n} messages ({rows} rows): whole {whole:.1} ms, newest {} {newest:.2} ms per frame", super::BUILD_CHUNK);
        }
    }

    #[test]
    fn building_from_any_message_matches_the_whole() {
        let mut ws = Workspace::default();
        ws.users.insert("U1".into(), slack_core::types::User { id: "U1".into(), name: "ann".into(), ..Default::default() });
        ws.users.insert("U2".into(), slack_core::types::User { id: "U2".into(), name: "bob".into(), ..Default::default() });
        let mut t = Timeline { loaded: true, ..Default::default() };
        // Groups, a gap past the group window, a day change, a system message and a hidden reply.
        let day = 86_400.0;
        let at = |secs: f64, user: &str, text: &str| msg(&format!("{secs:.6}"), user, text);
        let mut join = at(1_700_000_040.0 + day, "U2", "<@U2> has joined the channel");
        join.subtype = Some("channel_join".into());
        let mut reply = at(1_700_000_055.0 + day, "U1", "a reply");
        reply.thread_ts = Some(format!("{:.6}", 1_700_000_000.0 + day));
        for m in [
            at(1_700_000_000.0, "U1", "one"),
            at(1_700_000_010.0, "U1", "grouped with one"),
            at(1_700_000_020.0, "U2", "bob speaks"),
            at(1_700_004_000.0, "U2", "bob again, much later"),
            at(1_700_000_000.0 + day, "U1", "the next day"),
            at(1_700_000_030.0 + day, "U1", "and more"),
            join,
            at(1_700_000_050.0 + day, "U2", "after joining"),
            reply,
            at(1_700_000_060.0 + day, "U2", "last"),
        ] {
            t.upsert(m);
        }
        let whole = plain(&build_lines_owned(&ws, &t, 40, false, &|_, _| None, false).0);
        assert!(whole.iter().filter(|l| l.starts_with('─')).count() >= 2 && whole.iter().any(|l| l.contains("has joined")), "{whole:#?}");
        assert!(!whole.iter().any(|l| l.contains("a reply")), "replies stay in their thread");
        let shown = shown_messages(&t, false).count();
        assert_eq!(shown, 9);
        for from in 0..=shown {
            let (lines, owners, _) = build_lines_from(&ws, &t, from, 40, false, &|_, _| None, false);
            let part = plain(&lines);
            assert!(whole.ends_with(&part), "from {from}: {part:#?}\nis not the end of\n{whole:#?}");
            assert_eq!(lines.len(), owners.len());
        }
    }

    #[test]
    fn avatar_before_the_author() {
        let mut ws = Workspace::default();
        let mut bob = slack_core::types::User { id: "U2".into(), name: "bob".into(), ..Default::default() };
        bob.profile.image_48 = Some("https://avatars.slack-edge.com/x_48.png".into());
        ws.users.insert("U2".into(), bob);
        let mut t = Timeline { loaded: true, ..Default::default() };
        t.upsert(msg("1.0", "U2", "one"));
        t.upsert(msg("2.0", "U2", "two"));
        let (lines, _, slots) = build_lines_owned(&ws, &t, 40, false, &|_, _| None, true);
        let header = plain(&lines).into_iter().position(|l| l.contains("bob")).unwrap();
        assert!(plain(&lines)[header].starts_with("   bob"), "room for the picture");
        assert_eq!(slots.len(), 1, "grouped messages share one header and one picture");
        assert_eq!((slots[0].line, slots[0].col, slots[0].size, slots[0].file.id.as_str()), (header, 0, (2, 1), "avatar:U2"));
        let (_, _, none) = build_lines_owned(&ws, &t, 40, false, &|_, _| None, false);
        assert!(none.is_empty());
    }

    #[test]
    fn gutter_marks_message_starts_and_own_messages() {
        let ws = Workspace { self_id: "UME".into(), ..Default::default() };
        let mut t = Timeline { loaded: true, ..Default::default() };
        t.upsert(msg("1.0", "U2", "theirs"));
        t.upsert(msg("2.0", "U2", "theirs again"));
        t.upsert(msg("3.0", "UME", "mine\ntwo lines"));
        let (mut lines, owners, _) = build_lines_owned(&ws, &t, 40, false, &|_, _| None, false);
        let own = ["3.0"].into_iter().collect();
        add_gutter(&mut lines, &owners, &own, &Default::default());
        let rows = plain(&lines);
        assert!(rows.iter().all(|r| ["▌ ", "▎ ", "• ", "  "].iter().any(|g| r.starts_with(g))), "every row has the gutter: {rows:?}");
        // U2's header, then the start of U2's grouped second message.
        let bullets: Vec<&String> = rows.iter().filter(|r| r.starts_with('•')).collect();
        assert_eq!(bullets.len(), 2, "{rows:?}");
        assert!(bullets[1].contains("theirs again"), "a grouped message still shows where it starts: {rows:?}");
        let mine: Vec<&String> = rows.iter().filter(|r| r.starts_with('▌') || r.starts_with('▎')).collect();
        assert_eq!(mine.len(), 3, "header and both lines of our message: {rows:?}");
        assert!(mine[0].starts_with('▌') && mine[1].starts_with('▎'));
        let blank = rows.iter().position(|r| r.trim().is_empty()).expect("a blank row between the two authors");
        assert!(rows[blank + 1].contains("UME") || rows[blank + 1].contains("You") || rows[blank + 1].starts_with('▌'), "{rows:?}");
    }

    #[test]
    fn groups_consecutive_messages() {
        let ws = Workspace::default();
        let t = Timeline { messages: vec![msg("1000.0", "U1", "first"), msg("1010.0", "U1", "second"), msg("1020.0", "U2", "third")], loaded: true, ..Default::default() };
        let p = plain(&build_lines(&ws, &t, 60));
        // day separator, U1 header, first, second, blank row between authors, U2 header, third
        assert_eq!(p.len(), 7, "{p:?}");
        assert_eq!(p[2], "first");
        assert_eq!(p[3], "second");
        assert_eq!(p[4], "");
        assert!(p[5].starts_with("U2"));
    }

    #[test]
    fn one_day_separator_around_system_messages() {
        let ws = Workspace::default();
        let mut join = msg("1000.0", "U1", "joined");
        join.subtype = Some("channel_join".into());
        let t = Timeline { messages: vec![join, msg("1010.0", "U2", "hello")], loaded: true, ..Default::default() };
        let (lines, owners, _) = build_lines_owned(&ws, &t, 60, false, &|_, _| None, false);
        let p = plain(&lines);
        assert_eq!(p.iter().filter(|l| l.contains('─')).count(), 1, "{p:?}");
        assert_eq!(owners[0], None, "the day separator belongs to no message");
    }

    #[test]
    fn image_files_reserve_rows() {
        let ws = Workspace::default();
        let mut m = msg("1000.0", "U1", "look");
        m.files = vec![File { id: "F1".into(), name: "cat.png".into(), mimetype: "image/png".into(), ..Default::default() }];
        let t = Timeline { messages: vec![m], loaded: true, ..Default::default() };
        let (lines, _, slots) = build_lines_owned(&ws, &t, 40, false, &|f, _| (f.id == "F1").then_some((20, 3)), false);
        // day separator, header, "look", file line, 3 reserved rows
        assert_eq!(lines.len(), 7);
        assert_eq!(slots.len(), 1);
        assert_eq!((slots[0].line, slots[0].size), (4, (20, 3)));
        // A size that cannot fit (the sizer ignored the width): text only.
        let (narrow, _, none) = build_lines_owned(&ws, &t, 10, false, &|_, _| Some((20, 3)), false);
        assert!(none.is_empty() && narrow.len() < 7);
    }

    #[test]
    fn edited_mark_wraps_instead_of_overflowing() {
        let ws = Workspace::default();
        let mut m = msg("1000.0", "U1", "exactly ten");
        m.edited = Some(Default::default());
        assert_eq!(plain(&message_body(&ws, &m, 30, true, &|_, _| None).0), vec!["exactly ten (edited)"]);
        assert_eq!(plain(&message_body(&ws, &m, 12, true, &|_, _| None).0), vec!["exactly ten", "(edited)"]);
    }

    #[test]
    fn thread_pane_day_separators() {
        let ws = Workspace::default();
        let day = 86_400.0;
        let mut parent = msg("1000000.0", "U1", "parent");
        parent.thread_ts = Some("1000000.0".into());
        let reply = |ts: f64, text: &str| {
            let mut r = msg(&format!("{ts:.1}"), "U2", text);
            r.thread_ts = Some("1000000.0".into());
            r
        };
        let t = Timeline { messages: vec![parent, reply(1_000_060.0, "same day"), reply(1_000_000.0 + 2.0 * day, "two days later"), reply(1_000_000.0 + 2.0 * day + 60.0, "and again")], loaded: true, ..Default::default() };
        let (lines, owners, _) = build_lines_owned(&ws, &t, 40, true, &|_, _| None, false);
        let p = plain(&lines);
        let seps: Vec<usize> = p.iter().enumerate().filter(|(_, l)| l.starts_with('─')).map(|(i, _)| i).collect();
        assert_eq!(seps.len(), 1, "one change of day among the replies: {p:?}");
        assert_eq!(owners[seps[0]], None);
        assert_eq!(p[seps[0] + 2], "two days later", "{p:?}");
        assert_eq!(p[1], "parent", "no separator above the parent: {p:?}");
        // A first reply on a later day: the separator comes after the replies rule.
        let t = Timeline { messages: vec![t.messages[0].clone(), t.messages[2].clone()], loaded: true, ..Default::default() };
        let p = plain(&build_lines_owned(&ws, &t, 40, true, &|_, _| None, false).0);
        assert!(p[2].starts_with(" 1 reply") && p[3].starts_with('─'), "{p:?}");
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
        let (lines, owners, _) = build_lines_owned(&ws, &t, 30, true, &|_, _| None, false);
        let p = plain(&lines);
        assert_eq!(owners[2], None, "the replies rule belongs to no message");
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

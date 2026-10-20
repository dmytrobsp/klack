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

/// What stands before an author's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Avatars {
    /// Nothing (profile pictures are turned off).
    Off,
    /// Two-letter initials in the author's colour (no graphics).
    Initials,
    /// Their picture over the initials (which show while it loads, and for bots).
    Pictures,
}

/// "Lena Park" → "LP", "deploybot" → "DE": two cells wide (one wide character alone).
pub fn initials(name: &str) -> String {
    use unicode_width::UnicodeWidthChar;
    let words: Vec<&str> = name.split_whitespace().collect();
    let letters: Vec<char> = match words.as_slice() {
        [first, second, ..] => first.chars().take(1).chain(second.chars().take(1)).collect(),
        [one] => one.chars().take(2).collect(),
        [] => vec!['?'],
    };
    let mut out = String::new();
    let mut used = 0;
    for c in letters.into_iter().flat_map(char::to_uppercase) {
        let w = c.width().unwrap_or(1);
        if used + w > 2 {
            break;
        }
        out.push(c);
        used += w;
    }
    format!("{out:<w$}", w = out.chars().count() + 2 - used)
}

/// The fewest messages built for a draw (more when the view or the selection needs them).
const BUILD_CHUNK: usize = 100;

#[derive(Debug, Default)]
pub struct MessagePane {
    /// The message at the bottom edge of the view (None: the newest, following new ones), so a
    /// draw builds only the messages around the view however far up it is.
    anchor: Option<String>,
    /// Lines of the anchor message (of everything, when following) hidden below the view.
    scroll: usize,
    /// Lines to scroll down past the anchor, applied at the next draw.
    down: usize,
    pub area: Rect,
    /// The top of the loaded history was on screen at the last draw.
    top: bool,
    /// Selected message ts (message focus).
    pub selected: Option<String>,
    /// The selection (or message being edited) last brought into view: it is brought into view
    /// once, when it changes, so the wheel can scroll away from it afterwards.
    shown: Option<String>,
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
    /// Read up to here when the conversation was opened with unreads: a "— new —" line stands
    /// above the first newer message from someone else.
    pub new_since: Option<String>,
    /// Bring the "— new —" line into view at the next draw (set when a conversation opens).
    pub jump_to_new: bool,
}

impl MessagePane {
    /// A pane in thread mode.
    pub fn thread() -> Self {
        MessagePane { thread: true, ..Default::default() }
    }

    #[cfg(test)]
    pub fn offset(&self) -> usize {
        self.scroll
    }

    pub fn scroll_up(&mut self, n: usize) {
        let back = n.min(self.down);
        self.down -= back;
        self.scroll = self.scroll.saturating_add(n - back);
    }

    pub fn scroll_down(&mut self, n: usize) {
        let here = n.min(self.scroll);
        self.scroll -= here;
        self.down += n - here;
    }

    /// How many different messages the last draw showed.
    pub fn shown_count(&self) -> usize {
        self.rows.iter().flatten().collect::<std::collections::HashSet<_>>().len()
    }

    /// Back to the newest message, following new ones.
    pub fn to_bottom(&mut self) {
        (self.anchor, self.scroll, self.down) = (None, 0, 0);
    }

    /// True once the top of the loaded history is on screen.
    pub fn at_top(&self) -> bool {
        self.top
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

    /// Where a menu for a message opens: its first row on screen, past the gutter (the pane's
    /// middle when it is not on screen).
    pub fn spot(&self, ts: &str) -> (u16, u16) {
        let inner = self.area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
        match self.rows.iter().position(|o| o.as_deref() == Some(ts)) {
            Some(row) => (inner.x + GUTTER + 2, inner.y + row as u16),
            None => (inner.x + inner.width / 3, inner.y + inner.height / 3),
        }
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
        let avatars = if !ws.settings.messages.avatars {
            Avatars::Off
        } else if media.avatars {
            Avatars::Pictures
        } else {
            Avatars::Initials
        };
        let order: Vec<&str> = timeline.map(|t| shown_messages(t, self.thread).map(|m| m.ts.as_str()).collect()).unwrap_or_default();
        // The first newer message from someone else: the "— new —" line goes above it.
        let first_new: Option<String> = match (timeline, self.new_since.as_deref().filter(|_| !self.thread)) {
            (Some(t), Some(since)) => shown_messages(t, false).find(|m| ts_after(&m.ts, since) && m.user.as_deref() != Some(ws.self_id.as_str())).map(|m| m.ts.clone()),
            _ => None,
        };
        // Nothing new once the history is in: nothing to jump to.
        if first_new.is_none() && timeline.is_some_and(|t| t.loaded) {
            self.jump_to_new = false;
        }
        let height = inner.height as usize;
        // The part of the timeline that is built: `from..to` among the shown messages.
        let (mut from, mut to) = (0, order.len());
        let (mut lines, mut owners, mut slots) = match timeline {
            Some(t) if t.loaded || !t.messages.is_empty() => {
                // Building costs time for every line, on every frame: only the messages around
                // the view (with a screen to spare), the selection and the message being edited
                // are built, up to the anchor at the view's bottom edge. A thread is built whole.
                let last = order.len();
                let anchored = self.anchor.as_deref().and_then(|a| order.iter().rposition(|o| *o == a));
                if self.anchor.is_some() && anchored.is_none() {
                    // The anchor went away (deleted): back to the newest.
                    self.to_bottom();
                }
                to = anchored.map_or(last, |i| i + 1);
                // The view's bottom edge: the build reaches up from here.
                let edge = to;
                let jump = first_new.as_ref().filter(|_| self.jump_to_new);
                let kept: Vec<usize> = [&self.selected, &self.editing, &jump.cloned()].into_iter().flatten().filter_map(|ts| order.iter().rposition(|s| *s == ts.as_str())).collect();
                if let Some(&k) = kept.iter().max() {
                    to = to.max(k + 1);
                }
                let kept_first = kept.iter().min().copied();
                let mut take = BUILD_CHUNK;
                let mut more = BUILD_CHUNK;
                loop {
                    from = if self.thread { 0 } else { edge.saturating_sub(take) };
                    if let Some(k) = kept_first {
                        from = from.min(k);
                    }
                    let end = if self.thread { last } else { to };
                    let built = build_lines_range(ws, t, from, end, width, self.thread, &thumb_size, avatars, first_new.as_deref());
                    // Lines after the anchor message (to scroll down past it).
                    let below_anchor = anchored.and_then(|i| order.get(i)).and_then(|a| built.1.iter().rposition(|o| o.as_deref() == Some(*a))).map_or(0, |l| built.0.len() - 1 - l);
                    let enough_above = from == 0 || built.0.len() - below_anchor >= self.scroll + 2 * height;
                    let enough_below = self.down <= below_anchor || end == last;
                    if enough_above && enough_below {
                        to = end;
                        // Scrolling down past the anchor: the lines after it, less those asked.
                        if anchored.is_some() {
                            self.scroll = (self.scroll + below_anchor).saturating_sub(self.down);
                        }
                        self.down = 0;
                        break built;
                    }
                    if !enough_above {
                        take *= 2;
                    }
                    if !enough_below {
                        to = (to + more).min(last);
                        more *= 2;
                    }
                }
            }
            Some(_) => (vec![Line::styled("Loading messages…", Style::default().fg(super::theme::p().dim))], vec![None], vec![]),
            None => (vec![], vec![], vec![]),
        };
        let partial = from > 0;
        if let Some(t) = timeline.filter(|_| !self.thread && !partial) {
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
        let mut new_line = None;
        if let Some(first) = &first_new {
            if let Some(at) = owners.iter().position(|o| o.as_ref() == Some(first)) {
                new_line = Some(at);
                let label = " new ";
                let side = width.saturating_sub(label.len()) / 2;
                lines.insert(at, Line::styled(format!("{}{label}{}", "─".repeat(side), "─".repeat(side)), Style::default().fg(super::theme::p().danger)));
                owners.insert(at, None);
                for slot in slots.iter_mut().filter(|s| s.line >= at) {
                    slot.line += 1;
                }
            }
        }
        if let Some(t) = timeline {
            let own: std::collections::HashSet<&str> = t.messages.iter().filter(|m| m.user.as_deref() == Some(ws.self_id.as_str()) && !is_system(m)).map(|m| m.ts.as_str()).collect();
            let system: std::collections::HashSet<&str> = t.messages.iter().filter(|m| is_system(m)).map(|m| m.ts.as_str()).collect();
            let authors: std::collections::HashMap<&str, &str> = t.messages.iter().filter_map(|m| Some((m.ts.as_str(), m.user.as_deref()?))).collect();
            add_gutter(&mut lines, &owners, &own, &system, &authors);
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
        let total = lines.len();
        // Bring a new selection (or, from the composer, the message being edited) into view.
        let keep = if focused { self.selected.as_ref() } else { self.editing.as_ref() };
        if keep != self.shown.as_ref() {
            self.shown = None;
        }
        if let Some(sel) = keep.filter(|_| self.shown.is_none()) {
            if let (Some(first), Some(last)) = (owners.iter().position(|o| o.as_ref() == Some(sel)), owners.iter().rposition(|o| o.as_ref() == Some(sel))) {
                self.scroll = keep_in_view(total, height, self.scroll, first, last);
                self.shown = Some(sel.clone());
            }
        }
        // Just opened with more unread than fits: up to the "— new —" line.
        if let Some(at) = new_line.filter(|_| self.jump_to_new) {
            let start = total.saturating_sub(self.scroll).saturating_sub(height);
            if at < start {
                self.scroll = total.saturating_sub(at + height);
            }
            self.jump_to_new = false;
        }
        self.scroll = self.scroll.min(total.saturating_sub(height));
        let end = total.saturating_sub(self.scroll);
        let start = end.saturating_sub(height);
        self.top = !partial && start == 0;
        // Anchor the view to the message at its bottom row (or the next one, for a separator
        // row), unless it shows the newest message's end: then it follows new ones.
        if to == order.len() && end == total {
            (self.anchor, self.scroll) = (None, 0);
        } else if let Some((at, owner)) = owners.iter().enumerate().skip(end.saturating_sub(1)).find_map(|(n, o)| o.as_ref().map(|o| (n, o))) {
            let last_line = owners.iter().rposition(|o| o.as_ref() == Some(owner)).unwrap_or(at);
            self.anchor = Some(owner.clone());
            self.scroll = last_line + 1 - end.min(last_line + 1);
        }
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
        // Where we are, counted in messages (only part of a long history is built).
        let mut on_screen: Vec<&str> = self.rows.iter().flatten().map(String::as_str).collect();
        on_screen.dedup();
        let index = |ts: &&str| order.iter().rposition(|o| o == ts);
        // Reading further up: how many messages are below the view.
        let newer = on_screen.last().and_then(index).filter(|_| self.anchor.is_some()).map_or(0, |b| order.len() - 1 - b);
        let block = match newer {
            0 => block,
            n => block.title_bottom(Line::styled(format!(" ↓ {n} newer "), Style::default().fg(super::theme::p().accent)).right_aligned()),
        };
        f.render_widget(Paragraph::new(visible).block(block), area);
        // Links the terminal can open on its own click.
        if let Some(t) = timeline {
            for spot in link_spots(&t.messages, &lines, &owners).into_iter().filter(|s| (start..end).contains(&s.line)) {
                let y = inner.y + (pad + spot.line - start) as u16;
                let x = inner.x + spot.col;
                let width = spot.width.min(inner.right().saturating_sub(x));
                hyperlink(f.buffer_mut(), x, y, width, &spot.url);
            }
        }
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
                Some(Thumb::Failed(reason)) if !slot.file.id.starts_with(super::media::AVATAR_PREFIX) && !slot.file.id.starts_with(super::media::EMOJI_PREFIX) => {
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

/// The scroll (lines hidden below the view) that shows lines `first..=last` of `total`: moved as
/// little as it takes; a message taller than the view shows from its start, unless the view is
/// already all inside it.
fn keep_in_view(total: usize, height: usize, scroll: usize, first: usize, last: usize) -> usize {
    let end = total.saturating_sub(scroll);
    let start = end.saturating_sub(height);
    let from_start = total.saturating_sub(first + height);
    if last + 1 - first > height {
        if first <= start && last + 1 >= end { scroll } else { from_start }
    } else if last >= end {
        total - (last + 1)
    } else if first < start {
        from_start
    } else {
        scroll
    }
}

/// Lines of a snippet shown under its file line.
const SNIPPET_PREVIEW: usize = 5;

/// The open key as the snippet preview names it (`␣o`; empty: not set yet).
static OPEN_KEY: std::sync::RwLock<String> = std::sync::RwLock::new(String::new());

/// The key that opens a message's things, for "+N more lines · ␣o to view".
pub fn set_open_key(key: String) {
    *OPEN_KEY.write().unwrap_or_else(|e| e.into_inner()) = key;
}

/// A snippet's first lines in a code box (clipped, not wrapped), then how many are left.
fn snippet_preview(file: &File, width: usize, open_key: &str) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = super::highlight::preview(file, Style::default().fg(super::theme::p().code_fg))
        .into_iter()
        .take(SNIPPET_PREVIEW)
        .map(|spans| Line::from(super::highlight::clip_spans(spans, width.saturating_sub(2))))
        .collect();
    let shown = out.len() as u64;
    // Slack's own count first: `lines` includes a trailing newline as a line.
    let more = file.lines_more.or(file.lines.map(|n| n.saturating_sub(shown))).unwrap_or(0);
    if more > 0 {
        let view = if open_key.is_empty() { String::new() } else { format!(" · {open_key} to view") };
        let note = Span::styled(format!("+{more} more line{}{view}", if more == 1 { "" } else { "s" }), Style::default().fg(super::theme::p().dim));
        out.push(Line::from(super::highlight::clip_spans(vec![note], width.saturating_sub(2))));
    }
    text::shaded(out, width)
}

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

/// Prefix every line with the gutter: a bar down each message, heavier where it starts — yours (`own`, `▌` then `▎`)
/// in the theme's own colour, others' (`▎` then `▏`) thinner and dim in their name's colour (`authors`: message →
/// user), so yours stand out; system messages get none.
fn add_gutter(lines: &mut [Line<'static>], owners: &[Option<String>], own: &std::collections::HashSet<&str>, system: &std::collections::HashSet<&str>, authors: &std::collections::HashMap<&str, &str>) {
    let mut previous: Option<&str> = None;
    for (line, owner) in lines.iter_mut().zip(owners.iter().chain(std::iter::repeat(&None))) {
        let owner = owner.as_deref();
        let first = owner.is_some() && owner != previous;
        previous = owner;
        let gutter = match owner {
            None => Span::raw("  "),
            Some(ts) if system.contains(ts) => Span::raw("  "),
            Some(ts) if own.contains(ts) => Span::styled(if first { "▌ " } else { "▎ " }, Style::default().fg(super::theme::p().own)),
            Some(ts) => Span::styled(if first { "▎ " } else { "▏ " }, Style::default().fg(authors.get(ts).map_or(super::theme::p().dim, |u| text::user_color(u))).add_modifier(Modifier::DIM)),
        };
        line.spans.insert(0, gutter);
    }
}

/// Slack timestamp `a` is later than `b` (compared exactly: seconds, then the fraction).
fn ts_after(a: &str, b: &str) -> bool {
    let key = |ts: &str| {
        let (s, frac) = ts.split_once('.').unwrap_or((ts, ""));
        (s.parse::<u64>().unwrap_or(0), format!("{frac:0<6}"))
    };
    key(a) > key(b)
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
    build_lines_owned(ws, t, width, false, &|_, _| None, Avatars::Off).0
}

/// Lines plus the ts of the message each line belongs to.
/// An attachment's own colour (bots mark success, warnings and failures with it): `#2eb886`,
/// `2eb886`, or Slack's names `good`, `warning`, `danger`.
fn attachment_color(att: &serde_json::Value) -> Option<ratatui::style::Color> {
    let c = att["color"].as_str()?.trim();
    let hex = match c {
        "good" => "2eb886",
        "warning" => "daa038",
        "danger" => "a30200",
        _ => c.trim_start_matches('#'),
    };
    let v = u32::from_str_radix(hex, 16).ok().filter(|_| hex.len() == 6)?;
    Some(ratatui::style::Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// A reaction as a pill: " 👍 2 " on a shaded background, yours highlighted; the spaces
/// inside do not break, so a pill never wraps apart. Themes without a code background (the
/// terminal's own colours) keep "[👍 2]".
fn reaction_pill(emoji: &str, count: u64, mine: bool) -> Span<'static> {
    let p = super::theme::p();
    if p.code_bg == ratatui::style::Color::Reset {
        let style = if mine { Style::default().fg(p.mention).add_modifier(Modifier::BOLD) } else { Style::default() };
        return Span::styled(format!("[{emoji}\u{a0}{count}]"), style);
    }
    let style = if mine { Style::default().bg(p.selection).fg(p.mention).add_modifier(Modifier::BOLD) } else { Style::default().bg(p.code_bg) };
    Span::styled(format!("\u{a0}{emoji}\u{a0}{count}\u{a0}"), style)
}

/// A piece of a link in built message lines: `width` cells at `col` of `line`.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkSpot {
    pub line: usize,
    pub col: u16,
    pub width: u16,
    pub url: String,
}

/// Where the links of the messages in `lines` (`owners`: whose each line is) are: the
/// underlined link pieces of each message, matched in order to the links its text and its
/// attachments' text hold (a wrapped link is several pieces).
pub fn link_spots(messages: &[Message], lines: &[Line], owners: &[Option<String>]) -> Vec<LinkSpot> {
    let link_fg = Some(super::theme::p().link);
    let letters = |s: &str| s.chars().filter(|c| !c.is_whitespace()).count();
    let mut out = Vec::new();
    let mut n = 0;
    while n < lines.len() {
        let Some(ts) = owners[n].as_deref() else {
            n += 1;
            continue;
        };
        let end = owners[n..].iter().position(|o| o.as_deref() != Some(ts)).map_or(lines.len(), |k| n + k);
        if let Some(m) = messages.iter().find(|m| m.ts == ts) {
            let mut links = text::links_shown(&format::message_blocks(&m.text, &m.blocks));
            for att in &m.attachments {
                if let Some(detail) = att["text"].as_str().filter(|d| !d.is_empty()) {
                    links.extend(text::links_shown(&format::from_mrkdwn(detail)));
                }
            }
            let mut links = links.into_iter().map(|(url, shown)| (url, letters(&shown))).peekable();
            'lines: for (k, line) in lines[n..end].iter().enumerate() {
                let mut col = 0u16;
                for span in &line.spans {
                    let width = unicode_width::UnicodeWidthStr::width(span.content.as_ref()) as u16;
                    if span.style.fg == link_fg && span.style.add_modifier.contains(Modifier::UNDERLINED) {
                        let Some((url, left)) = links.peek_mut() else { break 'lines };
                        out.push(LinkSpot { line: n + k, col, width, url: url.clone() });
                        *left = left.saturating_sub(letters(&span.content));
                        if *left == 0 {
                            links.next();
                        }
                    }
                    col += width;
                }
            }
        }
        n = end;
    }
    out
}

/// Make `width` cells at (`x`, `y`) a hyperlink to `url` (OSC 8, which terminals that know it
/// open on their own click — kitty: Ctrl+Shift+click); each cell carries the codes and keeps
/// its width. Only web and mail links without control characters.
pub fn hyperlink(buf: &mut ratatui::buffer::Buffer, x: u16, y: u16, width: u16, url: &str) {
    if !super::open::openable(url) || url.chars().any(char::is_control) {
        return;
    }
    let mut cx = x;
    while cx < x + width {
        let Some(cell) = buf.cell_mut((cx, y)) else { return };
        let symbol = cell.symbol().to_string();
        let w = unicode_width::UnicodeWidthStr::width(symbol.as_str()).max(1) as u16;
        if !symbol.contains('\x1b') {
            cell.set_symbol(&format!("\x1b]8;;{url}\x1b\\{symbol}\x1b]8;;\x1b\\"));
            cell.diff_option = ratatui::buffer::CellDiffOption::ForcedWidth(std::num::NonZeroU16::new(w).unwrap_or(std::num::NonZeroU16::MIN));
        }
        cx += w;
    }
}

/// What stands in the rows of a picture that could not be loaded.
fn failed_thumb(reason: &str) -> String {
    format!("⚠ preview unavailable: {reason}")
}

#[cfg(test)]
pub fn build_lines_owned(ws: &Workspace, t: &Timeline, width: usize, thread: bool, thumb_size: ThumbSize, avatars: Avatars) -> (Vec<Line<'static>>, Vec<Option<String>>, Vec<ImageSlot>) {
    build_lines_from(ws, t, 0, width, thread, thumb_size, avatars)
}

/// [`build_lines_owned`] from the `from`-th shown message on: the same lines as the end of the
/// whole build (grouping and day separators carry on from the message before it).
#[cfg(test)]
pub fn build_lines_from(ws: &Workspace, t: &Timeline, from: usize, width: usize, thread: bool, thumb_size: ThumbSize, avatars: Avatars) -> (Vec<Line<'static>>, Vec<Option<String>>, Vec<ImageSlot>) {
    build_lines_range(ws, t, from, usize::MAX, width, thread, thumb_size, avatars, None)
}

/// [`build_lines_from`] up to (not including) the `to`-th shown message.
#[allow(clippy::too_many_arguments)]
/// `break_before`: a message that starts an author group of its own (with its header), as the
/// first one under the "— new —" line does.
pub fn build_lines_range(ws: &Workspace, t: &Timeline, from: usize, to: usize, width: usize, thread: bool, thumb_size: ThumbSize, avatars: Avatars, break_before: Option<&str>) -> (Vec<Line<'static>>, Vec<Option<String>>, Vec<ImageSlot>) {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut owners: Vec<Option<String>> = Vec::new();
    let mut slots: Vec<ImageSlot> = Vec::new();
    let shown: Vec<&Message> = shown_messages(t, thread).collect();
    let before = from.checked_sub(1).and_then(|i| shown.get(i).copied());
    // What the whole build would carry over: a system message ends a group.
    let mut prev: Option<&Message> = before.filter(|p| !is_system(p));
    let mut last_day: Option<f64> = before.map(|p| p.time());
    for (n, m) in shown.into_iter().enumerate().skip(from).take(to.saturating_sub(from)) {
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
        let grouped = prev.is_some_and(|p| p.user == m.user && p.bot_id == m.bot_id && ts - p.time() < GROUP_WINDOW && !is_system(p)) && break_before != Some(m.ts.as_str()) && !m.is_ephemeral;
        // Two-row avatars stand beside the name and the first line; all text is indented past them.
        let large = avatars != Avatars::Off && ws.settings.messages.large_avatars;
        let indent = if large { super::media::AVATAR_SIZE_LARGE.0 as usize + 1 } else { 0 };
        // The colour of the initials' second row, beside the first line.
        let mut beside = None;
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
            if m.is_ephemeral {
                header.push(Span::styled("  only visible to you", Style::default().fg(super::theme::p().dim).add_modifier(Modifier::ITALIC)));
            }
            if avatars != Avatars::Off {
                // The initials always; a picture, once loaded, is drawn over them.
                let (label, size) = if large { (format!(" {} ", initials(&ws.author(m))), super::media::AVATAR_SIZE_LARGE) } else { (initials(&ws.author(m)), super::media::AVATAR_SIZE) };
                header.insert(0, Span::raw(" "));
                header.insert(0, Span::styled(label, Style::default().bg(color).fg(ratatui::style::Color::Black).add_modifier(Modifier::BOLD)));
                if let Some(file) = super::media::message_avatar(ws, m, large).filter(|_| avatars == Avatars::Pictures) {
                    slots.push(ImageSlot { line: out.len(), col: 0, file, size });
                }
                if large {
                    beside = Some(color);
                }
            }
            out.push(Line::from(header));
        }
        let base = out.len();
        let (mut body, body_slots) = message_body(ws, m, width.saturating_sub(indent), !thread, thumb_size);
        if beside.is_some() && body.is_empty() {
            body.push(Line::raw(""));
        }
        if indent > 0 {
            for (n, line) in body.iter_mut().enumerate() {
                let pad = match beside.filter(|_| n == 0) {
                    Some(color) => vec![Span::styled(" ".repeat(indent - 1), Style::default().bg(color)), Span::raw(" ")],
                    None => vec![Span::raw(" ".repeat(indent))],
                };
                line.spans.splice(0..0, pad);
            }
        }
        out.extend(body);
        slots.extend(body_slots.into_iter().map(|mut s| {
            s.line += base;
            s.col += indent as u16;
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
        let detail = att["text"].as_str().unwrap_or("");
        // `fallback` is a plain-text copy for clients that cannot show the rest: only when there is
        // nothing else (no title, no text).
        let title = att["title"].as_str().or_else(|| att["fallback"].as_str().filter(|_| detail.is_empty())).unwrap_or("");
        let bar = Span::styled("▍ ", Style::default().fg(attachment_color(att).unwrap_or(super::theme::p().quote_bar)));
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
            out.extend(snippet_preview(file, width, &OPEN_KEY.read().unwrap_or_else(|e| e.into_inner())));
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
    if let Some(app) = ws.sent_using(m) {
        out.push(Line::styled(app, Style::default().fg(super::theme::p().dim).add_modifier(Modifier::ITALIC)));
    }
    if !m.reactions.is_empty() {
        let mut spans = Vec::new();
        for r in &m.reactions {
            let mine = r.users.iter().any(|u| *u == ws.self_id);
            spans.push(reaction_pill(&text::emoji_cell(ws, &r.name, None), r.count, mine));
            spans.push(Span::raw(" "));
        }
        out.extend(text::wrap(spans, width, vec![], vec![]));
    }
    if summary && m.reply_count > 0 {
        let last = m.latest_reply.as_deref().and_then(|t| t.parse::<f64>().ok()).map(|t| format!(" · last {}", time_label(ws, t))).unwrap_or_default();
        let s = if m.reply_count == 1 { "reply" } else { "replies" };
        out.push(Line::styled(format!("💬 {} {s}{last}", m.reply_count), Style::default().fg(super::theme::p().mention).add_modifier(Modifier::ITALIC)));
    }
    // Custom emoji pictures over the cells kept for them (text, attachments, reactions).
    for (line, col, name) in text::emoji_cells(ws, &mut out) {
        if let Some(file) = super::media::emoji_file(ws, &name) {
            slots.push(ImageSlot { line, col, file, size: super::media::AVATAR_SIZE });
        }
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
        let rows = plain(&snippet_preview(&f, 20, "␣o"));
        assert_eq!(rows, vec![" fn main() {        ", "     let a_very_lo… ", " }                  ", " +6 more lines · ␣… "], "a shaded box, every row as wide");
        assert!(snippet_preview(&f, 20, "␣o").iter().all(|l| l.spans.iter().all(|s| s.style.bg == Some(crate::tui::theme::p().code_bg))));
        assert!(plain(&snippet_preview(&f, 40, "^O"))[3].contains("+6 more lines · ^O to view"), "the open key as bound");
        let short = File { mode: "snippet".into(), preview: Some("one".into()), lines: Some(1), ..Default::default() };
        assert_eq!(plain(&snippet_preview(&short, 20, "␣o")), vec![" one "]);
        // Real values for a 3-line file ending in a newline.
        let trailing = File { mode: "snippet".into(), preview: Some("a\nb\nc\n".into()), lines: Some(4), lines_more: Some(0), ..Default::default() };
        assert_eq!(plain(&snippet_preview(&trailing, 20, "␣o")).len(), 3);
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
    fn attachment_colours() {
        use ratatui::style::Color;
        let c = |v: serde_json::Value| super::attachment_color(&v);
        assert_eq!(c(serde_json::json!({"color": "#2eb886"})), Some(Color::Rgb(0x2e, 0xb8, 0x86)));
        assert_eq!(c(serde_json::json!({"color": "ff0000"})), Some(Color::Rgb(255, 0, 0)));
        assert_eq!(c(serde_json::json!({"color": "danger"})), Some(Color::Rgb(0xa3, 0x02, 0x00)));
        assert_eq!(c(serde_json::json!({"color": "#fff"})), None, "short forms are not Slack's");
        assert_eq!(c(serde_json::json!({"color": "nope"})), None);
        assert_eq!(c(serde_json::json!({"title": "x"})), None);
    }

    #[test]
    fn sent_using_an_app() {
        let mut ws = Workspace::default();
        // As in a real message sent through the Claude app, and its bot user (`users.list`).
        let mut bot = slack_core::types::User { id: "U0AENR65H46".into(), name: "claude".into(), real_name: Some("Claude".into()), is_bot: true, ..Default::default() };
        bot.profile.real_name = "Claude".into();
        bot.profile.api_app_id = Some("A08SF47R6P4".into());
        ws.users.insert(bot.id.clone(), bot);
        ws.users.insert("U2".into(), slack_core::types::User { id: "U2".into(), name: "yuriy".into(), ..Default::default() });
        let mut m: Message = serde_json::from_value(serde_json::json!({"app_id": "A08SF47R6P4", "team": "T1", "text": "a reply", "ts": "1.0", "type": "message", "user": "U2"})).unwrap();
        let rows = plain(&message_body(&ws, &m, 40, false, &|_, _| None).0);
        assert_eq!(rows, ["a reply", "Sent using @Claude"]);
        let lines = message_body(&ws, &m, 40, false, &|_, _| None).0;
        assert!(lines[1].style.add_modifier.contains(Modifier::ITALIC), "in italics");
        m.app_id = Some("A0OTHER".into());
        assert_eq!(ws.sent_using(&m).as_deref(), Some("Sent using an app"), "an app whose bot user is not known");
        m.bot_id = Some("B1".into());
        assert_eq!(ws.sent_using(&m), None, "an app's own (bot) message");
        m.bot_id = None;
        m.app_id = None;
        assert_eq!(ws.sent_using(&m), None, "sent by the person themselves");
    }

    #[test]
    fn links_are_found_where_they_are_drawn() {
        let ws = Workspace::default();
        let mut m = msg("1.0", "U2", "see <https://github.com/acme/api/pull/482|the retry fix PR> and <https://x.y/z>");
        m.attachments = vec![serde_json::json!({"text": "via <https://docs.acme.dev|the docs>"})];
        let (lines, _) = message_body(&ws, &m, 16, false, &|_, _| None);
        let owners = vec![Some("1.0".to_string()); lines.len()];
        let spots = link_spots(&[m], &lines, &owners);
        let pieces: Vec<(usize, u16, &str)> = spots.iter().map(|s| (s.line, s.col, s.url.as_str())).collect();
        assert_eq!(plain(&lines)[..3], ["see the retry ", "fix PR and ", "https://x.y/z"]);
        assert_eq!(
            pieces,
            [(0, 4, "https://github.com/acme/api/pull/482"), (0, 8, "https://github.com/acme/api/pull/482"), (1, 0, "https://github.com/acme/api/pull/482"), (1, 4, "https://github.com/acme/api/pull/482"), (2, 0, "https://x.y/z"), (3, 6, "https://docs.acme.dev"), (3, 10, "https://docs.acme.dev")],
            "a wrapped label is several pieces of one link; an attachment's links too"
        );
        // Each cell of a piece becomes a hyperlink that keeps its width.
        let mut buf = ratatui::buffer::Buffer::with_lines(["ab😀c"]);
        hyperlink(&mut buf, 0, 0, 5, "https://x.y/z");
        assert_eq!(buf[(0, 0)].symbol(), "\x1b]8;;https://x.y/z\x1b\\a\x1b]8;;\x1b\\");
        assert_eq!(buf[(2, 0)].diff_option, ratatui::buffer::CellDiffOption::ForcedWidth(std::num::NonZeroU16::new(2).unwrap()), "the wide one");
        assert!(buf[(4, 0)].symbol().contains('c'));
        let mut other = ratatui::buffer::Buffer::with_lines(["ab"]);
        hyperlink(&mut other, 0, 0, 2, "javascript:alert(1)");
        assert_eq!(other[(0, 0)].symbol(), "a", "only web and mail links");
    }

    #[test]
    fn custom_emoji_pictures_in_text_and_reactions() {
        let mut ws = Workspace::default();
        ws.custom_emoji.insert("piggy".into(), "https://emoji.slack-edge.com/T1/piggy/b77.png".into());
        ws.custom_emoji.insert("pig2x".into(), "alias:piggy".into());
        ws.custom_emoji.insert("yes".into(), "alias:white_check_mark".into());
        ws.number_emoji();
        let mut m = msg("1.0", "U2", "so :piggy: much :yes:");
        m.reactions = vec![Reaction { name: "pig2x".into(), users: vec![], count: 2 }];
        // Without pictures (half blocks, or turned off): names, as before.
        let (lines, slots) = message_body(&ws, &m, 40, false, &|_, _| None);
        assert_eq!(plain(&lines)[0], "so :piggy: much ✅");
        assert!(slots.is_empty());
        ws.emoji_pictures = true;
        let (lines, slots) = message_body(&ws, &m, 40, false, &|_, _| None);
        let rows = plain(&lines);
        assert_eq!(rows[0], "so    much ✅", "two blank cells for the picture; an alias of a standard emoji stays a character");
        let at: Vec<(usize, u16, (u16, u16), &str)> = slots.iter().map(|s| (s.line, s.col, s.size, s.file.id.as_str())).collect();
        assert_eq!(at, [(0, 3, (2, 1), "emoji:piggy:b77.png"), (1, 1, (2, 1), "emoji:pig2x:b77.png")], "in the text, and in the reaction pill (an alias of a picture)");
        assert_eq!(rows[1], "\u{a0}  \u{a0}2\u{a0} ");
        // Wrapped onto a new line, the picture goes with its cells.
        let mut long = msg("2.0", "U2", "aaaa bbbb :piggy:");
        long.reactions.clear();
        let (lines, slots) = message_body(&ws, &long, 10, false, &|_, _| None);
        assert_eq!(plain(&lines), ["aaaa bbbb ", "  "]);
        assert_eq!((slots[0].line, slots[0].col), (1, 0));
        // Plain text (notifications, search) never gets a placeholder.
        let plain_text = format::plain_text(&format::message_blocks(&m.text, &m.blocks), &|i| text::inline_plain(&ws, i));
        assert_eq!(plain_text, "so :piggy: much ✅");
    }

    #[test]
    fn reactions_are_pills_that_do_not_break() {
        let pill = super::reaction_pill("👍", 2, false);
        assert_eq!(pill.content, "\u{a0}👍\u{a0}2\u{a0}");
        assert!(pill.style.bg.is_some(), "shaded");
        assert!(super::reaction_pill("🎉", 1, true).style.add_modifier.contains(Modifier::BOLD), "yours stand out");
        // Many pills in a narrow pane wrap between pills, never inside one.
        let spans: Vec<Span<'static>> = (0..8).flat_map(|n| [super::reaction_pill("🔥", n, false), Span::raw(" ")]).collect();
        for line in text::wrap(spans, 20, vec![], vec![]) {
            let row: String = line.spans.iter().map(|s| s.content.to_string()).collect();
            assert!(!row.trim_start().starts_with('\u{a0}') || row.trim_start().starts_with("\u{a0}🔥"), "{row:?}");
            assert!(row.matches('🔥').count() == row.matches('\u{a0}').count() / 3, "whole pills only: {row:?}");
        }
    }

    #[test]
    fn failed_thumbnails_say_why() {
        // As `load_thumb` reports a corrupt upload (`{:#}` of its error).
        let e = "not a picture that can be shown: The image format could not be determined";
        assert_eq!(super::failed_thumb(super::super::media::short_reason(e)), "⚠ preview unavailable: not a picture that can be shown");
        assert_eq!(super::super::media::short_reason("no preview"), "no preview");
    }

    /// How long building the lines of a big conversation takes (it happens on every frame):
    /// `cargo test --release -p klack build_lines_cost -- --ignored --nocapture`.
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
                rows = build_lines_owned(&ws, &t, 100, false, &|_, _| None, Avatars::Off).0.len();
            }
            let whole = start.elapsed().as_secs_f64() * 1000.0 / runs as f64;
            // What a draw at the bottom builds now: the newest BUILD_CHUNK messages.
            let start = std::time::Instant::now();
            for _ in 0..runs {
                build_lines_from(&ws, &t, n - super::BUILD_CHUNK, 100, false, &|_, _| None, Avatars::Off);
            }
            let newest = start.elapsed().as_secs_f64() * 1000.0 / runs as f64;
            // A frame of the pane itself, at the bottom and far up (anchored).
            let media = Media::with(ratatui_image::picker::Picker::halfblocks());
            let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(104, 40)).unwrap();
            let mut pane = MessagePane::default();
            let mut frame = |pane: &mut MessagePane, runs: usize| {
                let start = std::time::Instant::now();
                for _ in 0..runs {
                    terminal.draw(|f| pane.draw(f, f.area(), &ws, "t", Some(&t), false, &media)).unwrap();
                }
                start.elapsed().as_secs_f64() * 1000.0 / runs as f64
            };
            let bottom = frame(&mut pane, runs);
            pane.scroll_up(rows * 3 / 4);
            // The jump itself builds the whole way once; the frames after it are anchored.
            let jump = frame(&mut pane, 1);
            let far = frame(&mut pane, runs);
            println!("{n} messages ({rows} rows): whole build {whole:.1} ms, newest {} {newest:.2} ms; pane frame at the bottom {bottom:.2} ms, jumping 3/4 up {jump:.1} ms, then {far:.2} ms", super::BUILD_CHUNK);
        }
    }

    #[test]
    fn anchored_view_matches_lines_from_the_bottom() {
        let mut ws = Workspace::default();
        for (id, name) in [("U1", "ann"), ("U2", "bob"), ("U3", "carol")] {
            ws.users.insert(id.into(), slack_core::types::User { id: id.into(), name: name.into(), ..Default::default() });
        }
        let texts = ["short", "a longer message that wraps over a few rows of this narrow pane, as chat does", "line one\nline two\nline three"];
        let mut t = Timeline { loaded: true, ..Default::default() };
        // 400 messages over several days, in groups and alone.
        for i in 0..400usize {
            let at = 1_700_000_000.0 + (i as f64) * 900.0 * if i % 7 == 0 { 30.0 } else { 1.0 };
            t.upsert(msg(&format!("{at:.6}"), ["U1", "U2", "U3"][(i / 3) % 3], texts[i % 3]));
        }
        let media = Media::with(ratatui_image::picker::Picker::halfblocks());
        let (w, h) = (60u16, 22u16);
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        let mut pane = MessagePane::default();
        let mut draw = |pane: &mut MessagePane, t: &Timeline| {
            terminal.draw(|f| pane.draw(f, f.area(), &ws, "t", Some(t), false, &media)).unwrap();
        };
        // The old way: everything built, the view counted in lines from the bottom (with the
        // "beginning of conversation" note on top).
        let width = (w - 2 - 1 - super::GUTTER) as usize;
        let mut whole: Vec<Option<String>> = vec![None];
        whole.extend(build_lines_owned(&ws, &t, width, false, &|_, _| None, Avatars::Off).1);
        let rows = (h - 2) as usize;
        let window = |s: usize| whole[whole.len() - s - rows..whole.len() - s].to_vec();
        draw(&mut pane, &t);
        assert_eq!(pane.rows, window(0));
        let mut s = 0usize;
        for (up, n) in [(true, 3), (true, 40), (true, 500), (false, 7), (true, 1), (true, 2000), (false, 900), (false, 1), (true, 5000), (false, 333), (false, 99999)] {
            if up {
                pane.scroll_up(n);
                s = (s + n).min(whole.len() - rows);
            } else {
                pane.scroll_down(n);
                s = s.saturating_sub(n);
            }
            draw(&mut pane, &t);
            assert_eq!(pane.rows, window(s), "after {} {n} (at {s} lines from the bottom)", if up { "up" } else { "down" });
            assert_eq!(pane.at_top(), s == whole.len() - rows, "at the top only there");
        }
        // Scrolled up, a new message leaves the view where it is; at the bottom it shows.
        pane.scroll_up(150);
        draw(&mut pane, &t);
        let before = pane.rows.clone();
        let mut t2 = t.clone();
        t2.upsert(msg("1800000000.000000", "U1", "brand new"));
        draw(&mut pane, &t2);
        assert_eq!(pane.rows, before, "the view stays on what you were reading");
        pane.to_bottom();
        draw(&mut pane, &t2);
        let mut t3 = t2.clone();
        t3.upsert(msg("1800000100.000000", "U2", "newer still"));
        draw(&mut pane, &t3);
        assert_eq!(pane.rows.last().cloned().flatten().as_deref(), Some("1800000100.000000"), "following the newest");
        // Selecting a message far above the view brings it into view.
        let far = t3.messages[20].ts.clone();
        pane.selected = Some(far.clone());
        terminal.draw(|f| pane.draw(f, f.area(), &ws, "t", Some(&t3), true, &media)).unwrap();
        assert!(pane.rows.contains(&Some(far)), "the selection is on screen");
    }

    #[test]
    fn a_message_taller_than_the_view_stays_put_when_selected() {
        let mut ws = Workspace::default();
        ws.users.insert("U1".into(), slack_core::types::User { id: "U1".into(), name: "ann".into(), ..Default::default() });
        let mut t = Timeline { loaded: true, ..Default::default() };
        t.upsert(msg("1700000000.000000", "U1", "before"));
        let long: Vec<String> = (1..=40).map(|n| format!("line {n}")).collect();
        t.upsert(msg("1700000060.000000", "U1", &long.join("\n")));
        t.upsert(msg("1700000120.000000", "U1", "after"));
        let media = Media::with(ratatui_image::picker::Picker::halfblocks());
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 22)).unwrap();
        let mut pane = MessagePane::default();
        // The text on screen.
        let mut draw = |pane: &mut MessagePane| {
            let buffer = terminal.draw(|f| pane.draw(f, f.area(), &ws, "t", Some(&t), true, &media)).unwrap().buffer.clone();
            (0..buffer.area.height).map(|y| (0..buffer.area.width).map(|x| buffer[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
        };
        draw(&mut pane);
        // Clicked (selected) while part of it shows: it shows from its start, and stays there
        // through redraws (the pointer moving over it redraws).
        let tall = "1700000060.000000".to_string();
        pane.selected = Some(tall.clone());
        let first = draw(&mut pane);
        assert!(first.contains("line 1 ") && !first.contains("line 40"), "its first line at the top:\n{first}");
        for n in 0..4 {
            pane.hover = if n % 2 == 0 { Some(tall.clone()) } else { None };
            assert_eq!(draw(&mut pane), first, "redraw {n} does not move the view");
        }
        // The wheel scrolls through it (and past it) while it stays selected.
        pane.scroll_down(5);
        let down = draw(&mut pane);
        assert_ne!(down, first, "scrolled");
        assert_eq!(draw(&mut pane), down, "and stays scrolled");
        pane.scroll_down(100);
        assert!(draw(&mut pane).contains("after"), "scrolled past it to the bottom");
        // Selecting another message brings that one into view.
        pane.selected = Some("1700000000.000000".into());
        assert!(draw(&mut pane).contains("before"));
    }

    #[test]
    fn a_menu_opens_at_the_message() {
        let mut pane = MessagePane { area: Rect::new(10, 5, 40, 12), ..Default::default() };
        pane.rows = vec![None, Some("1.0".into()), Some("2.0".into()), Some("2.0".into())];
        assert_eq!(pane.spot("2.0"), (11 + super::GUTTER + 2, 8), "its first row, past the gutter");
        assert_eq!(pane.spot("9.0"), (11 + 38 / 3, 6 + 10 / 3), "not on screen: inside the pane");
    }

    #[test]
    fn keeping_lines_in_view() {
        // 100 lines, a 10-line view at the bottom shows 90..100.
        assert_eq!(super::keep_in_view(100, 10, 0, 92, 94), 0, "already shown");
        assert_eq!(super::keep_in_view(100, 10, 0, 80, 82), 10, "above: to the top of the view");
        assert_eq!(super::keep_in_view(100, 10, 30, 80, 82), 17, "below: to the bottom of the view");
        assert_eq!(super::keep_in_view(100, 10, 0, 50, 95), 40, "taller, partly shown: from its start");
        assert_eq!(super::keep_in_view(100, 10, 20, 50, 95), 20, "taller, filling the view: left alone");
        assert_eq!(super::keep_in_view(100, 10, 40, 50, 95), 40, "taller, its start at the top: left alone");
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
        let whole = plain(&build_lines_owned(&ws, &t, 40, false, &|_, _| None, Avatars::Off).0);
        assert!(whole.iter().filter(|l| l.starts_with('─')).count() >= 2 && whole.iter().any(|l| l.contains("has joined")), "{whole:#?}");
        assert!(!whole.iter().any(|l| l.contains("a reply")), "replies stay in their thread");
        let shown = shown_messages(&t, false).count();
        assert_eq!(shown, 9);
        for from in 0..=shown {
            let (lines, owners, _) = build_lines_from(&ws, &t, from, 40, false, &|_, _| None, Avatars::Off);
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
        let (lines, _, slots) = build_lines_owned(&ws, &t, 40, false, &|_, _| None, Avatars::Pictures);
        let header = plain(&lines).into_iter().position(|l| l.contains("bob")).unwrap();
        assert!(plain(&lines)[header].starts_with("BO bob"), "the initials, under the picture: {:?}", plain(&lines)[header]);
        assert_eq!(slots.len(), 1, "grouped messages share one header and one picture");
        assert_eq!((slots[0].line, slots[0].col, slots[0].size, slots[0].file.id.as_str()), (header, 0, (2, 1), "avatar:U2:x_48.png"));
        let (off, _, none) = build_lines_owned(&ws, &t, 40, false, &|_, _| None, Avatars::Off);
        assert!(none.is_empty());
        assert!(plain(&off).iter().any(|l| l.starts_with("bob")), "turned off: nothing before the name");
        let (lines, _, none) = build_lines_owned(&ws, &t, 40, false, &|_, _| None, Avatars::Initials);
        assert!(none.is_empty() && plain(&lines).iter().any(|l| l.starts_with("BO bob")), "no graphics: the initials alone");
    }

    #[test]
    fn initials_of_names() {
        assert_eq!(super::initials("Lena Park"), "LP");
        assert_eq!(super::initials("deploybot"), "DE");
        assert_eq!(super::initials("ann"), "AN");
        assert_eq!(super::initials("X"), "X ", "padded to two cells");
        assert_eq!(super::initials("张伟"), "张", "one wide character fills both cells");
        assert_eq!(super::initials("  "), "? ");
        assert_eq!(super::initials("émile zola"), "ÉZ");
    }

    #[test]
    fn gutter_marks_message_starts_and_own_messages() {
        let ws = Workspace { self_id: "UME".into(), ..Default::default() };
        let mut t = Timeline { loaded: true, ..Default::default() };
        t.upsert(msg("1.0", "U2", "theirs"));
        t.upsert(msg("2.0", "U2", "theirs again"));
        t.upsert(msg("3.0", "UME", "mine\ntwo lines"));
        let (mut lines, owners, _) = build_lines_owned(&ws, &t, 40, false, &|_, _| None, Avatars::Off);
        let own = ["3.0"].into_iter().collect();
        let authors = [("1.0", "U2"), ("2.0", "U2"), ("3.0", "UME")].into_iter().collect();
        add_gutter(&mut lines, &owners, &own, &Default::default(), &authors);
        let rows = plain(&lines);
        assert!(rows.iter().all(|r| ["▌ ", "▎ ", "▏ ", "  "].iter().any(|g| r.starts_with(g))), "every row has the gutter: {rows:?}");
        let theme = crate::tui::theme::p();
        let (theirs, mine): (Vec<usize>, Vec<usize>) = (0..lines.len()).filter(|i| rows[*i].starts_with(['▌', '▎', '▏'])).partition(|i| lines[*i].spans[0].style.add_modifier.contains(Modifier::DIM));
        // U2's header, then the start of U2's grouped second message: a heavier start each, thin and dim in U2's name colour.
        let starts: Vec<&String> = theirs.iter().map(|i| &rows[*i]).filter(|r| r.starts_with('▎')).collect();
        assert_eq!(starts.len(), 2, "{rows:?}");
        assert!(starts[1].contains("theirs again"), "a grouped message still shows where it starts: {rows:?}");
        assert!(theirs.iter().all(|i| lines[*i].spans[0].style.fg == Some(text::user_color("U2"))), "their name's colour");
        assert_eq!(mine.len(), 3, "header and both lines of our message: {rows:?}");
        assert!(rows[mine[0]].starts_with('▌') && rows[mine[1]].starts_with('▎'));
        assert!(mine.iter().all(|i| lines[*i].spans[0].style.fg == Some(theme.own)), "ours in the own colour, not dim");
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
        let (lines, owners, _) = build_lines_owned(&ws, &t, 60, false, &|_, _| None, Avatars::Off);
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
        let (lines, _, slots) = build_lines_owned(&ws, &t, 40, false, &|f, _| (f.id == "F1").then_some((20, 3)), Avatars::Off);
        // day separator, header, "look", file line, 3 reserved rows
        assert_eq!(lines.len(), 7);
        assert_eq!(slots.len(), 1);
        assert_eq!((slots[0].line, slots[0].size), (4, (20, 3)));
        // A size that cannot fit (the sizer ignored the width): text only.
        let (narrow, _, none) = build_lines_owned(&ws, &t, 10, false, &|_, _| Some((20, 3)), Avatars::Off);
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
        let (lines, owners, _) = build_lines_owned(&ws, &t, 40, true, &|_, _| None, Avatars::Off);
        let p = plain(&lines);
        let seps: Vec<usize> = p.iter().enumerate().filter(|(_, l)| l.starts_with('─')).map(|(i, _)| i).collect();
        assert_eq!(seps.len(), 1, "one change of day among the replies: {p:?}");
        assert_eq!(owners[seps[0]], None);
        assert_eq!(p[seps[0] + 2], "two days later", "{p:?}");
        assert_eq!(p[1], "parent", "no separator above the parent: {p:?}");
        // A first reply on a later day: the separator comes after the replies rule.
        let t = Timeline { messages: vec![t.messages[0].clone(), t.messages[2].clone()], loaded: true, ..Default::default() };
        let p = plain(&build_lines_owned(&ws, &t, 40, true, &|_, _| None, Avatars::Off).0);
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
        let (lines, owners, _) = build_lines_owned(&ws, &t, 30, true, &|_, _| None, Avatars::Off);
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
        let lines = build_lines(&ws, &t, 60);
        let p = plain(&lines);
        assert!(!p.iter().any(|l| l == "reply"), "{p:?}");
        assert!(p.iter().any(|l| l.contains("👍\u{a0}1")), "the reaction (a pill)");
        let summary = p.iter().position(|l| l.starts_with("💬 2 replies")).expect("the summary");
        assert!(lines[summary].style.add_modifier.contains(Modifier::ITALIC), "in italics");
    }

    #[test]
    fn timestamps_compare_exactly() {
        assert!(ts_after("1792331652.105150", "1792331652.105149"), "one microsecond (beyond an f64 at this size)");
        assert!(!ts_after("1792331652.105149", "1792331652.105149"));
        assert!(ts_after("10.5", "10.49") && ts_after("11.0", "10.999999") && ts_after("1.0", "0000000000.000000"));
    }

    #[test]
    fn new_line_above_the_first_unread_and_newer_count() {
        let mut ws = Workspace { self_id: "UME".into(), ..Default::default() };
        for (id, name) in [("U1", "ann"), ("UME", "me")] {
            ws.users.insert(id.into(), slack_core::types::User { id: id.into(), name: name.into(), ..Default::default() });
        }
        let mut t = Timeline { loaded: true, ..Default::default() };
        let ts = |i: usize| format!("{}.000100", 1_700_000_000 + i * 60);
        for i in 0..30 {
            t.upsert(msg(&ts(i), if i == 21 { "UME" } else { "U1" }, &format!("message {i}")));
        }
        let media = Media::with(ratatui_image::picker::Picker::halfblocks());
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 14)).unwrap();
        let screen = |terminal: &ratatui::Terminal<ratatui::backend::TestBackend>| -> Vec<String> {
            let buf = terminal.backend().buffer();
            (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()).collect()
        };
        // Read up to message 20; 21 is our own, so the line stands above 22.
        let mut pane = MessagePane { new_since: Some(ts(20)), ..Default::default() };
        terminal.draw(|f| pane.draw(f, f.area(), &ws, "t", Some(&t), false, &media)).unwrap();
        let at = pane.rows.iter().position(|r| r.as_deref() == Some(ts(22).as_str())).expect("message 22 on screen");
        assert_eq!(pane.rows[at - 1], None);
        let rows = screen(&terminal);
        assert!(rows[at].contains(" new ") && rows[at].contains('─'), "{rows:#?}");
        assert_eq!(rows.iter().filter(|r| r.contains(" new ")).count(), 1);
        assert!(!rows[13].contains("newer"), "following the newest: no count");
        // Only our own messages after it, or a thread: no line.
        for mut pane in [MessagePane { new_since: Some(ts(20)), thread: true, ..Default::default() }, MessagePane { new_since: Some(ts(29)), ..Default::default() }] {
            terminal.draw(|f| pane.draw(f, f.area(), &ws, "t", Some(&t), false, &media)).unwrap();
            assert!(!screen(&terminal).iter().any(|r| r.contains(" new ")));
        }
        // Reading further up, the bottom border counts the messages below the view.
        pane.scroll_up(12);
        terminal.draw(|f| pane.draw(f, f.area(), &ws, "t", Some(&t), false, &media)).unwrap();
        let last = pane.rows.iter().flatten().last().cloned().unwrap();
        let below = 29 - (0..30).position(|i| ts(i) == last).unwrap();
        assert!(below > 0);
        assert!(screen(&terminal)[13].contains(&format!("↓ {below} newer")), "{:?}", screen(&terminal)[13]);
    }

    #[test]
    fn two_row_avatars_indent_the_text() {
        let mut ws = Workspace::default();
        ws.settings.messages.large_avatars = true;
        let mut bob = slack_core::types::User { id: "U2".into(), name: "bob".into(), ..Default::default() };
        bob.profile.image_48 = Some("https://avatars.slack-edge.com/x_48.png".into());
        bob.profile.image_192 = Some("https://avatars.slack-edge.com/x_192.png".into());
        ws.users.insert("U2".into(), bob);
        let mut t = Timeline { loaded: true, ..Default::default() };
        t.upsert(msg("1.0", "U2", "one\nmore"));
        let image = File { id: "F1".into(), name: "p.png".into(), mimetype: "image/png".into(), thumb_360: Some("https://x/t.png".into()), ..Default::default() };
        t.upsert(Message { files: vec![image], ..msg("2.0", "U2", "two") });
        let (lines, _, slots) = build_lines_owned(&ws, &t, 40, false, &|_, _| Some((10, 3)), Avatars::Pictures);
        let p = plain(&lines);
        let header = p.iter().position(|l| l.contains("bob")).unwrap();
        assert!(p[header].starts_with(" BO  bob"), "four-cell initials under the picture: {p:?}");
        assert_eq!(p[header + 1], "     one", "the picture's second row beside the first line");
        assert_eq!(lines[header + 1].spans[0].style.bg, Some(text::user_color("U2")), "the initials' second row");
        assert_eq!(p[header + 2], "     more");
        assert!(p[header + 3].starts_with("     two"), "grouped messages indented the same: {p:?}");
        assert_eq!((slots[0].line, slots[0].col, slots[0].size, slots[0].file.id.as_str()), (header, 0, (4, 2), "avatar:U2@192:x_192.png"));
        assert_eq!((slots[1].file.id.as_str(), slots[1].col), ("F1", 5), "thumbnails move with the text");
        // No text at all: a row is kept for the picture's second half.
        let mut t = Timeline { loaded: true, ..Default::default() };
        t.upsert(msg("1.0", "U2", ""));
        t.upsert(msg("900.0", "U2", "later"));
        let p = plain(&build_lines_owned(&ws, &t, 40, false, &|_, _| None, Avatars::Initials).0);
        let header = p.iter().position(|l| l.contains("bob")).unwrap();
        assert_eq!(p[header + 1].trim(), "", "{p:?}");
        assert!(!p[header + 2].contains("later"), "the next author group starts below it: {p:?}");
        // Pictures off: the setting does nothing.
        let p = plain(&build_lines_owned(&ws, &t, 40, false, &|_, _| None, Avatars::Off).0);
        assert!(p.iter().any(|l| l == "later"), "{p:?}");
    }

    #[test]
    fn new_line_repeats_the_header_and_comes_into_view() {
        let mut ws = Workspace { self_id: "UME".into(), ..Default::default() };
        ws.users.insert("U1".into(), slack_core::types::User { id: "U1".into(), name: "ann".into(), ..Default::default() });
        let mut t = Timeline { loaded: true, ..Default::default() };
        // One author, a minute apart: a single group without the line.
        let ts = |i: usize| format!("{}.000100", 1_700_000_000 + i * 60);
        for i in 0..40 {
            t.upsert(msg(&ts(i), "U1", &format!("message {i}")));
        }
        let media = Media::with(ratatui_image::picker::Picker::halfblocks());
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 14)).unwrap();
        let screen = |terminal: &ratatui::Terminal<ratatui::backend::TestBackend>| -> Vec<String> {
            let buf = terminal.backend().buffer();
            (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()).collect()
        };
        // Read up to 5: 34 unread, far more than a screen. Drawn first while the history loads.
        let mut pane = MessagePane { new_since: Some(ts(5)), jump_to_new: true, ..Default::default() };
        let loading = Timeline::default();
        terminal.draw(|f| pane.draw(f, f.area(), &ws, "t", Some(&loading), false, &media)).unwrap();
        assert!(pane.jump_to_new, "still to do once the history is in");
        terminal.draw(|f| pane.draw(f, f.area(), &ws, "t", Some(&t), false, &media)).unwrap();
        let rows = screen(&terminal);
        let line = rows.iter().position(|r| r.contains(" new ")).expect("the line is on screen after opening");
        assert!(line <= 3, "near the top: {rows:#?}");
        assert!(rows[line + 1].contains("ann"), "the author's name again under the line: {rows:#?}");
        assert!(rows[line + 2].contains("message 6"));
        assert!(!pane.jump_to_new, "only once");
        // Without the jump (or once done) the view stays at the newest.
        let mut pane = MessagePane { new_since: Some(ts(5)), ..Default::default() };
        terminal.draw(|f| pane.draw(f, f.area(), &ws, "t", Some(&t), false, &media)).unwrap();
        assert!(screen(&terminal)[12].contains("message 39"));
    }

    #[test]
    fn ephemeral_answers_say_so() {
        let mut ws = Workspace::default();
        ws.users.insert("USLACKBOT".into(), slack_core::types::User { id: "USLACKBOT".into(), name: "slackbot".into(), ..Default::default() });
        let mut t = Timeline { loaded: true, ..Default::default() };
        t.upsert(msg("1.0", "USLACKBOT", "a reminder"));
        t.upsert(Message { is_ephemeral: true, ..msg("2.0", "USLACKBOT", "Need some help with /remind?") });
        let p = plain(&build_lines_owned(&ws, &t, 60, false, &|_, _| None, Avatars::Off).0);
        let headers: Vec<&String> = p.iter().filter(|l| l.contains("slackbot")).collect();
        assert_eq!(headers.len(), 2, "its own header even right after the same author: {p:?}");
        assert!(headers[1].contains("only visible to you") && !headers[0].contains("only visible"));
    }

    #[test]
    fn attachments_show_their_fallback_only_when_there_is_nothing_else() {
        let ws = Workspace::default();
        // Slackbot's answer to `/remind help` (shortened): text and an identical fallback.
        let att = serde_json::json!({"callback_id": "x", "id": 1, "mrkdwn_in": ["text"], "text": "Use `/remind` to:\nSnooze a reminder", "fallback": "Use `/remind` to:\nSnooze a reminder"});
        let m = Message { attachments: vec![att], ..msg("1.0", "USLACKBOT", "Need some help?") };
        let p = plain(&message_body(&ws, &m, 60, false, &|_, _| None).0);
        assert_eq!(p.iter().filter(|l| l.contains("Snooze a reminder")).count(), 1, "once, not again from the fallback: {p:?}");
        assert!(!p.iter().any(|l| l.contains('`')), "rendered, not raw: {p:?}");
        let only_fallback = serde_json::json!({"fallback": "[a link preview]"});
        let m = Message { attachments: vec![only_fallback], ..msg("2.0", "U1", "") };
        assert!(plain(&message_body(&ws, &m, 60, false, &|_, _| None).0).iter().any(|l| l.contains("[a link preview]")));
    }
}

//! Message timeline rendering and scrolling.


use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use slack_core::format;
use slack_core::types::{File, Message};

use ratatui_image::sliced::{SignedPosition, SlicedImage};

use super::applink::{AppLink, Here};
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
pub const GUTTER: u16 = 2;

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

/// Counts the changes to what built lines show that live outside the workspace: the theme, the time zone, the open
/// key's name, the language (see [`Look`]). (Tests: each test's thread its own, so tests running alongside do not rebuild its lines.)
#[cfg(not(test))]
static LOOK_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[cfg(test)]
thread_local! {
    static LOOK_EPOCH: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The theme, the time zone, the open key or the language changed: lines built before are built again.
pub fn looks_changed() {
    #[cfg(not(test))]
    LOOK_EPOCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    #[cfg(test)]
    LOOK_EPOCH.with(|n| n.set(n.get() + 1));
}

fn look_epoch() -> u64 {
    #[cfg(not(test))]
    return LOOK_EPOCH.load(std::sync::atomic::Ordering::Relaxed);
    #[cfg(test)]
    LOOK_EPOCH.with(|n| n.get())
}

/// What message lines built for a view (the Activity view, the Threads feed) show besides the messages themselves:
/// the width; people, custom emoji and conversations (`Workspace::look`, how many are known); the settings; the
/// theme, the time zone and the open key ([`looks_changed`]); pictures or initials; the day; who wrote linked
/// messages and the profile's workspaces; and when the first status shown runs out. Lines built for one are built
/// again once it no longer [`holds`](Look::holds).
#[derive(Debug, Clone)]
pub struct Look {
    key: LookKey,
    settings: super::settings::Settings,
    /// The first time a status shown changes by itself (it expires, a huddle state runs out).
    until: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
struct LookKey {
    width: u16,
    epoch: u64,
    people: (u64, usize, usize, usize),
    emoji_pictures: bool,
    avatars: Avatars,
    today: chrono::NaiveDate,
    links: (usize, usize, usize, u64),
}

impl LookKey {
    fn of(here: &Here, width: u16, avatars: Avatars) -> LookKey {
        let ws = here.ws;
        LookKey {
            width,
            epoch: look_epoch(),
            people: (ws.look, ws.users.len(), ws.custom_emoji.len(), ws.conversations.len()),
            emoji_pictures: ws.emoji_pictures,
            avatars,
            today: super::clock::now().date_naive(),
            links: (here.workspaces.len(), here.names.len(), here.shown, here.authors.answered()),
        }
    }
}

impl Look {
    /// The look of lines built now `width` wide, showing the statuses of `people`.
    pub fn new<'a>(here: &Here, width: u16, avatars: Avatars, people: impl IntoIterator<Item = &'a str>) -> Look {
        let until = here.ws.next_status_change(people, chrono::Utc::now().timestamp());
        Look { key: LookKey::of(here, width, avatars), settings: here.ws.settings.clone(), until }
    }

    /// Whether lines built with this look still look so now, `width` wide.
    pub fn holds(&self, here: &Here, width: u16, avatars: Avatars) -> bool {
        self.key == LookKey::of(here, width, avatars) && self.settings == here.ws.settings && self.until.is_none_or(|t| chrono::Utc::now().timestamp() < t)
    }

    /// The width it was built for.
    pub fn width(&self) -> u16 {
        self.key.width
    }
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
    /// The selection was put there (a step back or forward, a link) and has not been on screen since, nor left: it is
    /// where you are even so.
    put: bool,
    /// The selection (or message being edited) last brought into view: it is brought into view
    /// once, when it changes, so the wheel can scroll away from it afterwards.
    shown: Option<String>,
    /// Message ts owning each visible row of the last draw (for clicks).
    rows: Vec<Option<String>>,
    /// Where links were drawn at the last draw (for clicks and the hover note): the address, for a link into the
    /// profile's workspaces where it goes, and whether its author is worth asking for.
    links: Vec<(Rect, String, Option<AppLink>, bool)>,
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
        (self.anchor, self.scroll, self.down, self.put) = (None, 0, 0, false);
    }

    /// True once the top of the loaded history is on screen.
    pub fn at_top(&self) -> bool {
        self.top
    }

    /// The selection as where you are in the pane (what the history keeps): none when it is the newest message
    /// (`newest`), or when the pane follows the newest and it was not on screen at the last draw.
    pub fn kept_selection(&self, newest: Option<&str>) -> Option<String> {
        let selected = self.selected.as_ref()?;
        let left = !self.put && self.anchor.is_none() && !self.rows.iter().flatten().any(|r| r == selected);
        (newest != Some(selected.as_str()) && !left).then(|| selected.clone())
    }

    /// Select message `ts`, brought into view at the next draw (even when it was the one last brought into view).
    pub fn select(&mut self, ts: Option<String>) {
        (self.selected, self.shown, self.put) = (ts, None, true);
    }

    /// Move the selection by `delta` messages among the shown ones (`order` = ts list).
    pub fn move_selection(&mut self, order: &[String], delta: isize) {
        self.put = false;
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

    /// The links to messages drawn now whose author is worth asking for (their message's preview does not name them).
    pub fn authors_wanted(&self) -> impl Iterator<Item = &AppLink> {
        self.links.iter().filter(|l| l.3).filter_map(|(_, _, app, _)| app.as_ref())
    }

    #[cfg(test)]
    pub fn app_links_drawn(&self) -> usize {
        let mut drawn: Vec<&AppLink> = self.links.iter().filter_map(|l| l.2.as_ref()).collect();
        drawn.dedup();
        drawn.len()
    }

    /// The pane is not drawn now (the thread or the huddle view in its place, or the thread closed): nothing of it is on
    /// screen for the mouse or the author lookups.
    pub fn hide(&mut self) {
        self.area = Rect::default();
        self.links.clear();
        self.hover = None;
    }

    /// The link into the profile's workspaces drawn at a terminal position.
    pub fn link_at(&self, column: u16, row: u16) -> Option<AppLink> {
        self.link_under(column, row).and_then(|(_, app)| app)
    }

    /// Any link drawn at a terminal position: its address and where it goes in klack (for a link into the profile's
    /// workspaces).
    pub fn link_under(&self, column: u16, row: u16) -> Option<(String, Option<AppLink>)> {
        // Only where the pane is drawn now (the thread or the huddle view may have taken its place since).
        if !self.area.contains((column, row).into()) {
            return None;
        }
        self.links.iter().find(|(at, ..)| at.contains((column, row).into())).map(|(_, url, app, _)| (url.clone(), app.clone()))
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

    pub fn draw(&mut self, f: &mut Frame, area: Rect, here: &Here, title: &str, timeline: Option<&Timeline>, focused: bool, media: &Media) {
        let ws = here.ws;
        self.area = area;
        self.links.clear();
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
                    let built = build_lines_range(here, t, from, end, width, self.thread, &thumb_size, avatars, first_new.as_deref());
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
                lines.insert(at, new_rule(width));
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
        if self.selected.as_ref().is_some_and(|s| self.rows.iter().flatten().any(|r| r == s)) {
            self.put = false;
        }
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
        // The selected message (and the one being edited) is highlighted from edge to edge, like a
        // list selection; the message under the pointer is shaded the same way.
        let marked = [self.selected.as_ref().filter(|_| focused), self.editing.as_ref()];
        fill_rows(f, inner, &self.rows, &marked, self.hover.as_ref(), &slots, start, pad, media);
        f.render_widget(Paragraph::new(visible).block(block), area);
        // Links the terminal can open on its own click.
        if let Some(t) = timeline {
            for spot in link_spots(here, &t.messages, &lines, &owners).into_iter().filter(|s| (start..end).contains(&s.line)) {
                let y = inner.y + (pad + spot.line - start) as u16;
                let x = inner.x + spot.col;
                let width = spot.width.min(inner.right().saturating_sub(x));
                hyperlink(f.buffer_mut(), x, y, width, &spot.url);
                self.links.push((Rect { x, y, width, height: 1 }, spot.url.clone(), spot.app, spot.wants_author));
            }
        }
        if let (Some(top), Some(bottom)) = (on_screen.first().and_then(index), on_screen.last().and_then(index)) {
            super::scroll::indicator(f, area, top, bottom + 1 - top, order.len(), border);
        }
        self.wanted = draw_pictures(f, inner, slots, start, pad, media);
    }
}

/// The "— new —" rule `width` wide, above the first unread message.
/// "──── label ────" across `width` columns (measured as drawn: wide characters count twice).
pub fn labelled_rule(label: &str, width: usize, style: Style) -> Line<'static> {
    let label = format!(" {label} ");
    let side = width.saturating_sub(unicode_width::UnicodeWidthStr::width(label.as_str()));
    Line::styled(format!("{}{label}{}", "─".repeat(side / 2), "─".repeat(side - side / 2)), style)
}

pub fn new_rule(width: usize) -> Line<'static> {
    labelled_rule("new", width, Style::default().fg(super::theme::p().danger))
}

/// Fill the rows of a message list drawn in `inner` (`rows`: the message owning each row) from edge to edge: those of
/// a `marked` message (the selection, the one being edited) in the selection colour, those of `hover`'s in the hover
/// shade (the selection wins); the text's own backgrounds draw over them. A picture over such a row keeps the pane's
/// background behind it (seen through its transparent parts). `slots` are placed as in [`draw_pictures`].
#[allow(clippy::too_many_arguments)]
pub fn fill_rows<T: PartialEq>(f: &mut Frame, inner: Rect, rows: &[Option<T>], marked: &[Option<&T>], hover: Option<&T>, slots: &[ImageSlot], start: usize, pad: usize, media: &Media) {
    let highlight = Style::default().bg(super::theme::p().selection);
    let shade = Style::default().bg(super::theme::p().hover);
    let is_marked = |owner: &Option<T>| owner.as_ref().is_some_and(|o| marked.contains(&Some(o)));
    let is_hovered = |owner: &Option<T>| owner.is_some() && owner.as_ref() == hover;
    for (n, owner) in rows.iter().enumerate() {
        if is_marked(owner) {
            f.buffer_mut().set_style(Rect { y: inner.y + n as u16, height: 1, ..inner }, highlight);
        } else if is_hovered(owner) {
            f.buffer_mut().set_style(Rect { y: inner.y + n as u16, height: 1, ..inner }, shade);
        }
    }
    let height = inner.height as isize;
    for slot in slots.iter().filter(|s| matches!(media.thumbs.get(&s.file.id), Some(Thumb::Ready(_)))) {
        let top = slot.line as isize - start as isize + pad as isize;
        let (from, to) = (top.max(0), (top + slot.size.1 as isize).min(height));
        if from >= to || !rows.get(from as usize).is_some_and(|o| is_marked(o) || is_hovered(o)) {
            continue;
        }
        let x = inner.x + GUTTER + slot.col;
        let width = slot.size.0.min(inner.right().saturating_sub(x));
        f.buffer_mut().set_style(Rect { x, y: inner.y + from as u16, width, height: (to - from) as u16 }, Style::default().bg(ratatui::style::Color::Reset));
    }
}

/// Draw the pictures of a message list drawn in `inner` from line `start` of its lines (after `pad` blank rows) over
/// their reserved rows, past the gutter, sliced when partly scrolled out; returns those not loaded yet (to ask for).
pub fn draw_pictures(f: &mut Frame, inner: Rect, slots: Vec<ImageSlot>, start: usize, pad: usize, media: &Media) -> Vec<(File, (u16, u16))> {
    let height = inner.height as isize;
    let mut wanted = Vec::new();
    for slot in slots {
        let top = slot.line as isize - start as isize + pad as isize;
        if top + slot.size.1 as isize <= 0 || top >= height {
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
            None => wanted.push((slot.file, slot.size)),
        }
    }
    wanted
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
    looks_changed();
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
pub fn add_gutter(lines: &mut [Line<'static>], owners: &[Option<String>], own: &std::collections::HashSet<&str>, system: &std::collections::HashSet<&str>, authors: &std::collections::HashMap<&str, &str>) {
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

pub fn day_label(ws: &Workspace, ts: f64) -> String {
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
pub fn reaction_pill(emoji: &str, count: u64, mine: bool) -> Span<'static> {
    let p = super::theme::p();
    if p.code_bg == ratatui::style::Color::Reset {
        let style = if mine { Style::default().fg(p.mention).add_modifier(Modifier::BOLD) } else { Style::default() };
        return Span::styled(format!("[{emoji}\u{a0}{count}]"), style);
    }
    let style = if mine { Style::default().bg(p.selection).fg(p.mention).add_modifier(Modifier::BOLD) } else { Style::default().bg(p.code_bg) };
    Span::styled(format!("\u{a0}{emoji}\u{a0}{count}\u{a0}"), style)
}

/// A piece of a link in built message lines: `width` cells at `col` of `line`; `app`: where it goes in klack, for a
/// link into the profile's workspaces.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkSpot {
    pub line: usize,
    pub col: u16,
    pub width: u16,
    pub url: String,
    pub app: Option<AppLink>,
    /// A link to a message whose author is worth asking for.
    pub wants_author: bool,
}

/// Where the links of the messages in `lines` (`owners`: whose each line is) are: the
/// underlined link pieces of each message (in the link colour, or the app-link colour for links into the profile's
/// workspaces), matched in order to the links its text and its attachments' text hold (a wrapped link is several
/// pieces).
pub fn link_spots(here: &Here, messages: &[Message], lines: &[Line], owners: &[Option<String>]) -> Vec<LinkSpot> {
    let link_fg = [Some(super::theme::p().link), Some(super::theme::p().app_link)];
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
            let here = here.of(m);
            let mut links = text::links_shown(&here, &format::message_blocks(&m.text, &m.blocks));
            for att in &m.attachments {
                if let Some(detail) = att["text"].as_str().filter(|d| !d.is_empty()) {
                    links.extend(text::links_shown(&here, &format::from_mrkdwn(detail)));
                }
            }
            let mut links = links.into_iter().map(|(url, shown, app)| (url, app, letters(&shown))).peekable();
            let wants = |app: &Option<AppLink>| app.as_ref().is_some_and(|a| super::applink::wants_author(a, &here));
            'lines: for (k, line) in lines[n..end].iter().enumerate() {
                let mut col = 0u16;
                for span in &line.spans {
                    let width = unicode_width::UnicodeWidthStr::width(span.content.as_ref()) as u16;
                    if link_fg.contains(&span.style.fg) && span.style.add_modifier.contains(Modifier::UNDERLINED) {
                        let Some((url, app, left)) = links.peek_mut() else { break 'lines };
                        out.push(LinkSpot { line: n + k, col, width, url: url.clone(), app: app.clone(), wants_author: wants(app) });
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
    build_lines_range(&Here::alone(ws), t, from, usize::MAX, width, thread, thumb_size, avatars, None)
}

/// [`build_lines_from`] up to (not including) the `to`-th shown message.
#[allow(clippy::too_many_arguments)]
/// `break_before`: a message that starts an author group of its own (with its header), as the
/// first one under the "— new —" line does.
pub fn build_lines_range(here: &Here, t: &Timeline, from: usize, to: usize, width: usize, thread: bool, thumb_size: ThumbSize, avatars: Avatars, break_before: Option<&str>) -> (Vec<Line<'static>>, Vec<Option<String>>, Vec<ImageSlot>) {
    let shown: Vec<&Message> = shown_messages(t, thread).collect();
    // A thread's "N replies" rule under its parent.
    let replies = thread.then(|| {
        let replies = t.messages.len().saturating_sub(1);
        let label = format!(" {replies} {} ", if replies == 1 { "reply" } else { "replies" });
        Line::styled(format!("{label}{}", "─".repeat(width.saturating_sub(label.chars().count()))), Style::default().fg(super::theme::p().dim))
    });
    build_shown(here, &shown, from, to, width, thread, replies, thumb_size, avatars, break_before)
}

/// [`build_lines_range`] of the messages `shown` (oldest first), as a conversation shows them; with `thread`, the
/// first is a thread's parent: no day separator above it, no reply summaries, and `under_parent` (the thread pane's
/// "N replies", the Threads feed's "N more replies") below it, which starts a new author group.
#[allow(clippy::too_many_arguments)]
pub fn build_shown(here: &Here, shown: &[&Message], from: usize, to: usize, width: usize, thread: bool, under_parent: Option<Line<'static>>, thumb_size: ThumbSize, avatars: Avatars, break_before: Option<&str>) -> (Vec<Line<'static>>, Vec<Option<String>>, Vec<ImageSlot>) {
    let ws = here.ws;
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut owners: Vec<Option<String>> = Vec::new();
    let mut slots: Vec<ImageSlot> = Vec::new();
    let before = from.checked_sub(1).and_then(|i| shown.get(i).copied());
    // What the whole build would carry over: a system message ends a group.
    let mut prev: Option<&Message> = before.filter(|p| !is_system(p));
    let mut last_day: Option<f64> = before.map(|p| p.time());
    // Statuses are checked against the clock on every build (an expired one is gone).
    let now = chrono::Utc::now().timestamp();
    for (n, &m) in shown.iter().enumerate().skip(from).take(to.saturating_sub(from)) {
        let ts = m.time();
        owners.resize(out.len(), None);
        if let Some(line) = under_parent.as_ref().filter(|_| thread && n == 1) {
            out.push(line.clone());
            owners.resize(out.len(), None);
            prev = None;
        }
        // A day separator where the day changes; in a thread not above the parent.
        if last_day.is_none_or(|d| !same_day(d, ts)) && !(thread && n == 0) {
            last_day = Some(ts);
            out.push(labelled_rule(&day_label(ws, ts), width, Style::default().fg(super::theme::p().dim)));
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
            let mut header = vec![Span::styled(ws.author(m), Style::default().fg(color).add_modifier(Modifier::BOLD))];
            // The person's status emoji (a custom one's picture goes over its cells, below).
            if let Some(status) = m.user.as_deref().and_then(|u| text::status_span(ws, u, now)) {
                header.push(Span::raw(format!(" {status}")));
            }
            header.push(Span::styled(format!("  {}", time_label(ws, ts)), Style::default().fg(super::theme::p().dim)));
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
            let mut header = [Line::from(header)];
            for (_, col, name) in text::emoji_cells(ws, &mut header) {
                if let Some(file) = super::media::emoji_file(ws, &name) {
                    slots.push(ImageSlot { line: out.len(), col, file, size: super::media::AVATAR_SIZE });
                }
            }
            out.extend(header);
        }
        let base = out.len();
        let (mut body, body_slots) = message_body(here, m, width.saturating_sub(indent), !thread, thumb_size);
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
pub fn message_body(here: &Here, m: &Message, width: usize, summary: bool, thumb_size: ThumbSize) -> (Vec<Line<'static>>, Vec<ImageSlot>) {
    let ws = here.ws;
    let here = here.of(m);
    let mut slots = Vec::new();
    let mut out = Vec::new();
    let blocks = format::message_blocks(&m.text, &m.blocks);
    let mut body = text::render_blocks(&here, &blocks, width);
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
            for line in text::render_blocks(&here, &doc, width.saturating_sub(2)) {
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
    #[test]
    fn a_labelled_rule_is_as_wide_as_asked_whatever_its_label() {
        let dim = ratatui::style::Style::default();
        for label in ["new", "日本語のラベル", "x"] {
            let line = super::labelled_rule(label, 30, dim);
            let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
            assert_eq!(unicode_width::UnicodeWidthStr::width(text.as_str()), 30, "{text:?}");
            assert!(text.starts_with('─') && text.ends_with('─') && text.contains(&format!(" {label} ")), "{text:?}");
        }
    }

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
        let (_, full) = message_body(&Here::alone(&ws), &m, 60, true, &sizes);
        assert_eq!((full[0].size, full[0].file.id.as_str()), ((32, 10), "F1"));
        // The thread pane at 30 columns: smaller, and cached under its own id.
        let (_, narrow) = message_body(&Here::alone(&ws), &m, 30, true, &sizes);
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
        let rows = plain(&message_body(&Here::alone(&ws), &m, 40, false, &|_, _| None).0);
        assert_eq!(rows, ["a reply", "Sent using @Claude"]);
        let lines = message_body(&Here::alone(&ws), &m, 40, false, &|_, _| None).0;
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
        let (lines, _) = message_body(&Here::alone(&ws), &m, 16, false, &|_, _| None);
        let owners = vec![Some("1.0".to_string()); lines.len()];
        let spots = link_spots(&Here::alone(&ws), &[m], &lines, &owners);
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
    fn an_in_app_link_is_found_where_it_is_drawn() {
        let mut ws = Workspace::default();
        ws.conversations.insert("C1".into(), slack_core::types::Conversation { id: "C1".into(), name: "deploys".into(), is_channel: true, ..Default::default() });
        let workspaces = vec![("https://acme.slack.com/".to_string(), "T1".to_string())];
        let names = vec!["acme".to_string()];
        let authors = crate::tui::applink::Authors::new();
        let here = Here { ws: &ws, workspaces: &workspaces, names: &names, shown: 0, authors: &authors, message: None };
        let url = "https://acme.slack.com/archives/C1/p1729440000123456";
        let m = msg("1.0", "U2", &format!("see <{url}|> and <https://x.y> then <{url}|the fix>"));
        let (lines, _) = message_body(&here, &m, 12, false, &|_, _| None);
        assert_eq!(plain(&lines), ["see ↪ ", "#deploys › ", "message and ", "https://x.y ", "then ↪ the ", "fix"], "the label wraps; the URL is not shown");
        let accent = lines[1].spans.iter().find(|s| s.content.contains("deploys")).unwrap().style;
        assert_eq!((accent.fg, accent.add_modifier.contains(Modifier::UNDERLINED)), (Some(crate::tui::theme::p().app_link), true), "in the app-link colour, underlined");
        let external = lines[3].spans[0].style;
        assert_eq!((external.fg, external.add_modifier.contains(Modifier::UNDERLINED)), (Some(crate::tui::theme::p().link), true), "an external link as before");
        let owners = vec![Some("1.0".to_string()); lines.len()];
        let spots = link_spots(&here, &[m], &lines, &owners);
        let app = crate::tui::applink::parse(url);
        let pieces: Vec<(usize, u16, &str, bool)> = spots.iter().map(|s| (s.line, s.col, s.url.as_str(), s.app == app)).collect();
        assert_eq!(
            pieces,
            [(0, 4, url, true), (1, 0, url, true), (1, 9, url, true), (2, 0, url, true), (3, 0, "https://x.y", false), (4, 5, url, true), (4, 7, url, true), (5, 0, url, true)],
            "every piece of the label carries the real URL and where it goes"
        );
        assert!(spots.iter().filter(|s| s.url != url).all(|s| s.app.is_none()), "an external link goes nowhere in klack");
        // Slack's blocks label a pasted link with its own address (`&` there, `&amp;` in the address): not the
        // author's words.
        let pasted = format!("{url}?thread_ts=1729440000.000100&cid=C1");
        let block = serde_json::json!([{"type": "rich_text", "elements": [{"type": "rich_text_section", "elements": [{"type": "link", "url": pasted.replace('&', "&amp;"), "text": pasted}]}]}]);
        let m = Message { blocks: serde_json::from_value(block).unwrap(), ..msg("2.0", "U2", "") };
        assert_eq!(plain(&message_body(&here, &m, 60, false, &|_, _| None).0), ["↪ #deploys › thread reply"]);
    }

    #[test]
    fn hovering_a_link_names_where_it_goes() {
        let mut ws = Workspace::default();
        ws.conversations.insert("C1".into(), slack_core::types::Conversation { id: "C1".into(), name: "backend".into(), is_channel: true, ..Default::default() });
        let url = "https://acme.slack.com/archives/C1/p1729440000123456?thread_ts=1729440000.000100&cid=C1";
        let mut t = Timeline { loaded: true, ..Default::default() };
        t.upsert(msg("1729440000.000500", "U2", &format!("see <{}|> or <https://x.y/a?b=2>", url.replace('&', "&amp;"))));
        let workspaces = vec![("https://acme.slack.com/".to_string(), "T1".to_string())];
        let names = vec!["acme".to_string()];
        let authors = crate::tui::applink::Authors::new();
        let here = Here { ws: &ws, workspaces: &workspaces, names: &names, shown: 0, authors: &authors, message: None };
        let media = Media::with(ratatui_image::picker::Picker::halfblocks());
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 8)).unwrap();
        let mut pane = MessagePane::default();
        terminal.draw(|f| pane.draw(f, f.area(), &here, "t", Some(&t), true, &media)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let cell = |what: &str| (0..buf.area.height).flat_map(|y| (0..buf.area.width).map(move |x| (x, y))).find(|&(x, y)| buf[(x, y)].symbol().contains(what)).unwrap_or_else(|| panic!("{what} is drawn"));
        let note = |c: (u16, u16)| pane.link_under(c.0, c.1).map(|(url, app)| crate::tui::applink::hover_note(&url, app.as_ref(), &here));
        let (x, y) = cell("↪");
        assert_eq!(note((x + 3, y)).as_deref(), Some("↪ opens #backend › thread reply in klack"), "an in-app link says where it goes");
        let (ex, ey) = cell("\x1b]8;;https://x.y/a?b=2\x1b\\");
        assert_eq!(note((ex, ey)).as_deref(), Some("x.y/a?b=2"), "an outside link its address, without https://");
        assert_eq!(note((x - 2, y)), None, "off the link: no note, so the status line is clear again");
        // The pointer stays where it is while the pane scrolls the link away: the next draw finds nothing there.
        let mut t = Timeline { loaded: true, ..Default::default() };
        for n in 0..14 {
            t.upsert(msg(&format!("1729440000.{:06}", n), "U2", "filler"));
        }
        t.upsert(msg("1729440000.000500", "U2", &format!("see <{}|>", url.replace('&', "&amp;"))));
        let mut pane = MessagePane::default();
        terminal.draw(|f| pane.draw(f, f.area(), &here, "t", Some(&t), true, &media)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let (x, y) = (0..buf.area.height).flat_map(|y| (0..buf.area.width).map(move |x| (x, y))).find(|&(x, y)| buf[(x, y)].symbol().contains("↪")).expect("the link is on screen at the bottom");
        assert!(pane.link_under(x + 3, y).is_some());
        pane.scroll_up(6);
        terminal.draw(|f| pane.draw(f, f.area(), &here, "t", Some(&t), true, &media)).unwrap();
        assert_eq!(pane.link_under(x + 3, y), None, "scrolled away: nothing under the pointer any more");
    }

    #[test]
    fn the_selection_fills_the_whole_row() {
        let ws = Workspace::default();
        let mut t = Timeline { loaded: true, ..Default::default() };
        t.upsert(msg("1729440000.000100", "U2", "hi"));
        t.upsert(msg("1729440000.000200", "U3", "```code```"));
        let (workspaces, names) = (vec![], vec![]);
        let authors = crate::tui::applink::Authors::new();
        let here = Here { ws: &ws, workspaces: &workspaces, names: &names, shown: 0, authors: &authors, message: None };
        let media = Media::with(ratatui_image::picker::Picker::halfblocks());
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 10)).unwrap();
        let p = crate::tui::theme::p();
        let rows_of = |pane: &MessagePane, ts: &str| (0..pane.rows.len()).filter(|&n| pane.rows[n].as_deref() == Some(ts)).map(|n| n as u16 + 1).collect::<Vec<u16>>();
        let mut pane = MessagePane { selected: Some("1729440000.000100".into()), ..Default::default() };
        terminal.draw(|f| pane.draw(f, f.area(), &here, "t", Some(&t), true, &media)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let (first, second) = (rows_of(&pane, "1729440000.000100"), rows_of(&pane, "1729440000.000200"));
        assert!(!first.is_empty() && !second.is_empty());
        for &y in &first {
            assert_eq!((buf[(1, y)].bg, buf[(38, y)].bg), (p.selection, p.selection), "row {y}: from the left inner edge to the right one");
            assert_ne!(buf[(39, y)].bg, p.selection, "row {y}: not the border");
        }
        for &y in &second {
            assert!((1..39).all(|x| buf[(x, y)].bg != p.selection), "row {y}: the other message is not highlighted");
        }
        // Selected, the code block keeps its own background over the selection.
        pane.selected = Some("1729440000.000200".into());
        terminal.draw(|f| pane.draw(f, f.area(), &here, "t", Some(&t), true, &media)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let (body, code) = rows_of(&pane, "1729440000.000200").into_iter().find_map(|y| (1..38).find(|&x| buf[(x, y)].symbol() == "c" && buf[(x + 1, y)].symbol() == "o").map(|x| (y, x))).expect("the code block is drawn");
        assert_eq!(buf[(code, body)].bg, p.code_bg);
        assert_eq!(buf[(38, body)].bg, p.selection);
        assert!(rows_of(&pane, "1729440000.000100").iter().all(|&y| (1..39).all(|x| buf[(x, y)].bg != p.selection)), "the first one is no longer highlighted");
        // Not focused: no selection at all.
        terminal.draw(|f| pane.draw(f, f.area(), &here, "t", Some(&t), false, &media)).unwrap();
        let buf = terminal.backend().buffer().clone();
        assert!((1..9).all(|y| (1..39).all(|x| buf[(x, y)].bg != p.selection)));
    }

    #[test]
    fn the_hover_fills_the_whole_row() {
        let ws = Workspace::default();
        let mut t = Timeline { loaded: true, ..Default::default() };
        t.upsert(msg("1729440000.000100", "U2", "hi"));
        t.upsert(msg("1729440000.000200", "U3", "there"));
        let (workspaces, names) = (vec![], vec![]);
        let authors = crate::tui::applink::Authors::new();
        let here = Here { ws: &ws, workspaces: &workspaces, names: &names, shown: 0, authors: &authors, message: None };
        let media = Media::with(ratatui_image::picker::Picker::halfblocks());
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 10)).unwrap();
        let p = crate::tui::theme::p();
        let rows_of = |pane: &MessagePane, ts: &str| (0..pane.rows.len()).filter(|&n| pane.rows[n].as_deref() == Some(ts)).map(|n| n as u16 + 1).collect::<Vec<u16>>();
        let mut pane = MessagePane { hover: Some("1729440000.000100".into()), ..Default::default() };
        terminal.draw(|f| pane.draw(f, f.area(), &here, "t", Some(&t), true, &media)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let (first, second) = (rows_of(&pane, "1729440000.000100"), rows_of(&pane, "1729440000.000200"));
        assert!(!first.is_empty() && !second.is_empty());
        for &y in &first {
            assert_eq!((buf[(1, y)].bg, buf[(38, y)].bg), (p.hover, p.hover), "row {y}: from the left inner edge to the right one");
            assert_ne!(buf[(39, y)].bg, p.hover, "row {y}: not the border");
        }
        for &y in &second {
            assert!((0..40).all(|x| buf[(x, y)].bg != p.hover), "row {y}: the other message is not shaded");
        }
        // Selected as well: the selection wins on every row.
        pane.selected = Some("1729440000.000100".into());
        terminal.draw(|f| pane.draw(f, f.area(), &here, "t", Some(&t), true, &media)).unwrap();
        let buf = terminal.backend().buffer().clone();
        for &y in &rows_of(&pane, "1729440000.000100") {
            assert_eq!((buf[(1, y)].bg, buf[(38, y)].bg), (p.selection, p.selection), "row {y}");
        }
    }

    #[test]
    fn a_selected_picture_keeps_the_pane_background() {
        let ws = Workspace::default();
        let mut t = Timeline { loaded: true, ..Default::default() };
        let image = File { id: "F1".into(), name: "p.png".into(), mimetype: "image/png".into(), thumb_360: Some("https://x/t.png".into()), original_w: Some(80), original_h: Some(40), ..Default::default() };
        t.upsert(Message { files: vec![image], ..msg("1729440000.000100", "U2", "look") });
        let (workspaces, names) = (vec![], vec![]);
        let authors = crate::tui::applink::Authors::new();
        let here = Here { ws: &ws, workspaces: &workspaces, names: &names, shown: 0, authors: &authors, message: None };
        // kitty's placeholder cells keep the background under them (half blocks paint their own).
        let mut picker = ratatui_image::picker::Picker::halfblocks();
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
        let mut media = Media::with(picker);
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 16)).unwrap();
        let mut pane = MessagePane { selected: Some("1729440000.000100".into()), ..Default::default() };
        terminal.draw(|f| pane.draw(f, f.area(), &here, "t", Some(&t), true, &media)).unwrap();
        let (file, size) = pane.wanted.first().cloned().expect("the picture is asked for");
        let font = media.picker.font_size();
        let img = image::DynamicImage::new_rgb8(size.0 as u32 * font.width as u32, size.1 as u32 * font.height as u32);
        let proto = ratatui_image::sliced::SlicedProtocol::new(&media.picker, img, Some(ratatui::layout::Size::new(size.0, size.1))).unwrap();
        media.thumbs.insert(file.id.clone(), Thumb::Ready(proto));
        terminal.draw(|f| pane.draw(f, f.area(), &here, "t", Some(&t), true, &media)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let p = crate::tui::theme::p();
        let rows: Vec<u16> = (0..pane.rows.len()).filter(|&n| pane.rows[n].is_some()).map(|n| n as u16 + 1).collect();
        let picture: Vec<u16> = rows.iter().copied().filter(|&y| buf[(1 + GUTTER, y)].symbol().contains('\u{10EEEE}')).collect();
        assert_eq!(picture.len(), size.1 as usize, "the picture is drawn");
        for &y in &rows {
            assert_eq!(buf[(38, y)].bg, p.selection, "row {y} is highlighted to the edge");
        }
        for &y in &picture {
            assert!((1 + GUTTER..1 + GUTTER + size.0).all(|x| buf[(x, y)].bg != p.selection), "row {y}: not behind the picture");
            assert_eq!(buf[(1, y)].bg, p.selection, "row {y}: the gutter is");
        }
    }

    #[test]
    fn a_click_on_a_link_finds_its_target() {
        let mut ws = Workspace::default();
        ws.conversations.insert("C1".into(), slack_core::types::Conversation { id: "C1".into(), name: "deploys".into(), is_channel: true, ..Default::default() });
        let url = "https://acme.slack.com/archives/C1/p1729440000123456?thread_ts=1729440000.000100&cid=C1";
        let mut t = Timeline { loaded: true, ..Default::default() };
        t.upsert(msg("1729440000.000500", "U2", &format!("see <{}|> or <https://x.y/?a=1&amp;b=2>", url.replace('&', "&amp;"))));
        let workspaces = vec![("https://acme.slack.com/".to_string(), "T1".to_string())];
        let names = vec!["acme".to_string()];
        let authors = crate::tui::applink::Authors::new();
        let here = Here { ws: &ws, workspaces: &workspaces, names: &names, shown: 0, authors: &authors, message: None };
        let media = Media::with(ratatui_image::picker::Picker::halfblocks());
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 8)).unwrap();
        let mut pane = MessagePane::default();
        terminal.draw(|f| pane.draw(f, f.area(), &here, "t", Some(&t), true, &media)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let cell = |what: &str| (0..buf.area.height).flat_map(|y| (0..buf.area.width).map(move |x| (x, y))).find(|&(x, y)| buf[(x, y)].symbol().contains(what)).unwrap_or_else(|| panic!("{what} is drawn"));
        let (x, y) = cell("↪");
        let link = crate::tui::applink::parse(url);
        assert!(link.is_some());
        assert_eq!(pane.link_at(x, y), link, "the arrow");
        assert_eq!(pane.link_at(x + 4, y), link, "the label");
        assert_eq!(pane.link_at(x - 2, y), None, "the text before it");
        let (ex, ey) = cell("\x1b]8;;https://x.y/?a=1&b=2\x1b\\");
        assert_eq!(pane.link_at(ex, ey), None, "an external link opens in the browser");
        assert_eq!(pane.link_at(0, y), None, "the border");
        // Not drawn this time (the thread or the huddle view in its place): nothing there to click.
        let drawn = pane.area;
        pane.area = Rect::default();
        assert_eq!(pane.link_at(x, y), None, "a link from an earlier draw");
        pane.area = drawn;
        // The URL under both is Slack's own, unescaped once.
        assert!(buf[(x, y)].symbol().contains(&format!("\x1b]8;;{url}\x1b\\")), "{:?}", buf[(x, y)].symbol());
        let lines = message_body(&here, &t.messages[0], 56, false, &|_, _| None).0;
        let spots = link_spots(&here, &t.messages, &lines, &vec![Some(t.messages[0].ts.clone()); lines.len()]);
        assert!(spots.iter().any(|s| s.url == url) && spots.iter().any(|s| s.url == "https://x.y/?a=1&b=2"), "{spots:?}");
        // Slack's blocks carry the address escaped as well.
        let block = serde_json::json!([{"type": "rich_text", "elements": [{"type": "rich_text_section", "elements": [{"type": "link", "url": "https://x.y/?a=1&amp;b=2"}]}]}]);
        let m = Message { blocks: serde_json::from_value(block).unwrap(), ..msg("2.0", "U2", "") };
        let lines = message_body(&here, &m, 56, false, &|_, _| None).0;
        let spots = link_spots(&here, std::slice::from_ref(&m), &lines, &vec![Some("2.0".to_string()); lines.len()]);
        assert_eq!(spots.iter().map(|s| s.url.as_str()).collect::<Vec<_>>(), ["https://x.y/?a=1&b=2"]);
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
        let (lines, slots) = message_body(&Here::alone(&ws), &m, 40, false, &|_, _| None);
        assert_eq!(plain(&lines)[0], "so :piggy: much ✅");
        assert!(slots.is_empty());
        ws.emoji_pictures = true;
        let (lines, slots) = message_body(&Here::alone(&ws), &m, 40, false, &|_, _| None);
        let rows = plain(&lines);
        assert_eq!(rows[0], "so    much ✅", "two blank cells for the picture; an alias of a standard emoji stays a character");
        let at: Vec<(usize, u16, (u16, u16), &str)> = slots.iter().map(|s| (s.line, s.col, s.size, s.file.id.as_str())).collect();
        assert_eq!(at, [(0, 3, (2, 1), "emoji:piggy:b77.png"), (1, 1, (2, 1), "emoji:pig2x:b77.png")], "in the text, and in the reaction pill (an alias of a picture)");
        assert_eq!(rows[1], "\u{a0}  \u{a0}2\u{a0} ");
        // Wrapped onto a new line, the picture goes with its cells.
        let mut long = msg("2.0", "U2", "aaaa bbbb :piggy:");
        long.reactions.clear();
        let (lines, slots) = message_body(&Here::alone(&ws), &long, 10, false, &|_, _| None);
        assert_eq!(plain(&lines), ["aaaa bbbb ", "  "]);
        assert_eq!((slots[0].line, slots[0].col), (1, 0));
        // Plain text (notifications, search) never gets a placeholder.
        let plain_text = format::plain_text(&format::message_blocks(&m.text, &m.blocks), &|i| text::inline_plain(&ws, i));
        assert_eq!(plain_text, "so :piggy: much ✅");
    }

    #[test]
    fn a_language_applied_again_builds_the_lines_again() {
        let ws = Workspace::default();
        let here = Here::alone(&ws);
        let look = super::Look::new(&here, 80, Avatars::Off, []);
        assert!(look.holds(&here, 80, Avatars::Off));
        // (Its file may have been edited: the words may differ.)
        super::super::i18n::set("en").unwrap();
        assert!(!look.holds(&here, 80, Avatars::Off));
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
                    terminal.draw(|f| pane.draw(f, f.area(), &Here::alone(&ws), "t", Some(&t), false, &media)).unwrap();
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
            terminal.draw(|f| pane.draw(f, f.area(), &Here::alone(&ws), "t", Some(t), false, &media)).unwrap();
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
        terminal.draw(|f| pane.draw(f, f.area(), &Here::alone(&ws), "t", Some(&t3), true, &media)).unwrap();
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
            let buffer = terminal.draw(|f| pane.draw(f, f.area(), &Here::alone(&ws), "t", Some(&t), true, &media)).unwrap().buffer.clone();
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
    fn author_line_shows_the_status_emoji() {
        let mut ws = Workspace::default();
        let mut bob = slack_core::types::User { id: "U2".into(), name: "bob".into(), ..Default::default() };
        (bob.profile.status_emoji, bob.profile.status_text) = (":palm_tree:".into(), "Vacationing".into());
        ws.users.insert("U2".into(), bob.clone());
        let mut t = Timeline { loaded: true, ..Default::default() };
        t.upsert(msg("1.0", "U2", "one"));
        let rows = plain(&build_lines_owned(&ws, &t, 40, false, &|_, _| None, Avatars::Off).0);
        let header = rows.iter().find(|l| l.starts_with("bob")).unwrap();
        assert_eq!(header, &format!("bob 🌴  {}", time_label(&ws, 1.0)), "name, the emoji, then the time");
        // A custom emoji status is a picture over two blank cells, after the initials and the name.
        ws.custom_emoji.insert("piggy".into(), "https://emoji.slack-edge.com/T1/piggy/b77.png".into());
        ws.number_emoji();
        ws.emoji_pictures = true;
        bob.profile.status_emoji = ":piggy:".into();
        ws.users.insert("U2".into(), bob);
        let (lines, _, slots) = build_lines_owned(&ws, &t, 40, false, &|_, _| None, Avatars::Initials);
        let rows = plain(&lines);
        let at = rows.iter().position(|l| l.starts_with("BO bob")).unwrap();
        assert!(rows[at].starts_with("BO bob     "), "{:?}", rows[at]);
        let pics: Vec<(usize, u16, &str)> = slots.iter().map(|s| (s.line, s.col, s.file.id.as_str())).collect();
        assert_eq!(pics, [(at, 7, "emoji:piggy:b77.png")]);
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
        assert_eq!(plain(&message_body(&Here::alone(&ws), &m, 30, true, &|_, _| None).0), vec!["exactly ten (edited)"]);
        assert_eq!(plain(&message_body(&Here::alone(&ws), &m, 12, true, &|_, _| None).0), vec!["exactly ten", "(edited)"]);
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
        terminal.draw(|f| pane.draw(f, f.area(), &Here::alone(&ws), "t", Some(&t), false, &media)).unwrap();
        let at = pane.rows.iter().position(|r| r.as_deref() == Some(ts(22).as_str())).expect("message 22 on screen");
        assert_eq!(pane.rows[at - 1], None);
        let rows = screen(&terminal);
        assert!(rows[at].contains(" new ") && rows[at].contains('─'), "{rows:#?}");
        assert_eq!(rows.iter().filter(|r| r.contains(" new ")).count(), 1);
        assert!(!rows[13].contains("newer"), "following the newest: no count");
        // Only our own messages after it, or a thread: no line.
        for mut pane in [MessagePane { new_since: Some(ts(20)), thread: true, ..Default::default() }, MessagePane { new_since: Some(ts(29)), ..Default::default() }] {
            terminal.draw(|f| pane.draw(f, f.area(), &Here::alone(&ws), "t", Some(&t), false, &media)).unwrap();
            assert!(!screen(&terminal).iter().any(|r| r.contains(" new ")));
        }
        // Reading further up, the bottom border counts the messages below the view.
        pane.scroll_up(12);
        terminal.draw(|f| pane.draw(f, f.area(), &Here::alone(&ws), "t", Some(&t), false, &media)).unwrap();
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
        terminal.draw(|f| pane.draw(f, f.area(), &Here::alone(&ws), "t", Some(&loading), false, &media)).unwrap();
        assert!(pane.jump_to_new, "still to do once the history is in");
        terminal.draw(|f| pane.draw(f, f.area(), &Here::alone(&ws), "t", Some(&t), false, &media)).unwrap();
        let rows = screen(&terminal);
        let line = rows.iter().position(|r| r.contains(" new ")).expect("the line is on screen after opening");
        assert!(line <= 3, "near the top: {rows:#?}");
        assert!(rows[line + 1].contains("ann"), "the author's name again under the line: {rows:#?}");
        assert!(rows[line + 2].contains("message 6"));
        assert!(!pane.jump_to_new, "only once");
        // Without the jump (or once done) the view stays at the newest.
        let mut pane = MessagePane { new_since: Some(ts(5)), ..Default::default() };
        terminal.draw(|f| pane.draw(f, f.area(), &Here::alone(&ws), "t", Some(&t), false, &media)).unwrap();
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
        let p = plain(&message_body(&Here::alone(&ws), &m, 60, false, &|_, _| None).0);
        assert_eq!(p.iter().filter(|l| l.contains("Snooze a reminder")).count(), 1, "once, not again from the fallback: {p:?}");
        assert!(!p.iter().any(|l| l.contains('`')), "rendered, not raw: {p:?}");
        let only_fallback = serde_json::json!({"fallback": "[a link preview]"});
        let m = Message { attachments: vec![only_fallback], ..msg("2.0", "U1", "") };
        assert!(plain(&message_body(&Here::alone(&ws), &m, 60, false, &|_, _| None).0).iter().any(|l| l.contains("[a link preview]")));
    }

    #[test]
    fn only_links_drawn_without_a_preview_want_their_author() {
        let mut ws = Workspace::default();
        ws.conversations.insert("C1".into(), slack_core::types::Conversation { id: "C1".into(), name: "deploys".into(), is_channel: true, ..Default::default() });
        let (bare, previewed) = ("https://acme.slack.com/archives/C1/p1729440000000100", "https://acme.slack.com/archives/C1/p1729440000000200");
        let mut t = Timeline { loaded: true, ..Default::default() };
        t.upsert(msg("1729440000.000500", "U2", &format!("see <{bare}>")));
        let mut m = msg("1729440000.000600", "U2", &format!("and <{previewed}>"));
        m.attachments = vec![serde_json::json!({"from_url": previewed, "author_name": "Bob", "is_msg_unfurl": true})];
        t.upsert(m);
        let workspaces = vec![("https://acme.slack.com/".to_string(), "T1".to_string())];
        let names = vec!["acme".to_string()];
        let authors = crate::tui::applink::Authors::new();
        let here = Here { ws: &ws, workspaces: &workspaces, names: &names, shown: 0, authors: &authors, message: None };
        let media = Media::with(ratatui_image::picker::Picker::halfblocks());
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(70, 12)).unwrap();
        let mut pane = MessagePane::default();
        terminal.draw(|f| pane.draw(f, f.area(), &here, "t", Some(&t), true, &media)).unwrap();
        assert_eq!(pane.app_links_drawn(), 2, "both drawn");
        let mut wanted: Vec<_> = pane.authors_wanted().cloned().collect();
        wanted.dedup();
        assert_eq!(wanted, [crate::tui::applink::parse(bare).unwrap()], "the previewed link names its author already");
        // Hidden (the thread or the huddle view in its place, or the thread closed): nothing of it is drawn any more.
        pane.hide();
        assert_eq!(pane.authors_wanted().count(), 0);
        assert_eq!(pane.app_links_drawn(), 0);
        assert_eq!(pane.area, Rect::default());
    }
}

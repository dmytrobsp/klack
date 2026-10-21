//! Left-hand conversation list: channels, then direct messages.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::state::Workspace;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// The rows atop the list that open the views in the main pane: Threads (Ctrl+T), then
    /// Activity (Ctrl+A).
    Threads,
    Activity,
    Header(String),
    Conversation(String),
    /// A blank row between sections.
    Gap,
}

#[derive(Debug, Default)]
pub struct Sidebar {
    pub items: Vec<Item>,
    /// Index into `items` (always a conversation when any exist).
    pub selected: usize,
    /// First visible row.
    pub offset: usize,
    /// Last drawn area, for mouse hit-testing.
    pub area: Rect,
    /// Item under the mouse pointer.
    pub hover: Option<usize>,
    /// Conversations with a draft kept (marked ✎).
    pub drafts: std::collections::HashSet<String>,
    /// Folded sections, by name: only their header shows, with what is unread in them.
    pub folded: std::collections::HashSet<String>,
    /// The conversations of each folded section.
    hidden: std::collections::HashMap<String, Vec<String>>,
    /// Where the folded sections are kept (one file per workspace account).
    fold_file: Option<std::path::PathBuf>,
    /// The conversation open last time (opened again at start), and where it is kept.
    pub remembered: Option<String>,
    /// The thread (parent ts) that was open in it.
    pub remembered_thread: Option<String>,
    open_file: Option<std::path::PathBuf>,
}

impl Sidebar {
    /// A sidebar for one workspace account, with the sections folded there last time.
    pub fn for_account(team: &str, user: &str) -> Self {
        Self::kept_in(slack_core::session::project_dirs().map(|d| d.data_local_dir().join("sidebar")), team, user)
    }

    /// A sidebar whose folds and last conversation are kept in folder `dir`.
    fn kept_in(dir: Option<std::path::PathBuf>, team: &str, user: &str) -> Self {
        let fold_file = dir.as_ref().map(|d| d.join(format!("{team}-{user}.json")));
        let open_file = dir.as_ref().map(|d| d.join(format!("{team}-{user}.open")));
        let folded = fold_file.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
        // One line: the conversation; an optional second: the thread open in it.
        let text = open_file.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        let remembered = lines.next().map(String::from);
        let remembered_thread = remembered.as_ref().and_then(|_| lines.next()).map(String::from);
        Sidebar { folded, fold_file, remembered, remembered_thread, open_file, ..Default::default() }
    }

    /// Keep conversation `id` as the one to open next time.
    pub fn remember(&mut self, id: &str) {
        if self.remembered.as_deref() == Some(id) {
            return;
        }
        self.remembered = Some(id.to_string());
        self.remembered_thread = None;
        self.save_open();
    }

    /// Keep `thread` (a parent ts) as the thread open in the remembered conversation, or none.
    pub fn remember_thread(&mut self, thread: Option<&str>) {
        if self.remembered.is_none() || self.remembered_thread.as_deref() == thread {
            return;
        }
        self.remembered_thread = thread.map(String::from);
        self.save_open();
    }

    fn save_open(&self) {
        let (Some(path), Some(id)) = (&self.open_file, &self.remembered) else { return };
        let text = match &self.remembered_thread {
            Some(t) => format!("{id}\n{t}"),
            None => id.clone(),
        };
        if let Err(e) = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(path, text)) {
            tracing::warn!("keeping the open conversation in {}: {e}", path.display());
        }
    }

    fn save_folds(&self) {
        let Some(path) = &self.fold_file else { return };
        let result = if self.folded.is_empty() {
            match std::fs::remove_file(path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
                _ => Ok(()),
            }
        } else {
            let mut names: Vec<&String> = self.folded.iter().collect();
            names.sort();
            path.parent().map_or(Ok(()), std::fs::create_dir_all).map_err(anyhow::Error::from).and_then(|_| Ok(std::fs::write(path, serde_json::to_string(&names)?)?))
        };
        if let Err(e) = result {
            tracing::warn!("saving folded sections to {}: {e}", path.display());
        }
    }

    /// Rebuild the item list from the workspace, keeping the selected conversation (or the
    /// header of the folded section it is in).
    pub fn rebuild(&mut self, ws: &Workspace) {
        let keep = self.items.get(self.selected).cloned();
        self.items.clear();
        self.hidden.clear();
        self.items.extend(VIEWS.iter().cloned());
        for (header, ids) in ws.sidebar_groups() {
            // Sections stand apart: a blank row before each (none in compact mode).
            if !self.items.is_empty() && !ws.settings.appearance.compact {
                self.items.push(Item::Gap);
            }
            self.items.push(Item::Header(header.clone()));
            if self.folded.contains(&header) {
                self.hidden.insert(header, ids);
            } else {
                self.items.extend(ids.into_iter().map(Item::Conversation));
            }
        }
        let folded_into = |id: &String| self.hidden.iter().find(|(_, ids)| ids.contains(id)).map(|(h, _)| Item::Header(h.clone()));
        let keep = match keep {
            Some(Item::Conversation(id)) if !self.items.contains(&Item::Conversation(id.clone())) => folded_into(&id),
            other => other,
        };
        // An unfolded section's header: its first conversation.
        let after = |k: &Item| self.items.iter().position(|i| i == k).map(|n| n + 1).filter(|n| matches!(self.items.get(*n), Some(Item::Conversation(_))));
        self.selected = keep
            .and_then(|k| self.items.iter().position(|i| *i == k && self.is_stop(i)).or_else(|| after(&k)))
            .or_else(|| self.items.iter().position(|i| matches!(i, Item::Conversation(_))))
            .or_else(|| self.items.iter().position(|i| self.is_stop(i)))
            .unwrap_or(0);
    }

    /// Where the selection can stop: a conversation, or a folded section's header.
    fn is_stop(&self, item: &Item) -> bool {
        match item {
            Item::Conversation(_) | Item::Threads | Item::Activity => true,
            Item::Header(h) => self.folded.contains(h),
            Item::Gap => false,
        }
    }

    /// The view's row that is selected (Threads, Activity), if one is.
    pub fn on_view(&self) -> Option<Item> {
        self.items.get(self.selected).filter(|i| VIEWS.contains(i)).cloned()
    }

    /// The view's row drawn at a terminal position, if one is.
    pub fn view_at(&self, column: u16, row: u16) -> Option<Item> {
        let inner = self.area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
        if !inner.contains((column, row).into()) {
            return None;
        }
        self.items.get(self.offset + (row - inner.y) as usize).filter(|i| VIEWS.contains(i)).cloned()
    }

    /// Select conversation `id`'s row, or the header of the folded section it is in (left
    /// folded).
    pub fn select_row_of(&mut self, id: &str) {
        let section = self.hidden.iter().find(|(_, ids)| ids.iter().any(|i| i == id)).map(|(h, _)| Item::Header(h.clone()));
        let row = Item::Conversation(id.to_string());
        if let Some(n) = self.items.iter().position(|i| *i == row || Some(i) == section.as_ref()) {
            self.selected = n;
        }
    }

    /// Select the row of view `item` (Threads, Activity).
    pub fn select_view(&mut self, item: &Item) {
        if let Some(n) = self.items.iter().position(|i| i == item) {
            self.selected = n;
        }
    }

    /// The folded section whose header is selected.
    pub fn selected_folded(&self) -> Option<&str> {
        match self.items.get(self.selected) {
            Some(Item::Header(h)) if self.folded.contains(h) => Some(h),
            _ => None,
        }
    }

    /// The section conversation `id` is listed in (not folded).
    fn section_of(&self, id: &str) -> Option<&str> {
        let mut section = None;
        for item in &self.items {
            match item {
                Item::Header(h) => section = Some(h.as_str()),
                Item::Conversation(c) if c == id => return section,
                _ => {}
            }
        }
        None
    }

    /// Fold or unfold section `name`; unfolding the selected one selects the conversation
    /// still shown (`current`) when it is in there, else the section's first.
    pub fn toggle_fold_keeping(&mut self, ws: &Workspace, name: &str, current: Option<&str>) {
        let unfolding_selected = self.selected_folded() == Some(name);
        self.toggle_fold(ws, name);
        if let Some(current) = current.filter(|c| unfolding_selected && self.section_of(c) == Some(name)) {
            self.select_id(ws, current);
        }
    }

    /// Fold the section of the selected conversation, or unfold the selected folded section.
    pub fn toggle_selected_fold(&mut self, ws: &Workspace, current: Option<&str>) {
        let section = self.items[..=self.selected.min(self.items.len().saturating_sub(1))].iter().rev().find_map(|i| match i {
            Item::Header(h) => Some(h.clone()),
            _ => None,
        });
        if let Some(name) = section {
            self.toggle_fold_keeping(ws, &name, current);
        }
    }

    /// Fold or unfold section `name` (saved for next time).
    pub fn toggle_fold(&mut self, ws: &Workspace, name: &str) {
        if !self.folded.remove(name) {
            self.folded.insert(name.to_string());
        }
        self.rebuild(ws);
        self.save_folds();
    }

    pub fn selected_id(&self) -> Option<&str> {
        match self.items.get(self.selected) {
            Some(Item::Conversation(id)) => Some(id),
            _ => None,
        }
    }

    /// Move the selection by `delta` rows that it can stop at (conversations and folded
    /// sections), skipping the other headers.
    pub fn move_by(&mut self, delta: isize) {
        self.step(delta, true);
    }

    /// Move the selection by `delta` conversations, past folded sections too.
    pub fn move_by_conversation(&mut self, delta: isize) {
        self.step(delta, false);
    }

    fn step(&mut self, delta: isize, folded_too: bool) {
        let convs: Vec<usize> = self.items.iter().enumerate().filter(|(n, i)| if folded_too { self.is_stop(i) } else { matches!(i, Item::Conversation(_)) || *n == self.selected }).map(|(n, _)| n).collect();
        if convs.is_empty() {
            return;
        }
        let pos = convs.iter().position(|&n| n == self.selected).unwrap_or(0) as isize;
        let next = (pos + delta).clamp(0, convs.len() as isize - 1) as usize;
        self.selected = convs[next];
    }

    /// Select conversation `id`, unfolding its section if it is folded.
    pub fn select_id(&mut self, ws: &Workspace, id: &str) {
        if let Some(name) = self.hidden.iter().find(|(_, ids)| ids.iter().any(|i| i == id)).map(|(h, _)| h.clone()) {
            self.toggle_fold(ws, &name);
        }
        if let Some(n) = self.items.iter().position(|i| *i == Item::Conversation(id.to_string())) {
            self.selected = n;
        }
    }

    /// The row the pointer is over, for the hover shade: a conversation or a view's row.
    pub fn hover_at(&self, column: u16, row: u16) -> Option<usize> {
        let inner = self.area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
        if !inner.contains((column, row).into()) {
            return None;
        }
        let n = self.offset + (row - inner.y) as usize;
        matches!(self.items.get(n), Some(Item::Conversation(_) | Item::Threads | Item::Activity)).then_some(n)
    }

    /// The conversation item drawn at a terminal position.
    pub fn item_at(&self, column: u16, row: u16) -> Option<usize> {
        let inner = self.area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
        if !inner.contains((column, row).into()) {
            return None;
        }
        let n = self.offset + (row - inner.y) as usize;
        matches!(self.items.get(n), Some(Item::Conversation(_))).then_some(n)
    }

    /// The hover note for item `n`: a DM's person and their status ("Name — 🌴 Vacationing").
    pub fn status_note(&self, ws: &Workspace, n: usize, now: i64) -> Option<String> {
        let Some(Item::Conversation(id)) = self.items.get(n) else { return None };
        let c = ws.conversations.get(id).filter(|c| c.is_im)?;
        let status = super::text::status_line(ws, c.user.as_deref()?, now)?;
        Some(format!("{} — {status}", ws.conversation_label(c)))
    }

    /// The section header drawn at a terminal position.
    pub fn header_at(&self, column: u16, row: u16) -> Option<String> {
        let inner = self.area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
        if !inner.contains((column, row).into()) {
            return None;
        }
        match self.items.get(self.offset + (row - inner.y) as usize) {
            Some(Item::Header(h)) => Some(h.clone()),
            _ => None,
        }
    }

    /// Select the conversation drawn at terminal row `row`; true if one was hit.
    pub fn click(&mut self, column: u16, row: u16) -> bool {
        match self.item_at(column, row) {
            Some(n) => {
                self.selected = n;
                true
            }
            None => false,
        }
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, ws: &Workspace, focused: bool) {
        self.area = area;
        let height = area.height.saturating_sub(2) as usize;
        if self.selected < self.offset {
            // With the section's header (and the gap above it) when it is the first.
            let above = self.items[..self.selected].iter().rev().take_while(|i| !matches!(i, Item::Conversation(_))).count();
            self.offset = self.selected.saturating_sub(above.max(1));
        } else if height > 0 && self.selected >= self.offset + height {
            self.offset = self.selected + 1 - height;
        }
        let width = area.width.saturating_sub(2) as usize;
        let lines: Vec<Line> = self
            .items
            .iter()
            .enumerate()
            .skip(self.offset)
            .take(height)
            .map(|(n, item)| match item {
                Item::Threads | Item::Activity => view_line(ws, item, n == self.selected, n != self.selected && self.hover == Some(n)),
                Item::Header(h) => match self.hidden.get(h) {
                    Some(ids) => folded_line(ws, &shown_name(ws, h), ids, n == self.selected, width),
                    None => header_line(&shown_name(ws, h), width),
                },
                Item::Gap => Line::raw(""),
                Item::Conversation(id) if n != self.selected && self.hover == Some(n) => conversation_line(ws, id, false, self.drafts.contains(id), width).style(Style::default().bg(super::theme::p().hover)),
                Item::Conversation(id) => conversation_line(ws, id, n == self.selected, self.drafts.contains(id), width),
            })
            .collect();
        let border = if focused { Style::default().fg(super::theme::p().accent) } else { Style::default().fg(super::theme::p().dim) };
        let title = format!(" {} ", ws.team_name);
        f.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::ALL).border_style(border).title(title)), area);
        super::scroll::indicator(f, area, self.offset, height.min(self.items.len().saturating_sub(self.offset)), self.items.len(), border);
    }
}

/// A section's name as shown: with its emoji, when it has one (custom sections).
fn shown_name(ws: &Workspace, name: &str) -> String {
    match ws.sections.iter().find(|s| s.kind == "standard" && s.name == name && !s.emoji.is_empty()) {
        Some(s) => format!("{} {name}", super::text::emoji(ws, &s.emoji, None)),
        None => name.to_string(),
    }
}

/// A section's header: its name, then a rule to the edge (" Channels ────────").
fn header_line(name: &str, width: usize) -> Line<'static> {
    let label = format!(" {name} ");
    let rule = width.saturating_sub(unicode_width::UnicodeWidthStr::width(label.as_str()));
    Line::from(vec![
        Span::styled(label, Style::default().fg(super::theme::p().accent).add_modifier(Modifier::BOLD)),
        Span::styled("─".repeat(rule), Style::default().fg(super::theme::p().dim)),
    ])
}

/// The views' rows, in the order shown atop the sidebar.
const VIEWS: [Item; 2] = [Item::Threads, Item::Activity];

/// A view's row: "💬 Threads", with how many replies you have not read and the mentions among
/// them; "🔔 Activity", with how many items of the feed are unread.
fn view_line(ws: &Workspace, item: &Item, selected: bool, hover: bool) -> Line<'static> {
    let p = super::theme::p();
    let t = &ws.thread_unread;
    let (icon, name, unread, count, mentions) = match item {
        Item::Activity => ("🔔", "Activity", ws.activity_unread > 0, ws.activity_unread as u64, 0),
        _ => ("💬", "Threads", t.has_unreads, if t.has_unreads { t.replies } else { 0 }, t.mentions),
    };
    let style = if unread { Style::default().add_modifier(Modifier::BOLD).fg(p.strong) } else { Style::default().fg(p.read) };
    let mut spans = vec![unread_mark(unread), Span::raw(icon), Span::styled(super::i18n::tr(name), style)];
    if count > 0 {
        spans.push(Span::styled(format!(" {count}"), Style::default().fg(p.strong).add_modifier(Modifier::BOLD)));
    }
    if mentions > 0 {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!(" {mentions} "), Style::default().fg(p.badge_fg).bg(p.badge_bg).add_modifier(Modifier::BOLD)));
    }
    let line = Line::from(spans);
    if selected { line.style(Style::default().bg(p.selection)) } else if hover { line.style(super::theme::hover_style()) } else { line }
}

/// The first column of a row: a bar in the accent colour when it has something you have not
/// seen (in the sidebar, Ctrl+T and the activity list alike).
pub fn unread_mark(unread: bool) -> Span<'static> {
    if unread { Span::styled("▌", Style::default().fg(super::theme::p().accent)) } else { Span::raw(" ") }
}

/// A folded section's header: "▸ name", the rule, then how many of its conversations have
/// unread messages and the mentions among them.
fn folded_line(ws: &Workspace, name: &str, ids: &[String], selected: bool, width: usize) -> Line<'static> {
    let p = super::theme::p();
    let unread: Vec<_> = ids.iter().filter(|id| !ws.muted.contains(*id)).filter_map(|id| ws.unread.get(id)).filter(|u| u.has_unreads || u.mentions > 0).collect();
    let mentions: u64 = unread.iter().map(|u| u.mentions).sum();
    let count = if unread.is_empty() { String::new() } else { format!(" {}", unread.len()) };
    let badge = if mentions > 0 { format!(" {mentions}") } else { String::new() };
    let label = format!("▸ {name} ");
    let used = 1 + unicode_width::UnicodeWidthStr::width(label.as_str()) + count.chars().count() + badge.chars().count();
    let mut spans = vec![
        unread_mark(!unread.is_empty()),
        Span::styled(label, Style::default().fg(p.accent).add_modifier(Modifier::BOLD)),
        Span::styled("─".repeat(width.saturating_sub(used)), Style::default().fg(p.dim)),
        Span::styled(count, Style::default().fg(p.strong).add_modifier(Modifier::BOLD)),
    ];
    if !badge.is_empty() {
        spans.push(Span::styled(badge, Style::default().fg(p.badge_fg).bg(p.badge_bg).add_modifier(Modifier::BOLD)));
    }
    let line = Line::from(spans);
    if selected { line.style(Style::default().bg(p.selection)) } else { line }
}

fn conversation_line(ws: &Workspace, id: &str, selected: bool, draft: bool, width: usize) -> Line<'static> {
    let Some(c) = ws.conversations.get(id) else { return Line::raw(id.to_string()) };
    let unread = ws.unread.get(id).cloned().unwrap_or_default();
    let muted = ws.muted.contains(id);
    let sigil = if c.is_im {
        let online = c.user.as_deref().and_then(|u| ws.presence.get(u)).copied().unwrap_or(false);
        Span::styled(if online { "● " } else { "○ " }, Style::default().fg(if online { super::theme::p().good } else { super::theme::p().dim }))
    } else if c.is_mpim {
        Span::styled("◎ ", Style::default().fg(super::theme::p().dim))
    } else if c.is_private {
        Span::styled("🔒", Style::default().fg(super::theme::p().dim))
    } else {
        Span::styled("# ", Style::default().fg(super::theme::p().dim))
    };
    let fresh = unread.has_unreads && !muted;
    let name_style = if fresh {
        Style::default().add_modifier(Modifier::BOLD).fg(super::theme::p().strong)
    } else if muted {
        Style::default().fg(super::theme::p().dim)
    } else {
        Style::default().fg(super::theme::p().read)
    };
    let badge = if unread.mentions > 0 { format!(" {}", unread.mentions) } else { String::new() };
    let huddle = ws.huddles.get(id).map(|h| format!(" 🎧{}", h.participants.len())).unwrap_or_default();
    let preview = if !c.is_dm() && !c.is_member {
        " ·preview"
    } else if c.is_im && c.user.as_ref().is_some_and(|u| ws.vip.contains(u)) {
        " VIP"
    } else {
        ""
    };
    let label = ws.conversation_label(c);
    let draft = if draft { " ✎" } else { "" };
    // A person's status emoji right after their name (re-checked against the clock every draw).
    let now = chrono::Utc::now().timestamp();
    let status_of = |u: &str| ws.status_of(u, now).map(|s| format!(" {}", super::text::emoji(ws, &s.emoji, None)));
    let status = c.user.as_deref().filter(|_| c.is_im).and_then(status_of).unwrap_or_default();
    let status_w = unicode_width::UnicodeWidthStr::width(status.as_str());
    let room = width.saturating_sub(3 + badge.chars().count() + huddle.chars().count() + preview.chars().count() + draft.chars().count() + status_w + 1);
    let label = if c.is_mpim { group_label(&ws.mpim_people(c), &status_of, room).unwrap_or(label) } else { label };
    let label: String = if label.chars().count() > room { label.chars().take(room.saturating_sub(1)).chain(['…']).collect() } else { label };
    let mut spans = vec![unread_mark(fresh), sigil, Span::styled(label, name_style)];
    if !status.is_empty() {
        spans.push(Span::raw(status));
    }
    if !draft.is_empty() {
        spans.push(Span::styled(draft, Style::default().fg(super::theme::p().dim)));
    }
    if !preview.is_empty() {
        spans.push(Span::styled(preview, Style::default().fg(super::theme::p().dim)));
    }
    if !huddle.is_empty() {
        spans.push(Span::styled(huddle, Style::default().fg(super::theme::p().own)));
    }
    if !badge.is_empty() {
        spans.push(Span::styled(badge, Style::default().fg(super::theme::p().badge_fg).bg(super::theme::p().badge_bg).add_modifier(Modifier::BOLD)));
    }
    let mut line = Line::from(spans);
    if selected {
        line = line.style(Style::default().bg(super::theme::p().selection));
    }
    line
}

/// A group DM's label with each person's status emoji after their name, as many as fit in
/// `room` (dropped from the last person on); None when none has one or none fits.
fn group_label(people: &[(String, Option<String>)], status_of: &dyn Fn(&str) -> Option<String>, room: usize) -> Option<String> {
    let emoji: Vec<String> = people.iter().map(|(_, id)| id.as_deref().and_then(status_of).unwrap_or_default()).collect();
    let with = emoji.iter().rposition(|e| !e.is_empty())?;
    (0..=with).rev().map(|upto| people.iter().zip(&emoji).enumerate().map(|(n, ((name, _), e))| if n <= upto { format!("{name}{e}") } else { name.clone() }).collect::<Vec<_>>().join(", ")).find(|l| unicode_width::UnicodeWidthStr::width(l.as_str()) <= room)
}

#[cfg(test)]
mod tests {
    use super::*;
    use slack_core::types::Conversation;

    fn ws() -> Workspace {
        let mut ws = Workspace::default();
        ws.settings.appearance.hide_inactive_days = 0;
        for (id, name, im) in [("C1", "general", false), ("C2", "random", false), ("D1", "", true)] {
            ws.conversations.insert(
                id.into(),
                Conversation { id: id.into(), name: name.into(), is_channel: !im, is_im: im, is_member: true, user: im.then(|| "U1".into()), ..Default::default() },
            );
        }
        ws
    }

    #[test]
    fn unread_rows_stand_out() {
        let mut w = ws();
        let p = crate::tui::theme::p();
        w.unread.insert("C1".into(), crate::tui::state::Unread { has_unreads: true, mentions: 0 });
        w.unread.insert("C2".into(), crate::tui::state::Unread { has_unreads: true, mentions: 0 });
        w.muted.insert("C2".into());
        let row = |id| conversation_line(&w, id, false, false, 30);
        let text = |l: &Line| l.spans.iter().map(|s| s.content.to_string()).collect::<String>();
        let (unread, muted, read) = (row("C1"), row("C2"), row("D1"));
        assert_eq!(text(&unread), "▌# general", "a bar in the first column");
        assert_eq!((unread.spans[0].style.fg, unread.spans[2].style.fg), (Some(p.accent), Some(p.strong)));
        assert!(unread.spans[2].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!((text(&muted).as_str(), muted.spans[2].style.fg), (" # random", Some(p.dim)), "muted: dim, no bar");
        assert_eq!((text(&read).chars().next(), read.spans[2].style.fg), (Some(' '), Some(p.read)), "read: a step back");
    }

    #[test]
    fn mute_toggles_and_dims() {
        let mut w = ws();
        let p = crate::tui::theme::p();
        w.unread.insert("C1".into(), crate::tui::state::Unread { has_unreads: true, mentions: 0 });
        let row = |w: &Workspace| conversation_line(w, "C1", false, false, 30);
        let before = w.total_unread().0;
        assert!(row(&w).spans[2].style.add_modifier.contains(Modifier::BOLD));
        assert!(w.toggle_muted("C1"), "now muted");
        assert_eq!((row(&w).spans[2].style.fg, w.total_unread().0), (Some(p.dim), before - 1), "dim, and left out of the count");
        assert!(!w.toggle_muted("C1"), "now unmuted");
        assert!(row(&w).spans[2].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(w.total_unread().0, before);
    }

    #[test]
    fn a_failed_mute_is_reverted_once() {
        let mut w = ws();
        // A mute that failed is put back.
        assert!(w.toggle_muted("C1"));
        assert!(w.revert_mute("C1", true) && !w.muted.contains("C1"));
        // Slack's own event already settled it: left alone.
        assert!(w.toggle_muted("C1"));
        w.muted.remove("C1");
        assert!(!w.revert_mute("C1", true) && !w.muted.contains("C1"));
        // An unmute that failed goes back to muted.
        w.muted.insert("C1".into());
        w.toggle_muted("C1");
        assert!(w.revert_mute("C1", false) && w.muted.contains("C1"));
    }

    #[test]
    fn the_last_conversation_is_kept() {
        let dir = std::env::temp_dir().join(format!("klack-last-{}", std::process::id()));
        let mut s = Sidebar::kept_in(Some(dir.clone()), "T1", "U1");
        assert_eq!(s.remembered, None, "nothing kept yet");
        s.remember("C2");
        assert_eq!(std::fs::read_to_string(dir.join("T1-U1.open")).unwrap(), "C2");
        assert_eq!(Sidebar::kept_in(Some(dir.clone()), "T1", "U1").remembered.as_deref(), Some("C2"), "the next start finds it");
        assert_eq!(Sidebar::kept_in(Some(dir.clone()), "T2", "U1").remembered, None, "per workspace account");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_open_thread_is_kept_with_it() {
        let dir = std::env::temp_dir().join(format!("klack-thread-{}", std::process::id()));
        let mut s = Sidebar::kept_in(Some(dir.clone()), "T1", "U1");
        s.remember_thread(Some("1.5"));
        assert_eq!(s.remembered_thread, None, "no conversation yet");
        s.remember("C2");
        s.remember_thread(Some("1.5"));
        let again = Sidebar::kept_in(Some(dir.clone()), "T1", "U1");
        assert_eq!((again.remembered.as_deref(), again.remembered_thread.as_deref()), (Some("C2"), Some("1.5")));
        s.remember_thread(None);
        assert_eq!(std::fs::read_to_string(dir.join("T1-U1.open")).unwrap(), "C2", "closing the thread forgets it");
        s.remember_thread(Some("2.5"));
        s.remember("C3");
        assert_eq!(Sidebar::kept_in(Some(dir.clone()), "T1", "U1").remembered_thread, None, "another conversation: no thread");
        // A file from before threads were kept still loads.
        std::fs::write(dir.join("T1-U1.open"), "C9\n").unwrap();
        let old = Sidebar::kept_in(Some(dir.clone()), "T1", "U1");
        assert_eq!((old.remembered.as_deref(), old.remembered_thread), (Some("C9"), None));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn draft_marker() {
        let w = ws();
        let text = |l: Line| l.spans.iter().map(|s| s.content.to_string()).collect::<String>();
        assert_eq!(text(conversation_line(&w, "C1", false, true, 30)), " # general ✎");
        assert_eq!(text(conversation_line(&w, "C1", false, false, 30)), " # general");
        // A long name makes room for the marker.
        let line = text(conversation_line(&w, "C2", false, true, 10));
        assert!(line.ends_with(" ✎") && line.contains('…'), "{line:?}");
    }

    #[test]
    fn dm_row_shows_the_status_emoji() {
        let mut w = ws();
        let text = |l: Line| l.spans.iter().map(|s| s.content.to_string()).collect::<String>();
        let mut bob = slack_core::types::User { id: "U1".into(), name: "bob".into(), ..Default::default() };
        (bob.profile.status_emoji, bob.profile.status_text) = (":palm_tree:".into(), "Vacationing".into());
        w.users.insert("U1".into(), bob.clone());
        assert_eq!(text(conversation_line(&w, "D1", false, false, 30)), " ○ bob 🌴", "the glyph right after the name");
        // A long name is cut so the emoji still fits.
        bob.profile.display_name = "Bartholomew Longname-Smithson".into();
        w.users.insert("U1".into(), bob);
        let line = text(conversation_line(&w, "D1", false, false, 20));
        assert!(line.ends_with("… 🌴") && unicode_width::UnicodeWidthStr::width(line.as_str()) <= 20, "{line:?}");
        // Channels have no status.
        assert_eq!(text(conversation_line(&w, "C1", false, false, 30)), " # general");
        // The status text is the hover note over the row.
        let mut s = Sidebar::default();
        s.rebuild(&w);
        let dm = s.items.iter().position(|i| *i == Item::Conversation("D1".into())).unwrap();
        let general = s.items.iter().position(|i| *i == Item::Conversation("C1".into())).unwrap();
        assert_eq!(s.status_note(&w, dm, 0).as_deref(), Some("Bartholomew Longname-Smithson — 🌴 Vacationing"));
        assert_eq!(s.status_note(&w, general, 0), None);
    }

    #[test]
    fn group_dm_row_shows_each_status_emoji() {
        let mut w = ws();
        w.self_id = "UME".into();
        w.conversations.insert("G1".into(), Conversation { id: "G1".into(), name: "mpdm-me--cy--dee-1".into(), is_mpim: true, is_member: true, ..Default::default() });
        for (id, name, emoji) in [("UME", "me", ""), ("U3", "cy", ":palm_tree:"), ("U4", "dee", ":house:")] {
            let mut u = slack_core::types::User { id: id.into(), name: name.into(), ..Default::default() };
            u.profile.status_emoji = emoji.into();
            w.users.insert(id.into(), u);
        }
        let text = |l: Line| l.spans.iter().map(|s| s.content.to_string()).collect::<String>();
        assert_eq!(text(conversation_line(&w, "G1", false, false, 30)), " ◎ cy 🌴, dee 🏠", "each person's emoji after their name");
        // Short of room: the emoji go from the end first, then the names are cut.
        assert_eq!(text(conversation_line(&w, "G1", false, false, 16)), " ◎ cy 🌴, dee");
        assert_eq!(text(conversation_line(&w, "G1", false, false, 12)), " ◎ cy, dee");
        let narrow = text(conversation_line(&w, "G1", false, false, 9));
        assert!(narrow.ends_with('…') && !narrow.contains('🌴'), "{narrow:?}");
    }

    #[test]
    fn navigation_skips_headers() {
        let mut s = Sidebar::default();
        s.rebuild(&ws());
        assert_eq!(s.selected_id(), Some("C1"));
        s.move_by(1);
        assert_eq!(s.selected_id(), Some("C2"));
        s.move_by(1);
        assert_eq!(s.selected_id(), Some("D1"));
        s.move_by(5);
        assert_eq!(s.selected_id(), Some("D1"));
        s.move_by(-10);
        assert_eq!(s.on_view(), Some(Item::Threads), "the Threads row on top");
        s.move_by(2);
        assert_eq!(s.selected_id(), Some("C1"), "past the Activity row");
    }

    #[test]
    fn sections_stand_apart() {
        let mut s = Sidebar::default();
        let mut w = ws();
        s.rebuild(&w);
        let kinds: Vec<String> = s.items.iter().map(|i| match i {
            Item::Threads => "{Threads}".into(),
            Item::Activity => "{Activity}".into(),
            Item::Header(h) => format!("[{h}]"),
            Item::Conversation(id) => id.clone(),
            Item::Gap => "".into(),
        }).collect();
        assert_eq!(kinds, ["{Threads}", "{Activity}", "", "[Channels]", "C1", "C2", "", "[Direct messages]", "D1"], "the views' rows, then a gap before every section");
        w.settings.appearance.compact = true;
        s.rebuild(&w);
        assert!(!s.items.contains(&Item::Gap), "compact: no gaps");
        let text = |l: Line| l.spans.iter().map(|s| s.content.to_string()).collect::<String>();
        assert_eq!(text(header_line("Channels", 20)), " Channels ──────────");
        assert_eq!(text(header_line("A very long section name", 10)), " A very long section name ", "no rule when there is no room");
        // Scrolling up to a section's first conversation shows its header and the gap above.
        s.rebuild(&ws());
        s.select_id(&ws(), "D1");
        s.offset = 9;
        let backend = ratatui::backend::TestBackend::new(30, 5);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| s.draw(f, f.area(), &ws(), false)).unwrap();
        assert_eq!(s.offset, 6);
    }

    #[test]
    fn the_threads_row() {
        let mut w = ws();
        let text = |l: &Line| l.spans.iter().map(|s| s.content.to_string()).collect::<String>();
        assert_eq!(text(&view_line(&w, &Item::Threads, false, false)), " 💬Threads", "nothing unread: no bar, no count");
        w.apply_counts(&serde_json::json!({"threads": {"has_unreads": true, "mention_count": 1, "unread_count_by_channel": {"C1": 2, "D1": 1}}}));
        let line = view_line(&w, &Item::Threads, false, false);
        assert_eq!(text(&line), "▌💬Threads 3  1 ", "the bar, three unread replies, one mention");
        assert!(line.spans[2].style.add_modifier.contains(Modifier::BOLD));
        // Live: read elsewhere.
        super::super::live::apply(&mut w, &serde_json::json!({"type": "update_global_thread_state", "has_unreads": false, "mention_count": 0, "unread_count_by_channel": {}}), None);
        assert_eq!(text(&view_line(&w, &Item::Threads, false, false)), " 💬Threads");
        // It is a stop for the selection, and keeps the conversation shown.
        let mut s = Sidebar::default();
        s.rebuild(&w);
        assert_eq!(s.selected_id(), Some("C1"), "the first conversation, not the Threads row");
        s.move_by(-2);
        assert!(s.on_view() == Some(Item::Threads) && s.selected_id().is_none());
    }

    #[test]
    fn views_on_top() {
        let mut w = ws();
        let mut s = Sidebar::default();
        s.rebuild(&w);
        assert_eq!(s.items[..3], [Item::Threads, Item::Activity, Item::Gap], "the views' rows on top, Threads first");
        w.settings.appearance.compact = true;
        s.rebuild(&w);
        assert_eq!(s.items[..3], [Item::Threads, Item::Activity, Item::Header("Channels".into())], "compact: no gap");
        // ↓ from Threads stops on Activity, then the first conversation.
        s.selected = 0;
        s.move_by(1);
        assert_eq!((s.on_view(), s.selected_id()), (Some(Item::Activity), None));
        s.move_by(1);
        assert_eq!((s.on_view(), s.selected_id()), (None, Some("C1")));
        s.move_by(-2);
        assert_eq!(s.on_view(), Some(Item::Threads));
        // A rebuild keeps a view's row selected.
        s.move_by(1);
        s.rebuild(&w);
        assert_eq!(s.on_view(), Some(Item::Activity));
        // Drawn: their rows map to the views, the pointer shades them.
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(30, 10)).unwrap();
        terminal.draw(|f| s.draw(f, f.area(), &w, false)).unwrap();
        assert_eq!((s.view_at(3, 1), s.view_at(3, 2), s.view_at(3, 3), s.view_at(0, 1)), (Some(Item::Threads), Some(Item::Activity), None, None));
        assert_eq!((s.hover_at(3, 2), s.hover_at(3, 3)), (Some(1), None), "hover reaches the Activity row, not a header");
        // The Activity row: a bar and the count while the feed has unread items.
        let text = |l: &Line| l.spans.iter().map(|s| s.content.to_string()).collect::<String>();
        assert_eq!(text(&view_line(&w, &Item::Activity, false, false)), " 🔔Activity", "nothing unread: no bar, no count");
        w.activity_unread = 4;
        let line = view_line(&w, &Item::Activity, false, false);
        assert_eq!(text(&line), "▌🔔Activity 4");
        assert_eq!((line.spans[0].style.fg, line.spans[2].style.add_modifier.contains(Modifier::BOLD)), (Some(crate::tui::theme::p().accent), true), "the same mark as Threads");
        assert_ne!(view_line(&w, &Item::Activity, false, true).style, line.style, "the hovered row is shaded");
        // A narrow sidebar (an 80-column terminal): the rows stay on one line each.
        let mut narrow = ratatui::Terminal::new(ratatui::backend::TestBackend::new(20, 6)).unwrap();
        narrow.draw(|f| s.draw(f, f.area(), &w, false)).unwrap();
        let buf = narrow.backend().buffer().clone();
        let row = |y: u16| (0..20).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>();
        assert!(row(1).contains("Threads") && row(2).contains("Activity 4"), "{:?} {:?}", row(1), row(2));
    }

    #[test]
    fn rebuild_keeps_selection() {
        let mut s = Sidebar::default();
        let w = ws();
        s.rebuild(&w);
        s.select_id(&w, "D1");
        s.rebuild(&w);
        assert_eq!(s.selected_id(), Some("D1"));
    }

    #[test]
    fn folded_sections() {
        let mut w = ws();
        w.unread.insert("C2".into(), crate::tui::state::Unread { has_unreads: true, mentions: 2 });
        let mut s = Sidebar::default();
        s.rebuild(&w);
        s.select_id(&w, "C2");
        s.toggle_selected_fold(&w, Some("C2"));
        assert_eq!(s.items, [Item::Threads, Item::Activity, Item::Gap, Item::Header("Channels".into()), Item::Gap, Item::Header("Direct messages".into()), Item::Conversation("D1".into())]);
        assert_eq!((s.selected_id(), s.selected_folded()), (None, Some("Channels")), "the selection moves onto the folded header");
        let text = |l: Line| l.spans.iter().map(|s| s.content.to_string()).collect::<String>();
        assert_eq!(text(folded_line(&w, "Channels", &["C1".into(), "C2".into()], false, 24)), "▌▸ Channels ──────── 1 2", "one unread conversation, two mentions");
        assert_eq!(text(folded_line(&ws(), "Channels", &["C1".into()], false, 16)), " ▸ Channels ────");
        // A folded section is a stop of its own; Alt+↑/↓ go past it.
        s.move_by(1);
        assert_eq!(s.selected_id(), Some("D1"));
        s.move_by(-1);
        assert_eq!(s.selected_folded(), Some("Channels"));
        s.move_by(1);
        s.move_by_conversation(-1);
        assert_eq!(s.selected_id(), Some("D1"), "no conversation above, only the folded section");
        s.rebuild(&w);
        assert_eq!(s.selected_id(), Some("D1"), "a rebuild keeps it folded and the selection");
        // Drawn: the header row is clickable.
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(30, 9)).unwrap();
        terminal.draw(|f| s.draw(f, f.area(), &w, false)).unwrap();
        assert_eq!((s.header_at(3, 4), s.header_at(3, 6), s.header_at(3, 7)), (Some("Channels".into()), Some("Direct messages".into()), None));
        assert!(s.view_at(3, 1) == Some(Item::Threads) && s.view_at(3, 3).is_none(), "and the Threads row");
        // The pointer over the Threads row shades it (the selection stays).
        assert_eq!((s.hover_at(3, 1), s.hover_at(3, 3)), (Some(0), None), "hover reaches the Threads row, not a gap");
        assert_ne!(view_line(&w, &Item::Threads, false, true).style, view_line(&w, &Item::Threads, false, false).style, "the hovered Threads row is shaded");
        assert_eq!(view_line(&w, &Item::Threads, true, true).style, view_line(&w, &Item::Threads, true, false).style, "the selection wins over the hover");
        // Unfolding the selected section: back on the conversation still shown in it, else
        // on its first.
        s.move_by(-1);
        s.toggle_selected_fold(&w, Some("C2"));
        assert_eq!(s.selected_id(), Some("C2"));
        s.toggle_selected_fold(&w, Some("C2"));
        s.toggle_selected_fold(&w, Some("D1"));
        assert_eq!(s.selected_id(), Some("C1"));
        s.toggle_selected_fold(&w, None);
        s.move_by(1);
        assert_eq!(s.selected_id(), Some("D1"));
        // Going to a conversation inside unfolds its section.
        s.select_id(&w, "C1");
        assert_eq!(s.selected_id(), Some("C1"));
        assert!(s.folded.is_empty() && s.items.contains(&Item::Conversation("C2".into())));
        // Folding the section of the selection's neighbour, then unfolding it by name.
        s.toggle_fold(&w, "Direct messages");
        assert_eq!(s.selected_id(), Some("C1"), "folding another section keeps the selection");
        s.toggle_fold(&w, "Direct messages");
        assert!(s.items.contains(&Item::Conversation("D1".into())));
    }

    #[test]
    fn section_emoji() {
        let mut w = ws();
        w.sections = vec![super::super::state::Section { id: "L1".into(), kind: "standard".into(), name: "Projects".into(), emoji: "rocket".into(), ..Default::default() }];
        assert_eq!(shown_name(&w, "Projects"), "🚀 Projects");
        assert_eq!(shown_name(&w, "Channels"), "Channels", "built-in sections have none");
        w.settings.messages.emoji_as_names = true;
        assert_eq!(shown_name(&w, "Projects"), ":rocket: Projects");
    }
}

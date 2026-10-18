//! Left-hand conversation list: channels, then direct messages.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::state::Workspace;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
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
}

impl Sidebar {
    /// A sidebar for one workspace account, with the sections folded there last time.
    pub fn for_account(team: &str, user: &str) -> Self {
        let fold_file = slack_core::session::project_dirs().map(|d| d.data_local_dir().join("sidebar").join(format!("{team}-{user}.json")));
        let folded = fold_file.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
        Sidebar { folded, fold_file, ..Default::default() }
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
            Item::Conversation(_) => true,
            Item::Header(h) => self.folded.contains(h),
            Item::Gap => false,
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

    /// The conversation item drawn at a terminal position.
    pub fn item_at(&self, column: u16, row: u16) -> Option<usize> {
        let inner = self.area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
        if !inner.contains((column, row).into()) {
            return None;
        }
        let n = self.offset + (row - inner.y) as usize;
        matches!(self.items.get(n), Some(Item::Conversation(_))).then_some(n)
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
                Item::Header(h) => match self.hidden.get(h) {
                    Some(ids) => folded_line(ws, h, ids, n == self.selected, width),
                    None => header_line(h, width),
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

/// A section's header: its name, then a rule to the edge (" Channels ────────").
fn header_line(name: &str, width: usize) -> Line<'static> {
    let label = format!(" {name} ");
    let rule = width.saturating_sub(unicode_width::UnicodeWidthStr::width(label.as_str()));
    Line::from(vec![
        Span::styled(label, Style::default().fg(super::theme::p().accent).add_modifier(Modifier::BOLD)),
        Span::styled("─".repeat(rule), Style::default().fg(super::theme::p().dim)),
    ])
}

/// A folded section's header: "▸ name", the rule, then how many of its conversations have
/// unread messages and the mentions among them.
fn folded_line(ws: &Workspace, name: &str, ids: &[String], selected: bool, width: usize) -> Line<'static> {
    let p = super::theme::p();
    let unread: Vec<_> = ids.iter().filter(|id| !ws.muted.contains(*id)).filter_map(|id| ws.unread.get(id)).filter(|u| u.has_unreads || u.mentions > 0).collect();
    let mentions: u64 = unread.iter().map(|u| u.mentions).sum();
    let count = if unread.is_empty() { String::new() } else { format!(" {}", unread.len()) };
    let badge = if mentions > 0 { format!(" {mentions}") } else { String::new() };
    let label = format!(" ▸ {name} ");
    let used = unicode_width::UnicodeWidthStr::width(label.as_str()) + count.chars().count() + badge.chars().count();
    let mut spans = vec![
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
    let mut name_style = Style::default();
    if unread.has_unreads && !muted {
        name_style = name_style.add_modifier(Modifier::BOLD).fg(super::theme::p().strong);
    } else if muted {
        name_style = name_style.fg(super::theme::p().dim);
    }
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
    let room = width.saturating_sub(3 + badge.chars().count() + huddle.chars().count() + preview.chars().count() + draft.chars().count() + 1);
    let label: String = if label.chars().count() > room { label.chars().take(room.saturating_sub(1)).chain(['…']).collect() } else { label };
    let mut spans = vec![Span::raw(" "), sigil, Span::styled(label, name_style)];
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

#[cfg(test)]
mod tests {
    use super::*;
    use slack_core::types::Conversation;

    fn ws() -> Workspace {
        let mut ws = Workspace::default();
        for (id, name, im) in [("C1", "general", false), ("C2", "random", false), ("D1", "", true)] {
            ws.conversations.insert(
                id.into(),
                Conversation { id: id.into(), name: name.into(), is_channel: !im, is_im: im, is_member: true, user: im.then(|| "U1".into()), ..Default::default() },
            );
        }
        ws
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
        assert_eq!(s.selected_id(), Some("C1"));
    }

    #[test]
    fn sections_stand_apart() {
        let mut s = Sidebar::default();
        let mut w = ws();
        s.rebuild(&w);
        let kinds: Vec<String> = s.items.iter().map(|i| match i {
            Item::Header(h) => format!("[{h}]"),
            Item::Conversation(id) => id.clone(),
            Item::Gap => "".into(),
        }).collect();
        assert_eq!(kinds, ["[Channels]", "C1", "C2", "", "[Direct messages]", "D1"], "a gap before every section but the first");
        w.settings.appearance.compact = true;
        s.rebuild(&w);
        assert!(!s.items.contains(&Item::Gap), "compact: no gaps");
        let text = |l: Line| l.spans.iter().map(|s| s.content.to_string()).collect::<String>();
        assert_eq!(text(header_line("Channels", 20)), " Channels ──────────");
        assert_eq!(text(header_line("A very long section name", 10)), " A very long section name ", "no rule when there is no room");
        // Scrolling up to a section's first conversation shows its header and the gap above.
        s.rebuild(&ws());
        s.select_id(&ws(), "D1");
        s.offset = 6;
        let backend = ratatui::backend::TestBackend::new(30, 5);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| s.draw(f, f.area(), &ws(), false)).unwrap();
        assert_eq!(s.offset, 3);
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
        assert_eq!(s.items, [Item::Header("Channels".into()), Item::Gap, Item::Header("Direct messages".into()), Item::Conversation("D1".into())]);
        assert_eq!((s.selected_id(), s.selected_folded()), (None, Some("Channels")), "the selection moves onto the folded header");
        let text = |l: Line| l.spans.iter().map(|s| s.content.to_string()).collect::<String>();
        assert_eq!(text(folded_line(&w, "Channels", &["C1".into(), "C2".into()], false, 24)), " ▸ Channels ──────── 1 2", "one unread conversation, two mentions");
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
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(30, 8)).unwrap();
        terminal.draw(|f| s.draw(f, f.area(), &w, false)).unwrap();
        assert_eq!((s.header_at(3, 1), s.header_at(3, 3), s.header_at(3, 4)), (Some("Channels".into()), Some("Direct messages".into()), None));
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
}

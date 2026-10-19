//! Conversation details overlay (F2): about, members, pinned messages.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Tabs, Wrap};
use slack_core::types::{Conversation, Message};

use super::state::Workspace;
use super::text;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    About,
    Members,
    Pins,
}

const TABS: [Tab; 3] = [Tab::About, Tab::Members, Tab::Pins];

pub struct Details {
    pub channel: String,
    pub tab: Tab,
    pub info: Option<Conversation>,
    pub members: Option<Vec<String>>,
    pub pins: Option<Vec<Message>>,
    pub selected: usize,
    /// Last drawn area, for clicks.
    pub area: Rect,
}

impl Details {
    pub fn new(channel: String) -> Details {
        Details { channel, tab: Tab::About, info: None, members: None, pins: None, selected: 0, area: Rect::default() }
    }

    pub fn switch_tab(&mut self, delta: isize) {
        let i = TABS.iter().position(|t| *t == self.tab).unwrap_or(0) as isize;
        self.tab = TABS[(i + delta).rem_euclid(TABS.len() as isize) as usize];
        self.selected = 0;
    }

    /// Rows in the current list tab.
    pub fn len(&self) -> usize {
        match self.tab {
            Tab::About => 0,
            Tab::Members => self.members.as_ref().map_or(0, Vec::len),
            Tab::Pins => self.pins.as_ref().map_or(0, Vec::len),
        }
    }

    pub fn move_by(&mut self, delta: isize) {
        let len = self.len();
        self.selected = if len == 0 { 0 } else { (self.selected as isize + delta).clamp(0, len as isize - 1) as usize };
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, ws: &Workspace) {
        let [popup] = Layout::horizontal([Constraint::Length(80.min(area.width))]).flex(Flex::Center).areas(area);
        let [popup] = Layout::vertical([Constraint::Length(26.min(area.height))]).flex(Flex::Center).areas(popup);
        self.area = popup;
        super::media::clear_modal(f, popup);
        let conv = self.info.as_ref().or_else(|| ws.conversations.get(&self.channel));
        let name = conv.map(|c| if c.is_dm() { ws.conversation_label(c) } else { format!("#{}", c.name) }).unwrap_or_else(|| self.channel.clone());
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(super::theme::p().accent))
            .title(format!(" {name} "));
        let inner = block.inner(popup);
        f.render_widget(block, popup);
        let [tabs, body] = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).areas(inner);
        let titles = ["About".to_string(), format!("Members{}", self.members.as_ref().map(|m| format!(" ({})", m.len())).unwrap_or_default()), format!("Pins{}", self.pins.as_ref().map(|p| format!(" ({})", p.len())).unwrap_or_default())];
        let index = TABS.iter().position(|t| *t == self.tab).unwrap_or(0);
        f.render_widget(Tabs::new(titles).select(index).highlight_style(Style::default().fg(super::theme::p().accent).add_modifier(Modifier::BOLD)), tabs);
        let lines = match self.tab {
            Tab::About => about(ws, conv),
            Tab::Members => self.list(body.height as usize, self.members.as_deref().map(|m| m.iter().map(|u| member_line(ws, u)).collect())),
            Tab::Pins => self.list(body.height as usize, self.pins.as_deref().map(|p| p.iter().map(|m| pin_line(ws, m, body.width as usize)).collect())),
        };
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
        if self.tab != Tab::About {
            let height = body.height as usize;
            super::scroll::indicator_beside(f, popup, body, self.selected.saturating_sub(height.saturating_sub(1)), height, self.len(), Style::default().fg(super::theme::p().accent));
        }
    }

    /// A scrolled list with the selected row highlighted, or a loading note.
    fn list(&self, height: usize, rows: Option<Vec<Line<'static>>>) -> Vec<Line<'static>> {
        let Some(rows) = rows else { return vec![Line::styled("Loading…", Style::default().fg(super::theme::p().dim))] };
        if rows.is_empty() {
            return vec![Line::styled("Nothing here.", Style::default().fg(super::theme::p().dim))];
        }
        let start = self.selected.saturating_sub(height.saturating_sub(1));
        rows.into_iter()
            .enumerate()
            .skip(start)
            .take(height)
            .map(|(n, l)| if n == self.selected { l.style(Style::default().bg(super::theme::p().selection)) } else { l })
            .collect()
    }
}

use super::profile::field;

fn about(ws: &Workspace, conv: Option<&Conversation>) -> Vec<Line<'static>> {
    let Some(c) = conv else { return vec![Line::styled("Loading…", Style::default().fg(super::theme::p().dim))] };
    let mut out = Vec::new();
    if let Some(u) = c.user.as_deref().and_then(|u| ws.users.get(u)) {
        out.push(field("Name", ws.person_label(&u.id)));
        out.extend(super::profile::person_lines(ws, u, chrono::Utc::now().timestamp()));
        return out;
    }
    let kind = if c.is_mpim { "group message" } else if c.is_private { "private channel" } else { "public channel" };
    out.push(field("Type", kind.into()));
    let plain = |s: &str| slack_core::format::plain_text(&slack_core::format::message_blocks(s, &[]), &|i| text::inline_plain(ws, i));
    let topic = plain(&c.topic.value);
    let purpose = plain(&c.purpose.value);
    out.push(field("Topic", if topic.is_empty() { "—".into() } else { topic }));
    out.push(field("Purpose", if purpose.is_empty() { "—".into() } else { purpose }));
    if let Some(created) = c.created.and_then(|t| super::clock::at(t as i64)) {
        out.push(field("Created", created.format("%B %e, %Y").to_string()));
    }
    if let Some(s) = ws.section_of(&c.id) {
        out.push(field("Section", s.name.clone()));
    }
    if ws.starred.contains(&c.id) {
        out.push(field("Starred", "yes".into()));
    }
    if ws.muted.contains(&c.id) {
        out.push(field("Muted", "yes".into()));
    }
    out
}

fn member_line(ws: &Workspace, id: &str) -> Line<'static> {
    let (dot, color) = match ws.presence.get(id) {
        Some(true) => ("●", super::theme::p().good),
        Some(false) => ("○", super::theme::p().dim),
        None => ("·", super::theme::p().dim),
    };
    let handle = ws.users.get(id).map(|u| format!("  @{}", u.name)).unwrap_or_default();
    Line::from(vec![Span::styled(format!(" {dot} "), Style::default().fg(color)), Span::raw(ws.person_label(id)), Span::styled(handle, Style::default().fg(super::theme::p().dim))])
}

fn pin_line(ws: &Workspace, m: &Message, width: usize) -> Line<'static> {
    let when = super::clock::at(m.time() as i64).map(|d| d.format("%b %e").to_string()).unwrap_or_default();
    let body = slack_core::format::plain_text(&slack_core::format::message_blocks(&m.text, &m.blocks), &|i| text::inline_plain(ws, i)).replace('\n', " ");
    let author = ws.author(m);
    let room = width.saturating_sub(author.chars().count() + when.chars().count() + 6);
    Line::from(vec![Span::styled(format!(" {author}"), Style::default().fg(text::user_color(m.user.as_deref().unwrap_or("")))), Span::styled(format!(" {when}  "), Style::default().fg(super::theme::p().dim)), Span::raw(body.chars().take(room).collect::<String>())])
}

/// Pinned messages of a `pins.list` response.
pub fn parse_pins(v: &serde_json::Value) -> Vec<Message> {
    v["items"].as_array().into_iter().flatten().filter_map(|i| serde_json::from_value(i["message"].clone()).ok()).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn pins_parsing() {
        let v = serde_json::json!({"items":[{"type":"message","channel":"C1","message":{"ts":"1.0","text":"pinned","user":"U1"}},{"type":"file","file":{}}]});
        let pins = super::parse_pins(&v);
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].text, "pinned");
    }

    use super::*;

    #[test]
    fn tabs_wrap_and_reset_selection() {
        let mut d = Details::new("C1".into());
        d.members = Some(vec!["U1".into(), "U2".into()]);
        d.switch_tab(1);
        assert_eq!(d.tab, Tab::Members);
        d.move_by(5);
        assert_eq!(d.selected, 1);
        d.switch_tab(-2);
        assert_eq!(d.tab, Tab::Pins);
        assert_eq!(d.selected, 0);
        assert_eq!(d.len(), 0);
    }
}

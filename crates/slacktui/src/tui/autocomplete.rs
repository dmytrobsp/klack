//! `@user`, `#channel` and `:emoji:` suggestions for the composer.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use super::composer::Trigger;
use super::picker::EmojiPicker;
use super::state::Workspace;
use super::text::DIM;

const MAX_ITEMS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    /// Shown in the list.
    pub label: String,
    /// Dim text after the label.
    pub detail: String,
    /// Inserted into the composer (with a trailing space).
    pub replacement: String,
    /// Slack id for mentions (`U…`, `C…`, `!here`).
    pub id: Option<String>,
}

/// An open suggestion list for one composer.
#[derive(Debug, Clone)]
pub struct Completion {
    /// Which composer: the thread reply box or the channel one.
    pub thread: bool,
    /// Char index where the token (with its sigil) starts.
    pub start: usize,
    pub items: Vec<Suggestion>,
    pub selected: usize,
}

impl Completion {
    pub fn move_by(&mut self, delta: isize) {
        let n = self.items.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(n.max(1)) as usize;
    }
}

/// Rank: 0 = prefix of the first field, 1 = prefix of a word in any field, 2 = substring.
pub fn rank(q: &str, fields: &[&str]) -> Option<u8> {
    let fields: Vec<String> = fields.iter().map(|f| f.to_lowercase()).collect();
    if fields.first().is_some_and(|f| f.starts_with(q)) {
        Some(0)
    } else if fields.iter().any(|f| f.split([' ', '.', '-', '_']).any(|w| w.starts_with(q))) {
        Some(1)
    } else if fields.iter().any(|f| f.contains(q)) {
        Some(2)
    } else {
        None
    }
}

pub fn suggestions(ws: &Workspace, trigger: &Trigger) -> Vec<Suggestion> {
    match trigger {
        Trigger::User(q) => {
            let q = q.to_lowercase();
            let mut out: Vec<(u8, Suggestion)> = ws
                .users
                .values()
                .filter(|u| !u.deleted)
                .filter_map(|u| {
                    let real = u.real_name.clone().unwrap_or_default();
                    let r = rank(&q, &[u.display(), &u.name, &real])?;
                    let detail = if u.name != u.display() { u.name.clone() } else { String::new() };
                    Some((r, Suggestion { label: ws.person_label(&u.id), detail, replacement: format!("@{}", u.display()), id: Some(u.id.clone()) }))
                })
                .collect();
            for (name, detail) in [("here", "notify active members"), ("channel", "notify everyone in the channel"), ("everyone", "notify the whole workspace")] {
                if name.starts_with(&q) {
                    out.push((0, Suggestion { label: format!("@{name}"), detail: detail.into(), replacement: format!("@{name}"), id: Some(format!("!{name}")) }));
                }
            }
            out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.label.to_lowercase().cmp(&b.1.label.to_lowercase())));
            out.into_iter().map(|(_, s)| s).take(MAX_ITEMS).collect()
        }
        Trigger::Channel(q) => {
            let q = q.to_lowercase();
            let mut out: Vec<(u8, Suggestion)> = ws
                .conversations
                .values()
                .filter(|c| !c.is_dm() && !c.is_archived)
                .filter_map(|c| {
                    let r = rank(&q, &[&c.name])?;
                    let detail = if c.is_private { "private".into() } else if !c.is_member { "not a member".into() } else { String::new() };
                    Some((r, Suggestion { label: format!("#{}", c.name), detail, replacement: format!("#{}", c.name), id: Some(c.id.clone()) }))
                })
                .collect();
            out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.label.cmp(&b.1.label)));
            out.into_iter().map(|(_, s)| s).take(MAX_ITEMS).collect()
        }
        Trigger::Emoji(q) => {
            let custom: Vec<String> = ws.custom_emoji.keys().cloned().collect();
            let picker = EmojiPicker { query: q.clone(), selected: 0 };
            picker
                .candidates(&custom)
                .into_iter()
                .take(MAX_ITEMS)
                .map(|c| Suggestion { label: format!("{} :{}:", c.glyph, c.name), detail: String::new(), replacement: format!(":{}:", c.name), id: None })
                .collect()
        }
    }
}

/// Draw the list just above `anchor` (the composer box).
pub fn draw(f: &mut Frame, anchor: Rect, c: &Completion) {
    let height = c.items.len() as u16 + 2;
    let width = anchor.width.min(48);
    let y = anchor.y.saturating_sub(height);
    let area = Rect { x: anchor.x, y, width, height: height.min(anchor.y) };
    f.render_widget(Clear, area);
    let block = Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::Rgb(0x61, 0xaf, 0xef))).title(" Tab/Enter insert · Esc close ");
    let lines: Vec<Line> = c
        .items
        .iter()
        .enumerate()
        .map(|(n, s)| {
            let line = Line::from(vec![Span::raw(format!(" {}", s.label)), Span::styled(format!("  {}", s.detail), Style::default().fg(DIM))]);
            if n == c.selected { line.style(Style::default().bg(Color::Rgb(0x2c, 0x3e, 0x5c)).add_modifier(Modifier::BOLD)) } else { line }
        })
        .collect();
    f.render_widget(Paragraph::new(lines).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use slack_core::types::{Conversation, User};

    fn ws() -> Workspace {
        let mut w = Workspace::default();
        for (id, name, display) in [("U1", "anna.b", "Anna Bee"), ("U2", "bob", ""), ("U3", "hannah", "Hannah")] {
            let mut u = User { id: id.into(), name: name.into(), ..Default::default() };
            u.profile.display_name = display.into();
            w.users.insert(id.into(), u);
        }
        for (id, name) in [("C1", "general"), ("C2", "random"), ("C3", "gen-z")] {
            w.conversations.insert(id.into(), Conversation { id: id.into(), name: name.into(), is_channel: true, is_member: true, ..Default::default() });
        }
        w
    }

    #[test]
    fn users_prefix_first_then_words_then_substring() {
        let w = ws();
        let s = suggestions(&w, &Trigger::User("an".into()));
        let labels: Vec<&str> = s.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, vec!["Anna Bee", "Hannah"]);
        assert_eq!(s[0].replacement, "@Anna Bee");
        assert_eq!(s[0].id.as_deref(), Some("U1"));
    }

    #[test]
    fn specials_and_channels_and_emoji() {
        let w = ws();
        let here = suggestions(&w, &Trigger::User("he".into()));
        assert!(here.iter().any(|s| s.id.as_deref() == Some("!here")));
        let chans: Vec<String> = suggestions(&w, &Trigger::Channel("gen".into())).into_iter().map(|s| s.label).collect();
        assert_eq!(chans, vec!["#gen-z", "#general"]);
        let emoji = suggestions(&w, &Trigger::Emoji("tad".into()));
        assert_eq!(emoji[0].replacement, ":tada:");
        assert!(emoji[0].id.is_none());
    }
}

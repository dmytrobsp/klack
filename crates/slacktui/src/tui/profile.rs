//! Profile cards: a person's name, title, status, local time and presence, with actions
//! (message, huddle). Opened with `u` on a message (its author, then anyone it mentions) and
//! from the Members tab of F2.

use chrono::{FixedOffset, Local, TimeZone, Utc};
use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use slack_core::format::{Block as Doc, Inline};
use slack_core::types::{Message, User};

use super::state::Workspace;
use super::text::{self, DIM};

pub struct ProfileCard {
    /// The people this card can show (author first, then mentions); `index` is shown.
    pub people: Vec<String>,
    pub index: usize,
}

impl ProfileCard {
    pub fn new(people: Vec<String>) -> Option<ProfileCard> {
        (!people.is_empty()).then_some(ProfileCard { people, index: 0 })
    }

    pub fn user(&self) -> &str {
        &self.people[self.index]
    }

    pub fn step(&mut self, delta: isize) {
        self.index = (self.index as isize + delta).rem_euclid(self.people.len() as isize) as usize;
    }

    pub fn draw(&self, f: &mut Frame, area: Rect, ws: &Workspace) {
        let [popup] = Layout::horizontal([Constraint::Length(56.min(area.width))]).flex(Flex::Center).areas(area);
        let [popup] = Layout::vertical([Constraint::Length(15.min(area.height))]).flex(Flex::Center).areas(popup);
        f.render_widget(Clear, popup);
        let mut keys = String::from(" Enter message · h huddle");
        if self.people.len() > 1 {
            keys += &format!(" · ←/→ {}/{}", self.index + 1, self.people.len());
        }
        keys += " · Esc ";
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(0x61, 0xaf, 0xef)))
            .title(" Profile ")
            .title_bottom(Line::styled(keys, Style::default().fg(DIM)).right_aligned());
        let lines = match ws.users.get(self.user()) {
            Some(u) => {
                let mut lines = vec![Line::styled(ws.person_label(&u.id), Style::default().fg(text::user_color(&u.id)).add_modifier(Modifier::BOLD)), Line::raw("")];
                lines.extend(person_lines(ws, u, Utc::now().timestamp()));
                lines
            }
            None => vec![Line::styled(format!("unknown person {}", self.user()), Style::default().fg(DIM))],
        };
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), popup);
    }
}

/// The message's author followed by the people it mentions (no duplicates).
pub fn people_in(m: &Message) -> Vec<String> {
    let mut out: Vec<String> = m.user.iter().cloned().collect();
    let mut add = |inlines: &[Inline]| {
        for i in inlines {
            if let Inline::User { id } = i {
                if !out.contains(id) {
                    out.push(id.clone());
                }
            }
        }
    };
    for b in slack_core::format::message_blocks(&m.text, &m.blocks) {
        match b {
            Doc::Paragraph(i) | Doc::Quote(i) => add(&i),
            Doc::List { items, .. } => items.iter().for_each(|i| add(i)),
            Doc::Code(_) => {}
        }
    }
    out
}

/// Your own availability for the status bar: status, away, paused notifications.
pub fn availability(ws: &Workspace, now: i64) -> String {
    let mut parts = Vec::new();
    if let Some(me) = ws.users.get(&ws.self_id) {
        let p = &me.profile;
        let live = p.status_expiration.is_none_or(|t| t == 0 || t > now);
        if live && (!p.status_emoji.is_empty() || !p.status_text.is_empty()) {
            let emoji = p.status_emoji.trim_matches(':');
            let emoji = if emoji.is_empty() { String::new() } else { text::emoji(ws, emoji, None) };
            parts.push(format!("{emoji} {}", p.status_text).trim().to_string());
        }
    }
    if ws.presence.get(&ws.self_id) == Some(&false) {
        parts.push("away".into());
    }
    match ws.dnd.snooze_until.filter(|t| *t > now as f64) {
        Some(t) => parts.push(format!("🔕 until {}", Local.timestamp_opt(t as i64, 0).single().map(|d| d.format(ws.settings.time_format()).to_string()).unwrap_or_default())),
        None if ws.dnd.active(now as f64) => parts.push("🔕".into()),
        None => {}
    }
    parts.join(" · ")
}

pub fn field(label: &str, value: String) -> Line<'static> {
    Line::from(vec![Span::styled(format!("{label:<11}"), Style::default().fg(DIM)), Span::raw(value)])
}

/// Their local time at `now` (epoch seconds), e.g. `16:05 · Eastern European Summer Time`.
pub fn local_time(u: &User, now: i64, time_format: &str) -> Option<String> {
    let offset = FixedOffset::east_opt(u.tz_offset? as i32)?;
    let time = offset.timestamp_opt(now, 0).single()?.format(time_format).to_string();
    let label = u.tz_label.clone().or_else(|| u.tz.clone()).unwrap_or_default();
    Some(if label.is_empty() { time } else { format!("{time} · {label}") })
}

/// Name, handle, title, status, local time and presence.
pub fn person_lines(ws: &Workspace, u: &User, now: i64) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    if let Some(real) = u.real_name.as_deref().filter(|r| !r.is_empty() && *r != u.display()) {
        out.push(field("Full name", real.to_string()));
    }
    let pronouns = u.profile.pronouns.as_deref().filter(|p| !p.is_empty()).map(|p| format!(" ({p})")).unwrap_or_default();
    out.push(field("Handle", format!("@{}{pronouns}", u.name)));
    if !u.profile.title.is_empty() {
        out.push(field("Title", u.profile.title.clone()));
    }
    if !u.profile.status_text.is_empty() || !u.profile.status_emoji.is_empty() {
        let emoji = u.profile.status_emoji.trim_matches(':');
        let emoji = if emoji.is_empty() { String::new() } else { text::emoji(ws, emoji, None) + " " };
        let until = u
            .profile
            .status_expiration
            .filter(|t| *t > 0)
            .and_then(|t| Local.timestamp_opt(t, 0).single())
            .map(|t| format!(" (until {})", t.format(&format!("%a {}", ws.settings.time_format()))))
            .unwrap_or_default();
        out.push(field("Status", format!("{emoji}{}{until}", u.profile.status_text)));
    }
    if let Some(t) = local_time(u, now, ws.settings.time_format()) {
        out.push(field("Local time", t));
    }
    let presence = match ws.presence.get(&u.id) {
        Some(true) => "active",
        Some(false) => "away",
        None => "unknown",
    };
    out.push(field("Presence", presence.into()));
    if let Some(email) = u.profile.email.as_deref().filter(|e| !e.is_empty()) {
        out.push(field("Email", email.to_string()));
    }
    if u.is_bot || u.is_app_user {
        out.push(field("Kind", "app / bot".into()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn author_then_mentions() {
        let m = Message { user: Some("U1".into()), text: "hi <@U2> and <@U1>, also <@U3>".into(), ..Default::default() };
        assert_eq!(people_in(&m), vec!["U1", "U2", "U3"]);
        assert!(people_in(&Message { text: "bot says hi".into(), ..Default::default() }).is_empty());
    }

    #[test]
    fn local_time_uses_the_offset() {
        let u = User { tz_offset: Some(3 * 3600), tz_label: Some("Eastern European Summer Time".into()), ..Default::default() };
        // 2026-10-18 12:00 UTC
        assert_eq!(local_time(&u, 1_792_324_800, "%H:%M").as_deref(), Some("15:00 · Eastern European Summer Time"));
        assert_eq!(local_time(&u, 1_792_324_800, "%-I:%M %p").as_deref(), Some("3:00 PM · Eastern European Summer Time"));
        assert_eq!(local_time(&User::default(), 0, "%H:%M"), None);
    }

    #[test]
    fn availability_summary() {
        let mut ws = Workspace { self_id: "UME".into(), ..Default::default() };
        let mut me = User { id: "UME".into(), ..Default::default() };
        me.profile.status_text = "lunch".into();
        me.profile.status_expiration = Some(1000);
        ws.users.insert("UME".into(), me);
        ws.presence.insert("UME".into(), false);
        assert_eq!(availability(&ws, 500), "lunch · away");
        assert_eq!(availability(&ws, 2000), "away", "an expired status is not shown");
        ws.dnd.snooze_until = Some(3000.0);
        assert!(availability(&ws, 2000).starts_with("away · 🔕 until "), "{}", availability(&ws, 2000));
    }

    #[test]
    fn card_cycles_people() {
        let mut c = ProfileCard::new(vec!["U1".into(), "U2".into()]).unwrap();
        c.step(1);
        assert_eq!(c.user(), "U2");
        c.step(1);
        assert_eq!(c.user(), "U1");
        c.step(-1);
        assert_eq!(c.user(), "U2");
        assert!(ProfileCard::new(vec![]).is_none());
    }
}

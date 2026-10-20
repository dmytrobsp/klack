//! Profile cards: a person's name, title, status, local time and presence, with actions
//! (message, huddle). Opened with `␣u` on a message (its author, then anyone it mentions) and
//! from the Members tab of the details.

use chrono::{FixedOffset, TimeZone, Utc};
use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use slack_core::format::{Block as Doc, Inline};
use slack_core::types::{Message, User};

use super::state::Workspace;
use super::text;

pub struct ProfileCard {
    /// The people this card can show (author first, then mentions); `index` is shown.
    pub people: Vec<String>,
    pub index: usize,
    /// Last drawn area, for clicks.
    pub area: Rect,
}

impl ProfileCard {
    pub fn new(people: Vec<String>) -> Option<ProfileCard> {
        (!people.is_empty()).then_some(ProfileCard { people, index: 0, area: Rect::default() })
    }

    pub fn user(&self) -> &str {
        &self.people[self.index]
    }

    pub fn step(&mut self, delta: isize) {
        self.index = (self.index as isize + delta).rem_euclid(self.people.len() as isize) as usize;
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, ws: &Workspace) {
        let [popup] = Layout::horizontal([Constraint::Length(56.min(area.width))]).flex(Flex::Center).areas(area);
        let [popup] = Layout::vertical([Constraint::Length(15.min(area.height))]).flex(Flex::Center).areas(popup);
        self.area = popup;
        super::media::clear_modal(f, popup);
        let keys = if self.people.len() > 1 { format!(" {}/{} in this message ", self.index + 1, self.people.len()) } else { String::new() };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(super::theme::p().accent))
            .title(format!(" {} ", super::i18n::tr("Profile")))
            .title(vip_badge(ws, self.user()).right_aligned())
            .title_bottom(Line::styled(keys, Style::default().fg(super::theme::p().dim)).right_aligned());
        let lines = match ws.users.get(self.user()) {
            Some(u) => {
                let mut lines = vec![Line::styled(ws.person_label(&u.id), Style::default().fg(text::user_color(&u.id)).add_modifier(Modifier::BOLD)), Line::raw("")];
                lines.extend(person_lines(ws, u, Utc::now().timestamp()));
                lines
            }
            None => vec![Line::styled(format!("unknown person {}", self.user()), Style::default().fg(super::theme::p().dim))],
        };
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), popup);
    }
}

/// The VIP marker for a card's top-right border: a gold "★ VIP" for VIPs, a dim "☆ VIP"
/// for everyone else (nothing for yourself).
pub fn vip_badge(ws: &Workspace, user: &str) -> Line<'static> {
    let p = super::theme::p();
    if user == ws.self_id {
        Line::raw("")
    } else if ws.vip.contains(user) {
        Line::styled(" ★ VIP ", Style::default().fg(Color::Black).bg(p.pin).add_modifier(Modifier::BOLD))
    } else {
        Line::styled(" ☆ VIP ", Style::default().fg(p.dim))
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

/// Your own availability for the status bar: status and paused notifications (away is its dot).
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
    if let Some(t) = ws.dnd_until(now as f64) {
        parts.push(match super::clock::until_label(t as i64, now, ws.settings.time_format()) {
            Some(label) => format!("🔕 until {label}"),
            None => "🔕".into(),
        });
    }
    parts.join(" · ")
}

pub fn field(label: &str, value: String) -> Line<'static> {
    Line::from(vec![Span::styled(format!("{label:<11}"), Style::default().fg(super::theme::p().dim)), Span::raw(value)])
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
            .and_then(|t| super::clock::at(t))
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
        assert_eq!(availability(&ws, 500), "lunch", "away is the status line's dot");
        assert_eq!(availability(&ws, 2000), "", "an expired status is not shown");
        ws.dnd.snooze_until = Some(3000.0);
        assert!(availability(&ws, 2000).starts_with("🔕 until "), "{}", availability(&ws, 2000));
        // A scheduled period reads the same, and another day names its weekday; the clock follows the setting.
        let noon = |days: i64| crate::tui::clock::local_minute(crate::tui::clock::zone(), crate::tui::clock::at(1_800_000_000).unwrap().date_naive() + chrono::Days::new(days as u64), 12 * 60).unwrap();
        let (today, tomorrow) = (noon(0), noon(1));
        ws.dnd = crate::tui::state::Dnd { window: Some((today - 3600, today + 3600)).map(|(a, b)| (a as f64, b as f64)), ..Default::default() };
        ws.settings.region.clock_24h = true;
        assert_eq!(availability(&ws, today), "🔕 until 13:00");
        ws.settings.region.clock_24h = false;
        assert_eq!(availability(&ws, today), "🔕 until 1:00 PM");
        ws.settings.region.clock_24h = true;
        ws.dnd = crate::tui::state::Dnd { snooze_until: Some(tomorrow as f64 + 3600.0), ..Default::default() };
        let tomorrow_label = availability(&ws, today);
        assert!(tomorrow_label.starts_with("🔕 until ") && tomorrow_label.ends_with(" 13:00") && tomorrow_label.len() > "🔕 until 13:00".len(), "{tomorrow_label}");
    }

    #[test]
    fn vip_badge_always_there() {
        let mut ws = Workspace { self_id: "UME".into(), ..Default::default() };
        let text = |l: Line| l.spans.iter().map(|s| s.content.to_string()).collect::<String>();
        assert_eq!(text(vip_badge(&ws, "U2")), " ☆ VIP ");
        ws.vip.insert("U2".into());
        assert_eq!(text(vip_badge(&ws, "U2")), " ★ VIP ");
        assert_eq!(text(vip_badge(&ws, "UME")), "", "not on your own card");
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

//! Profile cards: a person's big picture, name, title, status, local time and presence, with actions
//! (message, huddle). Opened with `␣u` on a message (its author, then anyone it mentions) and
//! from the Members tab of the details.

use chrono::{FixedOffset, TimeZone, Utc};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui_image::sliced::{SignedPosition, SlicedImage};
use slack_core::format::{Block as Doc, Inline};
use slack_core::types::{File, Message, User};

use super::media::{Media, Thumb};
use super::state::Workspace;
use super::text;

pub struct ProfileCard {
    /// The people this card can show (author first, then mentions); `index` is shown.
    pub people: Vec<String>,
    pub index: usize,
    /// Last drawn area, for clicks.
    pub area: Rect,
    /// The picture it drew without having it yet (to load).
    pub wanted: Vec<(File, (u16, u16))>,
}

/// The card's height without the picture, borders included.
const TEXT_ROWS: u16 = 15;
/// The picture's heights in rows (twice as many columns: about square), the largest that fits is drawn.
const PICTURE_ROWS: [u16; 5] = [5, 6, 8, 10, 12];

/// The picture's size in cells on a card `width` wide with `spare` rows beyond its text (it takes
/// a blank row under it too): the largest that fits, None when not even the smallest does.
pub fn picture_size(width: u16, spare: u16) -> Option<(u16, u16)> {
    let inner = width.saturating_sub(2);
    PICTURE_ROWS.iter().rev().find(|&&rows| rows < spare && rows * 2 <= inner).map(|&rows| (rows * 2, rows))
}

impl ProfileCard {
    pub fn new(people: Vec<String>) -> Option<ProfileCard> {
        (!people.is_empty()).then_some(ProfileCard { people, index: 0, area: Rect::default(), wanted: Vec::new() })
    }

    pub fn user(&self) -> &str {
        &self.people[self.index]
    }

    pub fn step(&mut self, delta: isize) {
        self.index = (self.index as isize + delta).rem_euclid(self.people.len() as isize) as usize;
    }

    /// The card centred in `area`: their picture on top (the initials while it loads, or without
    /// pictures), then the name and [`person_lines`]; a short `area` gets a smaller picture or none.
    pub fn draw(&mut self, f: &mut Frame, area: Rect, ws: &Workspace, media: &Media) {
        let width = 56.min(area.width);
        let picture = picture_size(width, area.height.saturating_sub(TEXT_ROWS));
        let height = TEXT_ROWS + picture.map_or(0, |(_, rows)| rows + 1);
        let [popup] = Layout::horizontal([Constraint::Length(width)]).flex(Flex::Center).areas(area);
        let [popup] = Layout::vertical([Constraint::Length(height.min(area.height))]).flex(Flex::Center).areas(popup);
        self.area = popup;
        self.wanted.clear();
        super::media::clear_modal(f, popup);
        let keys = if self.people.len() > 1 { format!(" {}/{} in this message ", self.index + 1, self.people.len()) } else { String::new() };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(super::theme::p().accent))
            .title(format!(" {} ", super::i18n::tr("Profile")))
            .title(vip_badge(ws, self.user()).right_aligned())
            .title_bottom(Line::styled(keys, Style::default().fg(super::theme::p().dim)).right_aligned());
        let inner = block.inner(popup);
        f.render_widget(block, popup);
        let mut text = inner;
        if let Some(size) = picture {
            let pic = Rect { x: inner.x + inner.width.saturating_sub(size.0) / 2, y: inner.y, width: size.0.min(inner.width), height: size.1.min(inner.height) };
            self.draw_picture(f, pic, size, ws, media);
            let below = (size.1 + 1).min(inner.height);
            text = Rect { y: inner.y + below, height: inner.height - below, ..inner };
        }
        let lines = match ws.users.get(self.user()) {
            Some(u) => {
                let mut lines = vec![Line::styled(ws.person_label(&u.id), Style::default().fg(text::user_color(&u.id)).add_modifier(Modifier::BOLD)), Line::raw("")];
                lines.extend(person_lines(ws, u, Utc::now().timestamp()));
                lines
            }
            None => vec![Line::styled(format!("unknown person {}", self.user()), Style::default().fg(super::theme::p().dim))],
        };
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), text);
    }

    /// Their initials in their colour over `pic`, and their picture over those once it is loaded
    /// (asked for here when it is not).
    fn draw_picture(&mut self, f: &mut Frame, pic: Rect, size: (u16, u16), ws: &Workspace, media: &Media) {
        let id = self.user().to_string();
        let colour = text::user_color(&id);
        let middle = Rect { y: pic.y + pic.height / 2, height: 1.min(pic.height), ..pic };
        f.buffer_mut().set_style(pic, Style::default().bg(colour));
        let initials = format!(" {} ", super::messages::initials(&ws.user_name(&id)));
        f.render_widget(Paragraph::new(initials).style(Style::default().bg(colour).fg(Color::Black).add_modifier(Modifier::BOLD)).alignment(Alignment::Center), middle);
        let Some(file) = ws.users.get(&id).and_then(|u| super::media::profile_picture_file(u, size)).filter(|_| media.avatars && ws.settings.messages.avatars) else { return };
        match media.thumbs.get(&file.id) {
            Some(Thumb::Ready(proto)) => f.render_widget(SlicedImage::new(proto, SignedPosition { x: 0, y: 0 }), pic),
            Some(_) => {}
            None => self.wanted.push((file, size)),
        }
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

    /// B (`U2`) and C (`U3`), each with a big picture.
    fn people() -> Workspace {
        let mut ws = Workspace { self_id: "UME".into(), ..Default::default() };
        for (id, name) in [("U2", "B Two"), ("U3", "C Three")] {
            let mut u = User { id: id.into(), name: id.to_lowercase(), ..Default::default() };
            u.profile.display_name = name.into();
            u.profile.image_512 = Some(format!("https://avatars.slack-edge.com/{id}_512.png"));
            ws.users.insert(id.into(), u);
        }
        ws
    }

    /// A kitty terminal drawing profile pictures.
    fn kitty() -> Media {
        let mut picker = ratatui_image::picker::Picker::halfblocks();
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
        let mut media = Media::with(picker);
        media.avatars = true;
        media
    }

    /// The pictures in `wanted`, loaded (blank ones of their size).
    fn load(media: &mut Media, wanted: &[(File, (u16, u16))]) {
        let font = media.picker.font_size();
        for (file, size) in wanted {
            let img = image::DynamicImage::new_rgb8(size.0 as u32 * font.width as u32, size.1 as u32 * font.height as u32);
            let proto = ratatui_image::sliced::SlicedProtocol::new(&media.picker, img, Some(ratatui::layout::Size::new(size.0, size.1))).unwrap();
            media.thumbs.insert(file.id.clone(), Thumb::Ready(proto));
        }
    }

    /// The smallest rect around the cells of `buf` that show a picture (kitty's placeholders).
    fn picture_at(buf: &ratatui::buffer::Buffer) -> Option<Rect> {
        let cells: Vec<(u16, u16)> = (0..buf.area.height).flat_map(|y| (0..buf.area.width).map(move |x| (x, y))).filter(|&(x, y)| buf[(x, y)].symbol().contains('\u{10EEEE}')).collect();
        let (x0, y0) = (cells.iter().map(|c| c.0).min()?, cells.iter().map(|c| c.1).min()?);
        let (x1, y1) = (cells.iter().map(|c| c.0).max()?, cells.iter().map(|c| c.1).max()?);
        assert_eq!(cells.len(), ((x1 - x0 + 1) * (y1 - y0 + 1)) as usize, "one whole picture");
        Some(Rect::new(x0, y0, x1 - x0 + 1, y1 - y0 + 1))
    }

    fn row(buf: &ratatui::buffer::Buffer, y: u16) -> String {
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    /// The picture's slot on a card drawn at `card`: centred under the top border.
    fn slot(card: Rect, size: (u16, u16)) -> Rect {
        Rect::new(card.x + 1 + (card.width - 2 - size.0) / 2, card.y + 1, size.0, size.1)
    }

    #[test]
    fn the_card_shows_a_big_picture_on_top() {
        let ws = people();
        let mut media = kitty();
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 40)).unwrap();
        let mut card = ProfileCard::new(vec!["U2".into()]).unwrap();
        terminal.draw(|f| card.draw(f, f.area(), &ws, &media)).unwrap();
        assert_eq!((card.area.width, card.area.height), (56, TEXT_ROWS + 13), "the card grows by the picture and a blank row");
        let wanted: Vec<_> = card.wanted.iter().map(|(f, size)| (f.id.as_str(), *size)).collect();
        assert_eq!(wanted, [("avatar:U2@512:U2_512.png:24x12", (24, 12))], "the big picture, at the card's size");
        load(&mut media, &card.wanted.clone());
        terminal.draw(|f| card.draw(f, f.area(), &ws, &media)).unwrap();
        assert!(card.wanted.is_empty(), "loaded: not asked for again");
        let buf = terminal.backend().buffer().clone();
        let pic = slot(card.area, (24, 12));
        assert_eq!(picture_at(&buf), Some(pic), "24 × 12, centred at the top");
        assert_eq!(pic.x - card.area.x, card.area.right() - pic.right(), "as far from both sides");
        assert!(row(&buf, pic.bottom()).trim_matches(|c| c == ' ' || c == '│').is_empty(), "a blank row under it");
        assert!(row(&buf, pic.bottom() + 1).contains("B Two"), "then the name: {:?}", row(&buf, pic.bottom() + 1));
        assert!(row(&buf, pic.bottom() + 3).contains("Handle") && row(&buf, pic.bottom() + 3).contains("@u2"), "then the rest as before");
    }

    #[test]
    fn a_short_terminal_gets_a_smaller_picture_or_none() {
        assert_eq!(picture_size(56, 30), Some((24, 12)), "never bigger than 24 × 12");
        assert_eq!(picture_size(56, 13), Some((24, 12)));
        assert_eq!(picture_size(56, 12), Some((20, 10)));
        assert_eq!(picture_size(56, 9), Some((16, 8)));
        assert_eq!(picture_size(56, 7), Some((12, 6)));
        assert_eq!(picture_size(56, 6), Some((10, 5)));
        assert_eq!(picture_size(56, 5), None, "too few rows to spare: none");
        assert_eq!(picture_size(56, 0), None);
        assert_eq!(picture_size(20, 30), Some((16, 8)), "a narrow card: as wide as fits");
        assert_eq!(picture_size(13, 30), Some((10, 5)));
        assert_eq!(picture_size(11, 30), None);
        let ws = people();
        let media = kitty();
        let mut card = ProfileCard::new(vec!["U2".into()]).unwrap();
        // 24 rows: 9 to spare, a 16 × 8 picture; the text all there.
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 24)).unwrap();
        terminal.draw(|f| card.draw(f, f.area(), &ws, &media)).unwrap();
        assert_eq!(card.area.height, 24);
        assert_eq!(card.wanted.iter().map(|(_, size)| *size).collect::<Vec<_>>(), [(16, 8)]);
        let buf = terminal.backend().buffer().clone();
        let pic = slot(card.area, (16, 8));
        assert!(row(&buf, pic.bottom() + 1).contains("B Two") && (0..24).any(|y| row(&buf, y).contains("Presence")), "the text is not cut");
        // 20 rows: none, the card as it was.
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 20)).unwrap();
        terminal.draw(|f| card.draw(f, f.area(), &ws, &media)).unwrap();
        assert_eq!(card.area.height, TEXT_ROWS);
        assert!(card.wanted.is_empty(), "no picture asked for");
        assert!(row(terminal.backend().buffer(), card.area.y + 1).contains("B Two"), "the name first");
    }

    #[test]
    fn the_initials_stand_in_while_loading_and_without_pictures() {
        let mut ws = people();
        let mut media = kitty();
        let mut card = ProfileCard::new(vec!["U2".into()]).unwrap();
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 40)).unwrap();
        let initials_in = |buf: &ratatui::buffer::Buffer, card: Rect| {
            let pic = slot(card, (24, 12));
            let middle = row(buf, pic.y + pic.height / 2);
            assert!(middle.contains(" BT "), "the initials in the middle: {middle:?}");
            let corners = [(pic.x, pic.y), (pic.right() - 1, pic.y), (pic.x, pic.bottom() - 1), (pic.right() - 1, pic.bottom() - 1)];
            assert!(corners.iter().all(|&c| buf[c].bg == text::user_color("U2")), "a block in their colour, the picture's size");
            assert_eq!(buf[(pic.x - 1, pic.y)].bg, ratatui::style::Color::Reset, "no wider");
            assert!(picture_at(buf).is_none());
        };
        terminal.draw(|f| card.draw(f, f.area(), &ws, &media)).unwrap();
        initials_in(terminal.backend().buffer(), card.area);
        // Loading, or failed: the initials stay, nothing asked for again.
        let (file, _) = card.wanted[0].clone();
        media.thumbs.insert(file.id.clone(), Thumb::Loading);
        terminal.draw(|f| card.draw(f, f.area(), &ws, &media)).unwrap();
        initials_in(terminal.backend().buffer(), card.area);
        assert!(card.wanted.is_empty());
        media.thumbs.insert(file.id.clone(), Thumb::Failed("download failed".into()));
        terminal.draw(|f| card.draw(f, f.area(), &ws, &media)).unwrap();
        initials_in(terminal.backend().buffer(), card.area);
        let with_pictures = card.area;
        // Without pictures (half blocks; or "Show profile pictures" off): the same card, the initials.
        let halfblocks = Media::with(ratatui_image::picker::Picker::halfblocks());
        terminal.draw(|f| card.draw(f, f.area(), &ws, &halfblocks)).unwrap();
        assert_eq!(card.area, with_pictures, "the card does not jump");
        initials_in(terminal.backend().buffer(), card.area);
        assert!(card.wanted.is_empty());
        ws.settings.messages.avatars = false;
        media.thumbs.clear();
        terminal.draw(|f| card.draw(f, f.area(), &ws, &media)).unwrap();
        initials_in(terminal.backend().buffer(), card.area);
        assert!(card.wanted.is_empty());
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

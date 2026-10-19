//! `/command`, `@user`, `#channel` and `:emoji:` suggestions for the composer, and file paths
//! after `/upload`.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::composer::Trigger;
use super::picker::EmojiPicker;
use super::state::Workspace;

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

/// Suggestions for `trigger` in a composer that posts to `conversation`.
pub fn suggestions(ws: &Workspace, conversation: Option<&str>, trigger: &Trigger) -> Vec<Suggestion> {
    match trigger {
        Trigger::Path { partial, quoted } => path_suggestions(partial, *quoted),
        Trigger::Argument { command, before, partial } => arguments(command, before, partial, &[]),
        // All matching commands: the list scrolls.
        Trigger::Command(q) => {
            let q = q.to_lowercase();
            let own = super::commands::OWN.iter().map(|&(name, usage, desc)| (name.to_string(), usage.to_string(), desc.to_string()));
            let slack = ws.slack_commands.iter().filter(|c| !super::commands::OWN.iter().any(|(name, ..)| *name == c.name)).map(|c| (c.name.clone(), c.usage.clone(), c.desc.clone()));
            let mut out: Vec<(u8, Suggestion)> = own
                .chain(slack)
                .filter_map(|(name, usage, desc)| {
                    // By name only: its start, or a part after `-` (`pri` → /create-private).
                    let r = name[1..].split('-').position(|part| part.starts_with(&q))?.min(1) as u8;
                    Some((r, Suggestion { label: format!("{name} {usage}").trim_end().to_string(), detail: desc, replacement: name, id: None }))
                })
                .collect();
            out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.replacement.cmp(&b.1.replacement)));
            out.into_iter().map(|(_, s)| s).collect()
        }
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
            // @here and friends mean nothing in a DM or group DM.
            let in_dm = conversation.and_then(|c| ws.conversations.get(c)).is_some_and(|c| c.is_dm());
            for (name, detail) in [("here", "notify active members"), ("channel", "notify everyone in the channel"), ("everyone", "notify the whole workspace")] {
                if name.starts_with(&q) && !in_dm {
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
            let picker = EmojiPicker { query: q.clone(), selected: 0 };
            picker
                .candidates(&ws.custom_emoji)
                .into_iter()
                .take(MAX_ITEMS)
                .map(|c| Suggestion { label: format!("{} :{}:", c.glyph, c.name), detail: if c.custom { "custom".into() } else { String::new() }, replacement: format!(":{}:", c.name), id: None })
                .collect()
        }
    }
}

/// Fixed choices for the word being typed after one of klack's commands (`before` holds the
/// words between the command and it): `/workspace` names (from `workspaces`), `/section`'s
/// actions, `/dnd` and `/status … for` durations, `/status clear`, `/snippet` languages.
pub fn arguments(command: &str, before: &[String], partial: &str, workspaces: &[String]) -> Vec<Suggestion> {
    const DURATIONS: [(&str, &str); 6] = [("30m", "30 minutes"), ("1h", "an hour"), ("2h", "two hours"), ("4h", "four hours"), ("8h", "eight hours"), ("1d", "a day")];
    let pairs = |list: &[(&str, &str)]| list.iter().map(|&(v, d)| (v.to_string(), d.to_string())).collect::<Vec<_>>();
    let choices = match (command, before) {
        ("/workspace", []) => workspaces.iter().map(|w| (w.clone(), "switch to it".to_string())).collect(),
        ("/section", []) => pairs(&[("rename", "rename this conversation's section (then its new name)"), ("delete", "delete this conversation's section")]),
        ("/dnd", []) => [pairs(&DURATIONS), pairs(&[("off", "resume notifications")])].concat(),
        ("/status", []) => pairs(&[("clear", "clear your status")]),
        ("/status", [.., last]) if last == "for" => [pairs(&DURATIONS[..5]), pairs(&[("today", "until midnight")])].concat(),
        ("/snippet", []) => super::commands::SNIPPET_LANGS.iter().map(|l| (l.to_string(), "language".to_string())).collect(),
        _ => Vec::new(),
    };
    let q = partial.to_lowercase();
    choices
        .into_iter()
        .filter(|(value, _)| value.to_lowercase().starts_with(&q))
        .map(|(value, detail)| Suggestion { label: value.clone(), detail, replacement: value, id: None })
        .collect()
}

/// At most this many entries of a folder are looked at.
const MAX_ENTRIES: usize = 5000;

/// The entries of the folder in `partial` (a path being typed; relative to klack's working
/// folder, `~` for home) whose names start with its last part, any case; hidden ones only when
/// that part starts with `.`. A folder completes with its `/` (and no space after it, see
/// `ends_open`) so its entries follow; a name with a space, or typed after a quote, is quoted.
pub fn path_suggestions(partial: &str, quoted: bool) -> Vec<Suggestion> {
    let partial = if partial == "~" { "~/" } else { partial };
    let (dir, prefix) = partial.split_at(partial.rfind('/').map_or(0, |i| i + 1));
    let listed = match dir.strip_prefix('~') {
        Some(tail) => match directories::UserDirs::new() {
            Some(d) => d.home_dir().join(tail.trim_start_matches('/')),
            None => return Vec::new(),
        },
        None if dir.is_empty() => std::path::PathBuf::from("."),
        None => std::path::PathBuf::from(dir),
    };
    let Ok(entries) = std::fs::read_dir(&listed) else { return Vec::new() };
    let lower = prefix.to_lowercase();
    let mut found: Vec<(String, Option<u64>)> = entries
        .flatten()
        .take(MAX_ENTRIES)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if (name.starts_with('.') && !prefix.starts_with('.')) || !name.to_lowercase().starts_with(&lower) {
                return None;
            }
            // Through links: a link to a folder is a folder.
            let meta = std::fs::metadata(e.path()).ok()?;
            Some((name, (!meta.is_dir()).then(|| meta.len())))
        })
        .collect();
    found.sort_by_key(|(name, _)| name.to_lowercase());
    found
        .into_iter()
        .map(|(name, size)| {
            let slash = if size.is_none() { "/" } else { "" };
            let path = format!("{dir}{name}{slash}");
            let replacement = match (quoted || path.contains(char::is_whitespace), size) {
                (false, _) => path,
                (true, None) => format!("\"{path}"),
                (true, Some(_)) => format!("\"{path}\""),
            };
            Suggestion { label: format!("{name}{slash}"), detail: size.map_or_else(|| "folder".into(), super::text::human_size), replacement, id: None }
        })
        .collect()
}

/// A replacement that is inserted without a space after it: a folder, whose entries come next.
pub fn ends_open(replacement: &str) -> bool {
    replacement.ends_with('/')
}

/// Where the list of `items` suggestions goes: just above `anchor` (the composer box). Commands
/// get more room for what they take and do.
fn popup_area(anchor: Rect, items: usize, wide: bool) -> Rect {
    let height = items.min(MAX_ITEMS) as u16 + 2;
    let width = anchor.width.min(if wide { 76 } else { 48 });
    let y = anchor.y.saturating_sub(height);
    Rect { x: anchor.x, y, width, height: height.min(anchor.y) }
}

impl Completion {
    fn area(&self, anchor: Rect) -> Rect {
        popup_area(anchor, self.items.len(), self.items.first().is_some_and(|s| s.replacement.starts_with('/')))
    }

    /// The first suggestion shown: the list scrolls to keep the highlighted one in view.
    fn first(&self) -> usize {
        (self.selected + 1).saturating_sub(MAX_ITEMS)
    }

    /// The suggestion drawn at (`col`, `row`) by [`draw`] above `anchor`.
    pub fn row_at(&self, anchor: Rect, col: u16, row: u16) -> Option<usize> {
        let area = self.area(anchor);
        let inner = Rect { x: area.x + 1, y: area.y + 1, width: area.width.saturating_sub(2), height: area.height.saturating_sub(2) };
        inner.contains((col, row).into()).then(|| self.first() + (row - inner.y) as usize).filter(|n| *n < self.items.len())
    }
}

/// Draw the list just above `anchor` (the composer box).
pub fn draw(f: &mut Frame, anchor: Rect, c: &Completion) {
    let area = c.area(anchor);
    super::media::clear_popup(f, area);
    let title = if c.items.len() > MAX_ITEMS { format!(" Suggestions {}/{} ", c.selected + 1, c.items.len()) } else { " Suggestions ".into() };
    let block = Block::default().borders(Borders::ALL).border_style(Style::default().fg(super::theme::p().accent)).title(title);
    let lines: Vec<Line> = c
        .items
        .iter()
        .enumerate()
        .skip(c.first())
        .take(MAX_ITEMS)
        .map(|(n, s)| {
            let line = Line::from(vec![Span::raw(format!(" {}", s.label)), Span::styled(format!("  {}", s.detail), Style::default().fg(super::theme::p().dim))]);
            if n == c.selected { line.style(Style::default().bg(super::theme::p().selection).add_modifier(Modifier::BOLD)) } else { line }
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
        let s = suggestions(&w, Some("C1"), &Trigger::User("an".into()));
        let labels: Vec<&str> = s.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, vec!["Anna Bee", "Hannah"]);
        assert_eq!(s[0].replacement, "@Anna Bee");
        assert_eq!(s[0].id.as_deref(), Some("U1"));
    }

    #[test]
    fn commands_klack_s_own_and_slack_s() {
        let mut w = ws();
        let cmd = |name: &str, desc: &str| slack_core::types::SlashCommand { name: name.into(), usage: String::new(), desc: desc.into() };
        w.slack_commands = vec![cmd("/remind", "Set a reminder"), cmd("/away", "Toggle your away status"), cmd("/shrug", "Appends a shrug")];
        let names = |q: &str| suggestions(&w, Some("C1"), &Trigger::Command(q.into())).into_iter().map(|s| s.replacement).collect::<Vec<_>>();
        let all = names("");
        assert_eq!(all.len(), super::super::commands::OWN.len() + 2, "Slack's /away is klack's");
        assert!(all.len() > MAX_ITEMS, "every command, not the first eight");
        assert_eq!(names("re"), ["/remind"], "by name, not by what it does");
        assert_eq!(names("pri"), ["/create-private"], "a part of the name");
        assert_eq!(names("CR"), ["/create", "/create-private"], "any case");
        let up = &suggestions(&w, Some("C1"), &Trigger::Command("up".into()))[0];
        assert_eq!((up.label.as_str(), up.detail.as_str(), up.replacement.as_str()), ("/upload <path> [text]", "upload a file", "/upload"));
    }

    #[test]
    fn fixed_choices_after_commands() {
        let got = |command: &str, before: &[&str], partial: &str| {
            let before: Vec<String> = before.iter().map(|w| w.to_string()).collect();
            arguments(command, &before, partial, &["Acme".into(), "rslack-testbed".into()]).into_iter().map(|s| s.replacement).collect::<Vec<_>>()
        };
        assert_eq!(got("/workspace", &[], ""), ["Acme", "rslack-testbed"]);
        assert_eq!(got("/workspace", &[], "rs"), ["rslack-testbed"]);
        assert_eq!(got("/section", &[], ""), ["rename", "delete"]);
        assert_eq!(got("/section", &["rename"], ""), Vec::<String>::new(), "the new name is free text");
        assert_eq!(got("/dnd", &[], "o"), ["off"]);
        assert_eq!(got("/dnd", &[], "").len(), 7);
        assert_eq!(got("/status", &[], ""), ["clear"]);
        assert_eq!(got("/status", &[":tada:", "party", "for"], "t"), ["today"]);
        assert_eq!(got("/status", &["lunch"], ""), Vec::<String>::new());
        assert_eq!(got("/snippet", &[], "rus"), ["rust"]);
        assert_eq!(got("/remind", &[], ""), Vec::<String>::new(), "Slack's commands: free text");
        for (command, before) in [("/dnd", vec![]), ("/status", vec!["for".to_string()])] {
            for s in arguments(command, &before, "", &[]) {
                let text = format!("{command} {}{}", if before.is_empty() { "" } else { "x for " }, s.replacement);
                assert!(matches!(super::super::commands::parse(&text, &text), Some(Ok(_))), "{text} is understood");
            }
        }
    }

    #[test]
    fn paths_after_upload() {
        let dir = std::env::temp_dir().join(format!("klack-paths-{}", std::process::id()));
        for d in ["Pictures", "Public", "My Docs"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        for f in ["pic.png", ".hidden", "notes.txt"] {
            std::fs::write(dir.join(f), "12345").unwrap();
        }
        let base = format!("{}/", dir.display());
        let got = |partial: &str, quoted| path_suggestions(&format!("{base}{partial}"), quoted).into_iter().map(|s| (s.label, s.replacement.replace(&base, "…/"))).collect::<Vec<_>>();
        let pair = |l: &str, r: &str| (l.to_string(), r.to_string());
        assert_eq!(got("p", false), [pair("pic.png", "…/pic.png"), pair("Pictures/", "…/Pictures/"), pair("Public/", "…/Public/")], "any case, by name");
        assert_eq!(got("", false).len(), 5, "no hidden files…");
        assert_eq!(got(".h", false), [pair(".hidden", "…/.hidden")], "…unless asked for");
        assert_eq!(got("My", false), [pair("My Docs/", "\"…/My Docs/")], "a space opens a quote, a folder leaves it open");
        assert_eq!(got("no", true), [pair("notes.txt", "\"…/notes.txt\"")], "typed after a quote: closed after a file");
        assert_eq!(path_suggestions(&format!("{base}nothing/x"), false), []);
        let s = &path_suggestions(&format!("{base}notes"), false)[0];
        assert_eq!(s.detail, "5 B");
        assert!(ends_open(&got("Pict", false)[0].1) && !ends_open("…/pic.png"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn long_lists_scroll() {
        let items: Vec<Suggestion> = (0..20).map(|i| Suggestion { label: format!("/c{i}"), detail: String::new(), replacement: format!("/c{i}"), id: None }).collect();
        let mut c = Completion { thread: false, start: 0, items, selected: 0 };
        let anchor = Rect { x: 0, y: 30, width: 100, height: 3 };
        assert_eq!(c.area(anchor), Rect { x: 0, y: 20, width: 76, height: 10 }, "eight rows, wide for commands");
        assert_eq!(c.row_at(anchor, 5, 21), Some(0));
        c.selected = 11;
        assert_eq!(c.row_at(anchor, 5, 21), Some(4), "scrolled to keep the 12th in view");
        assert_eq!(c.row_at(anchor, 5, 28), Some(11));
    }

    #[test]
    fn clicking_a_suggestion() {
        let items = suggestions(&ws(), Some("C1"), &Trigger::User("an".into()));
        let c = Completion { thread: false, start: 0, items, selected: 0 };
        // Composer at rows 30–32: two suggestions in a box on rows 26–29, inside rows 27 and 28.
        let anchor = Rect { x: 20, y: 30, width: 60, height: 3 };
        assert_eq!(c.row_at(anchor, 25, 27), Some(0));
        assert_eq!(c.row_at(anchor, 25, 28), Some(1));
        assert_eq!(c.row_at(anchor, 25, 26), None, "the border");
        assert_eq!(c.row_at(anchor, 25, 30), None, "the composer");
        assert_eq!(c.row_at(anchor, 70, 27), None, "right of the box (48 wide)");
    }

    #[test]
    fn specials_and_channels_and_emoji() {
        let w = ws();
        let here = suggestions(&w, Some("C1"), &Trigger::User("he".into()));
        assert!(here.iter().any(|s| s.id.as_deref() == Some("!here")));
        let mut w2 = ws();
        w2.conversations.insert("D1".into(), Conversation { id: "D1".into(), is_im: true, user: Some("U2".into()), ..Default::default() });
        w2.conversations.insert("G1".into(), Conversation { id: "G1".into(), is_mpim: true, ..Default::default() });
        for dm in ["D1", "G1"] {
            let s = suggestions(&w2, Some(dm), &Trigger::User("".into()));
            assert!(s.iter().all(|s| !s.id.as_deref().unwrap_or("").starts_with('!')), "no @here/@channel/@everyone in {dm}");
        }
        assert!(suggestions(&w2, Some("D1"), &Trigger::User("han".into())).iter().any(|s| s.label == "Hannah"), "people still offered");
        let chans: Vec<String> = suggestions(&w, Some("C1"), &Trigger::Channel("gen".into())).into_iter().map(|s| s.label).collect();
        assert_eq!(chans, vec!["#gen-z", "#general"]);
        let emoji = suggestions(&w, Some("C1"), &Trigger::Emoji("tad".into()));
        assert_eq!(emoji[0].replacement, ":tada:");
        assert!(emoji[0].id.is_none());
    }
}

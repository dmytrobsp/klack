//! What `o` opens in a message — its links (in the text, and the title links of attachments
//! and link previews), pictures, videos and snippets, in the order they appear — and opening a
//! link with the system's opener.

use slack_core::format::{self, Block, Inline};
use slack_core::types::{File, Message};

use super::applink::{self, AppLink, Here};

/// Opens links instead of the system's opener (a program name or path, given the address).
pub const OPEN_WITH_ENV: &str = "KLACK_OPEN_WITH";

#[derive(Debug, Clone)]
pub enum Thing {
    /// `app`: where it goes in klack, for a link into the profile's workspaces.
    Link { url: String, label: Option<String>, app: Option<AppLink> },
    /// The message's `n`th picture (the viewer steps through them all from there).
    Picture(usize),
    Video(File),
    Snippet(File),
}

/// Everything in `m` that can be opened, in message order: links first (they are in the text
/// and the previews below it), then the files. With `here`, links into the profile's workspaces come first (they open
/// in klack); without it every link is an outside one.
pub fn things(m: &Message, here: Option<&Here>) -> Vec<Thing> {
    let mut out = Vec::new();
    for block in format::message_blocks(&m.text, &m.blocks) {
        let items: Vec<&Inline> = match &block {
            Block::Paragraph(items) | Block::Quote(items) => items.iter().collect(),
            Block::List { items, .. } => items.iter().flatten().collect(),
            Block::Code(_) => Vec::new(),
        };
        for i in items {
            if let Inline::Link { url, label, .. } = i {
                add_link(&mut out, url, label.as_deref().and_then(|l| applink::own_words(url, l)).map(str::to_string), here);
            }
        }
    }
    for att in &m.attachments {
        let url = ["title_link", "from_url", "original_url"].iter().find_map(|k| att[*k].as_str());
        if let Some(url) = url {
            add_link(&mut out, url, att["title"].as_str().map(|t| format::unescape(t)).filter(|t| !t.is_empty()), here);
        }
    }
    let mut pictures = 0;
    for f in &m.files {
        if f.is_image() {
            out.push(Thing::Picture(pictures));
            pictures += 1;
        } else if f.is_video() {
            out.push(Thing::Video(f.clone()));
        } else if f.is_snippet() {
            out.push(Thing::Snippet(f.clone()));
        }
    }
    // In klack first (a stable sort: the rest keep their order).
    out.sort_by_key(|t| !t.in_app());
    out
}

fn add_link(out: &mut Vec<Thing>, url: &str, label: Option<String>, here: Option<&Here>) {
    if !out.iter().any(|t| matches!(t, Thing::Link { url: u, .. } if u == url)) {
        let app = here.and_then(|h| applink::parse(url).filter(|a| applink::workspace_of(a, h.workspaces, h.shown).is_some()));
        out.push(Thing::Link { url: url.to_string(), label, app });
    }
}

impl Thing {
    /// A link into the profile's workspaces, which opens in klack.
    pub fn in_app(&self) -> bool {
        matches!(self, Thing::Link { app: Some(_), .. })
    }
}

/// One line for the picker: an icon, then the address (without `https://`) and its label, or
/// the file's name.
/// An in-app link shows `↪ ` and where it goes, after the author's words for it when it has them.
pub fn label(t: &Thing, m: &Message, here: &Here) -> String {
    let name = |f: &File| if f.title.is_empty() { f.name.clone() } else { f.title.clone() };
    match t {
        Thing::Link { label, app: Some(app), .. } => {
            let readable = applink::label(app, here, None).unwrap_or_default();
            match label {
                Some(l) => format!("↪ {l} — {readable}"),
                None => format!("↪ {readable}"),
            }
        }
        Thing::Link { url, label, .. } => {
            let short = url.trim_start_matches("https://").trim_start_matches("http://").trim_start_matches("mailto:").trim_end_matches('/');
            match label {
                Some(l) => format!("🔗 {short}  {l}"),
                None => format!("🔗 {short}"),
            }
        }
        Thing::Picture(n) => format!("🖼\u{fe0f} {}", m.files.iter().filter(|f| f.is_image()).nth(*n).map(name).unwrap_or_default()),
        Thing::Video(f) => format!("🎞\u{fe0f} {}", name(f)),
        Thing::Snippet(f) => format!("📄 {}", name(f)),
    }
}

/// Only web and mail links are handed to the opener.
pub fn openable(url: &str) -> bool {
    slack_core::reqwest::Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "http" | "https" | "mailto"))
}

/// Open `url` with `KLACK_OPEN_WITH`, else the system's opener (`xdg-open`, `open`, the URL
/// handler on Windows): the address is one argument, never read by a shell.
pub fn open_url(url: &str) -> anyhow::Result<()> {
    anyhow::ensure!(openable(url), "klack opens only web and mail links");
    let mut cmd = match std::env::var(OPEN_WITH_ENV).ok().filter(|v| !v.trim().is_empty()) {
        Some(program) => std::process::Command::new(program),
        None if cfg!(target_os = "macos") => std::process::Command::new("open"),
        None if cfg!(windows) => {
            let mut c = std::process::Command::new("rundll32");
            c.arg("url.dll,FileProtocolHandler");
            c
        }
        None => std::process::Command::new("xdg-open"),
    };
    let program = cmd.get_program().to_string_lossy().into_owned();
    let mut child = cmd
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| anyhow::anyhow!("{program}: {e}"))?;
    // Reaped when it is done (openers hand over to the browser and exit).
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_s_things_in_order() {
        let file = |name: &str, mimetype: &str| File { id: name.into(), name: name.into(), mimetype: mimetype.into(), ..Default::default() };
        let mut m = Message {
            text: "see <https://github.com/acme/api/pull/482|the PR> and <https://docs.acme.dev/x> (again <https://docs.acme.dev/x>) <mailto:a@b.c|mail>".into(),
            ..Default::default()
        };
        m.attachments = vec![serde_json::json!({"title": "Fix the retry &amp; backoff", "title_link": "https://github.com/acme/api/pull/482"}), serde_json::json!({"from_url": "https://news.example/a", "title": "A story"})];
        m.files = vec![file("one.png", "image/png"), file("demo.mp4", "video/mp4"), file("two.jpg", "image/jpeg"), file("notes.pdf", "application/pdf")];
        let ws = super::super::state::Workspace::default();
        let here = Here::alone(&ws);
        let got = things(&m, None);
        let labels: Vec<String> = got.iter().map(|t| label(t, &m, &here)).collect();
        assert_eq!(
            labels,
            ["🔗 github.com/acme/api/pull/482  the PR", "🔗 docs.acme.dev/x", "🔗 a@b.c  mail", "🔗 news.example/a  A story", "🖼\u{fe0f} one.png", "🎞\u{fe0f} demo.mp4", "🖼\u{fe0f} two.jpg"],
            "text links once each (the PR's preview is the same link), then previews, then files; a PDF is not opened"
        );
        assert!(matches!(got[6], Thing::Picture(1)), "the second picture");
        assert!(things(&Message { text: "nothing here".into(), ..Default::default() }, None).is_empty());
    }

    #[test]
    fn only_web_and_mail_links_open() {
        assert!(openable("https://github.com/x") && openable("http://x.y") && openable("mailto:a@b.c"));
        assert!(!openable("file:///etc/passwd") && !openable("slack://channel?id=C1") && !openable("javascript:alert(1)") && !openable("not a url"));
        assert!(open_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn the_open_menu_groups_in_app_links() {
        use super::super::menu::{Item, Menu};
        let mut ws = super::super::state::Workspace::default();
        ws.conversations.insert("C1".into(), slack_core::types::Conversation { id: "C1".into(), name: "backend".into(), is_channel: true, ..Default::default() });
        let workspaces = vec![("https://acme.slack.com/".to_string(), "T1".to_string())];
        let names = vec!["acme".to_string()];
        let authors = applink::Authors::new();
        let here = Here { ws: &ws, workspaces: &workspaces, names: &names, shown: 0, authors: &authors, message: None };
        let reply = "https://acme.slack.com/archives/C1/p1729440000123456?thread_ts=1729440000.000100&cid=C1";
        let file = File { id: "F1".into(), name: "one.png".into(), mimetype: "image/png".into(), ..Default::default() };
        let m = Message { text: format!("<https://docs.acme.dev/x> then <{reply}|the fix> and <https://acme.slack.com/archives/C1>"), files: vec![file], ..Default::default() };
        let got = things(&m, Some(&here));
        let labels: Vec<String> = got.iter().map(|t| label(t, &m, &here.of(&m))).collect();
        assert_eq!(labels, ["↪ the fix — #backend › thread reply", "↪ #backend", "🔗 docs.acme.dev/x", "🖼\u{fe0f} one.png"], "in klack first, in message order within each group");
        let menu = Menu::opener(false, "1.0".into(), got.clone(), labels, (0, 0));
        let rows: Vec<(Item, &str)> = menu.items.iter().map(|i| (i.0, i.1.as_str())).collect();
        assert_eq!(
            rows,
            [
                (Item::Heading, "In klack"),
                (Item::Thing(0), "↪ the fix — #backend › thread reply"),
                (Item::Thing(1), "↪ #backend"),
                (Item::Heading, "In the browser"),
                (Item::Thing(2), "🔗 docs.acme.dev/x"),
                (Item::Heading, "Files"),
                (Item::Thing(3), "🖼\u{fe0f} one.png")
            ]
        );
        assert_eq!(menu.chosen(), Some(Item::Thing(0)), "it starts on a row, not the heading");
        let mut menu = menu;
        menu.move_by(2);
        assert_eq!(menu.chosen(), Some(Item::Thing(2)), "a heading is stepped over");
        menu.move_by(-1);
        assert_eq!(menu.chosen(), Some(Item::Thing(1)));
        menu.move_by(-5);
        assert_eq!(menu.chosen(), Some(Item::Thing(0)), "and the top is a row");
        // Without links into klack there are no headings.
        let plain = Menu::opener(false, "1.0".into(), things(&Message { text: "<https://a.b/> <https://c.d/>".into(), ..Default::default() }, Some(&here)), vec!["a".into(), "b".into()], (0, 0));
        assert!(plain.items.iter().all(|i| matches!(i.0, Item::Thing(_))));
        // A message with one thing that is an in-app link: that thing opens at once.
        let single = things(&Message { text: format!("<{reply}|>"), ..Default::default() }, Some(&here));
        assert!(matches!(single.as_slice(), [t] if t.in_app()));
        // The address as its label is not the author's words (the same rule as the message draws).
        let bare = Message { text: "<http://acme.slack.com/archives/C1|acme.slack.com/archives/C1>".into(), ..Default::default() };
        let got = things(&bare, Some(&here));
        assert_eq!(got.iter().map(|t| label(t, &bare, &here)).collect::<Vec<_>>(), ["↪ #backend"]);
        // Files only, after links into klack: no "In the browser".
        let files = Message { text: "<https://acme.slack.com/archives/C1>".into(), files: vec![File { id: "F2".into(), name: "two.png".into(), mimetype: "image/png".into(), ..Default::default() }], ..Default::default() };
        let got = things(&files, Some(&here));
        let rows: Vec<String> = Menu::opener(false, "1.0".into(), got, vec!["a".into(), "b".into()], (0, 0)).items.iter().map(|i| i.1.clone()).collect();
        assert_eq!(rows, ["In klack", "a", "Files", "b"]);
        // Another workspace's link is an outside one.
        let other = things(&Message { text: "<https://other.slack.com/archives/C9>".into(), ..Default::default() }, Some(&here));
        assert!(!other[0].in_app());
    }
}

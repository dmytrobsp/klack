//! What `o` opens in a message — its links (in the text, and the title links of attachments
//! and link previews), pictures, videos and snippets, in the order they appear — and opening a
//! link with the system's opener.

use slack_core::format::{self, Block, Inline};
use slack_core::types::{File, Message};

/// Opens links instead of the system's opener (a program name or path, given the address).
pub const OPEN_WITH_ENV: &str = "KLACK_OPEN_WITH";

#[derive(Debug, Clone)]
pub enum Thing {
    Link { url: String, label: Option<String> },
    /// The message's `n`th picture (the viewer steps through them all from there).
    Picture(usize),
    Video(File),
    Snippet(File),
}

/// Everything in `m` that can be opened, in message order: links first (they are in the text
/// and the previews below it), then the files.
pub fn things(m: &Message) -> Vec<Thing> {
    let mut out = Vec::new();
    for block in format::message_blocks(&m.text, &m.blocks) {
        let items: Vec<&Inline> = match &block {
            Block::Paragraph(items) | Block::Quote(items) => items.iter().collect(),
            Block::List { items, .. } => items.iter().flatten().collect(),
            Block::Code(_) => Vec::new(),
        };
        for i in items {
            if let Inline::Link { url, label, .. } = i {
                add_link(&mut out, url, label.clone().filter(|l| !l.is_empty() && l != url));
            }
        }
    }
    for att in &m.attachments {
        let url = ["title_link", "from_url", "original_url"].iter().find_map(|k| att[*k].as_str());
        if let Some(url) = url {
            add_link(&mut out, url, att["title"].as_str().map(|t| format::unescape(t)).filter(|t| !t.is_empty()));
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
    out
}

fn add_link(out: &mut Vec<Thing>, url: &str, label: Option<String>) {
    if !out.iter().any(|t| matches!(t, Thing::Link { url: u, .. } if u == url)) {
        out.push(Thing::Link { url: url.to_string(), label });
    }
}

/// One line for the picker: an icon, then the address (without `https://`) and its label, or
/// the file's name.
pub fn label(t: &Thing, m: &Message) -> String {
    let name = |f: &File| if f.title.is_empty() { f.name.clone() } else { f.title.clone() };
    match t {
        Thing::Link { url, label } => {
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
        let got = things(&m);
        let labels: Vec<String> = got.iter().map(|t| label(t, &m)).collect();
        assert_eq!(
            labels,
            ["🔗 github.com/acme/api/pull/482  the PR", "🔗 docs.acme.dev/x", "🔗 a@b.c  mail", "🔗 news.example/a  A story", "🖼\u{fe0f} one.png", "🎞\u{fe0f} demo.mp4", "🖼\u{fe0f} two.jpg"],
            "text links once each (the PR's preview is the same link), then previews, then files; a PDF is not opened"
        );
        assert!(matches!(got[6], Thing::Picture(1)), "the second picture");
        assert!(things(&Message { text: "nothing here".into(), ..Default::default() }).is_empty());
    }

    #[test]
    fn only_web_and_mail_links_open() {
        assert!(openable("https://github.com/x") && openable("http://x.y") && openable("mailto:a@b.c"));
        assert!(!openable("file:///etc/passwd") && !openable("slack://channel?id=C1") && !openable("javascript:alert(1)") && !openable("not a url"));
        assert!(open_url("file:///etc/passwd").is_err());
    }
}

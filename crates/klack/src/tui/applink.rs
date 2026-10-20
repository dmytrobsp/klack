//! Slack links that point into a workspace this profile has open: parsing, matching and labels.

use slack_core::types::Message;

use super::state::Workspace;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Conversation { channel: String },
    Message { channel: String, ts: String, thread_ts: Option<String> },
    Person { user: String },
    File { file: String },
}

impl Target {
    /// The channel the target is in, if it names one.
    pub fn channel(&self) -> Option<&str> {
        match self {
            Target::Conversation { channel } | Target::Message { channel, .. } => Some(channel),
            Target::Person { .. } | Target::File { .. } => None,
        }
    }
}

impl From<super::notify::Target> for Target {
    fn from(t: super::notify::Target) -> Target {
        match t.ts {
            Some(ts) => Target::Message { channel: t.channel, ts, thread_ts: t.thread_ts },
            None => Target::Conversation { channel: t.channel },
        }
    }
}

/// How the link names its workspace: by host (`acme.slack.com`) or by team id (`app.slack.com/client/T…`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Team {
    Host(String),
    Id(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppLink {
    pub team: Team,
    pub target: Target,
}

fn id(s: &str) -> Option<String> {
    (!s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric())).then(|| s.to_string())
}

/// `p1729440000123456` -> `1729440000.123456`.
fn permalink_ts(p: &str) -> Option<String> {
    let d = p.strip_prefix('p')?;
    if d.len() < 16 || !d.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (s, us) = d.split_at(d.len() - 6);
    Some(format!("{s}.{us}"))
}

/// A Slack timestamp, `1729430000.000100`.
fn plain_ts(s: &str) -> Option<String> {
    let (a, b) = s.split_once('.')?;
    let digits = |x: &str| !x.is_empty() && x.bytes().all(|c| c.is_ascii_digit());
    (digits(a) && digits(b)).then(|| s.to_string())
}

/// The part of a URL after `http://` or `https://` (the scheme in any case).
fn after_scheme(url: &str) -> Option<&str> {
    for scheme in ["https://", "http://"] {
        if url.len() >= scheme.len() && url.is_char_boundary(scheme.len()) && url[..scheme.len()].eq_ignore_ascii_case(scheme) {
            return Some(&url[scheme.len()..]);
        }
    }
    None
}

pub fn parse(url: &str) -> Option<AppLink> {
    let rest = after_scheme(url.trim())?;
    let rest = rest.split('#').next()?;
    let (before_query, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (host, path) = before_query.split_once('/')?;
    let host = host.to_ascii_lowercase();
    let label = host.strip_suffix(".slack.com")?;
    if label.is_empty() || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-') {
        return None;
    }
    let mut seg: Vec<&str> = path.split('/').collect();
    if seg.last() == Some(&"") {
        seg.pop();
    }
    let query_thread = || {
        query.split('&').filter_map(|kv| kv.split_once('=')).find(|(k, _)| *k == "thread_ts").and_then(|(_, v)| plain_ts(v))
    };
    if host == "app.slack.com" {
        let team = match seg.as_slice() {
            ["client", t, ..] => id(t)?,
            _ => return None,
        };
        let target = match &seg[2..] {
            [ch] => Target::Conversation { channel: id(ch)? },
            [ch, "thread", ct] => {
                let channel = id(ch)?;
                let (c, ts) = ct.split_once('-')?;
                if c != channel {
                    return None;
                }
                let ts = plain_ts(ts)?;
                Target::Message { channel, ts: ts.clone(), thread_ts: Some(ts) }
            }
            _ => return None,
        };
        return Some(AppLink { team: Team::Id(team), target });
    }
    let target = match seg.as_slice() {
        ["archives", ch] => Target::Conversation { channel: id(ch)? },
        ["archives", ch, p] => Target::Message { channel: id(ch)?, ts: permalink_ts(p)?, thread_ts: query_thread() },
        ["team", u] => Target::Person { user: id(u)? },
        ["files", u, f, ..] => {
            id(u)?;
            Target::File { file: id(f)? }
        }
        _ => return None,
    };
    Some(AppLink { team: Team::Host(host), target })
}

fn host_of(url: &str) -> String {
    let rest = after_scheme(url.trim()).unwrap_or(url.trim());
    rest.split(['/', '?', '#']).next().unwrap_or("").to_ascii_lowercase()
}

/// The workspace a link points into: `workspaces` is (url, team id) per workspace; `shown` wins when
/// several match (two accounts of one team).
pub fn workspace_of(link: &AppLink, workspaces: &[(String, String)], shown: usize) -> Option<usize> {
    let matches = |i: usize| match &link.team {
        Team::Host(h) => host_of(&workspaces[i].0) == *h,
        Team::Id(t) => workspaces[i].1 == *t,
    };
    let all: Vec<usize> = (0..workspaces.len()).filter(|&i| matches(i)).collect();
    if all.contains(&shown) { Some(shown) } else { all.first().copied() }
}

/// Who wrote linked messages, by (channel, ts): what a link to a message is labelled with (filled in Z5).
#[derive(Debug, Default)]
pub struct Authors(std::collections::BTreeMap<(String, String), String>);

impl Authors {
    pub const fn new() -> Authors {
        Authors(std::collections::BTreeMap::new())
    }

    pub fn get(&self, channel: &str, ts: &str) -> Option<&str> {
        self.0.get(&(channel.to_string(), ts.to_string())).map(String::as_str)
    }

    #[cfg(test)]
    pub fn insert(&mut self, channel: &str, ts: &str, name: &str) {
        self.0.insert((channel.into(), ts.into()), name.into());
    }
}

/// None known.
static NO_AUTHORS: Authors = Authors::new();

/// Where links are drawn: this workspace, the profile's workspaces ((url, team id) each, `names` their rail labels,
/// `shown` this one's index), what is known of linked messages, and the message the link is in (its link previews and
/// files name what it points to).
#[derive(Clone, Copy)]
pub struct Here<'a> {
    pub ws: &'a Workspace,
    pub workspaces: &'a [(String, String)],
    pub names: &'a [String],
    pub shown: usize,
    pub authors: &'a Authors,
    pub message: Option<&'a Message>,
}

impl<'a> Here<'a> {
    /// This workspace alone, which no link matches: every link is drawn as an external one.
    pub fn alone(ws: &'a Workspace) -> Here<'a> {
        Here { ws, workspaces: &[], names: &[], shown: 0, authors: &NO_AUTHORS, message: None }
    }

    /// The same, for links in message `m`.
    pub fn of(self, m: &'a Message) -> Here<'a> {
        Here { message: Some(m), ..self }
    }
}

/// The status line's note while the mouse is over a link: where an in-app one goes, or the address of an outside one.
pub fn hover_note(url: &str, app: Option<&AppLink>, here: &Here) -> String {
    match app.and_then(|a| label(a, here, None)) {
        Some(readable) => format!("↪ opens {readable} in klack"),
        None => url.trim_start_matches("https://").trim_start_matches("http://").trim_start_matches("mailto:").to_string(),
    }
}

/// What a link into the profile's workspaces is shown as (after `↪ `): the author's own words for it
/// (`author_text`), else where it goes (`#deploys › message from Bob`); None when it is not in-app.
pub fn label(link: &AppLink, here: &Here, author_text: Option<&str>) -> Option<String> {
    let at = workspace_of(link, here.workspaces, here.shown)?;
    if let Some(words) = author_text.map(str::trim).filter(|w| !w.is_empty()) {
        return Some(words.to_string());
    }
    let ws = here.ws;
    let readable = match &link.target {
        Target::Conversation { channel } => conversation(ws, channel),
        Target::Message { channel, ts, thread_ts } => {
            let author = here.authors.get(channel, ts).map(str::to_string).or_else(|| preview_author(here, link));
            let what = match (thread_ts.as_deref().map(|t| t == ts), author) {
                (None, None) => "message".to_string(),
                (None, Some(a)) => format!("message from {a}"),
                (Some(false), None) => "thread reply".to_string(),
                (Some(false), Some(a)) => format!("reply from {a}"),
                // The thread itself (the desktop app's thread link).
                (Some(true), None) => "thread".to_string(),
                (Some(true), Some(a)) => format!("thread from {a}"),
            };
            format!("{} › {what}", conversation(ws, channel))
        }
        Target::Person { user } => known_user(ws, user).map_or_else(|| "someone's profile".to_string(), |n| format!("{n}'s profile")),
        Target::File { file } => {
            let carried = here.message.and_then(|m| m.files.iter().find(|f| f.id == *file));
            carried.map(|f| if f.title.is_empty() { &f.name } else { &f.title }).filter(|n| !n.is_empty()).map_or_else(|| "a file".to_string(), |n| format!("file {n}"))
        }
    };
    if at == here.shown {
        return Some(readable);
    }
    let name = here.names.get(at).map_or("another workspace", String::as_str);
    Some(format!("{name} · {readable}"))
}

/// A person's name when this workspace knows them (never their id).
fn known_user(ws: &Workspace, id: &str) -> Option<String> {
    ws.users.get(id).map(|u| u.display().to_string()).filter(|n| !n.is_empty())
}

/// A conversation as a link label: `#deploys`, `DM with Bob`, `group DM`; one this workspace does not know is
/// `a channel` (`a DM`).
fn conversation(ws: &Workspace, channel: &str) -> String {
    match ws.conversations.get(channel) {
        Some(c) if c.is_im => c.user.as_deref().and_then(|u| known_user(ws, u)).map_or_else(|| "a DM".to_string(), |n| format!("DM with {n}")),
        Some(c) if c.is_mpim => "group DM".to_string(),
        Some(c) if !c.name.is_empty() => format!("#{}", c.name),
        _ if channel.starts_with('D') => "a DM".to_string(),
        _ => "a channel".to_string(),
    }
}

/// The author of the linked message as the message's own link preview of it names them.
fn preview_author(here: &Here, link: &AppLink) -> Option<String> {
    let Target::Message { channel, ts, .. } = &link.target else { return None };
    let same = |url: &str| parse(url).is_some_and(|l| l.team == link.team && l.target.channel() == Some(channel) && matches!(&l.target, Target::Message { ts: t, .. } if t == ts));
    let preview = here.message?.attachments.iter().find(|a| ["from_url", "original_url"].iter().any(|k| a[*k].as_str().is_some_and(same)))?;
    preview["author_name"].as_str().map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).or_else(|| preview["author_id"].as_str().and_then(|u| known_user(here.ws, u)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(h: &str, target: Target) -> Option<AppLink> {
        Some(AppLink { team: Team::Host(h.into()), target })
    }
    fn conv(c: &str) -> Target {
        Target::Conversation { channel: c.into() }
    }
    fn msg(c: &str, ts: &str, th: Option<&str>) -> Target {
        Target::Message { channel: c.into(), ts: ts.into(), thread_ts: th.map(String::from) }
    }

    #[test]
    fn parses_every_form() {
        assert_eq!(parse("https://acme.slack.com/archives/C0123"), host("acme.slack.com", conv("C0123")));
        assert_eq!(parse("https://acme.slack.com/archives/C0123/p1729440000123456"), host("acme.slack.com", msg("C0123", "1729440000.123456", None)));
        assert_eq!(
            parse("https://acme.slack.com/archives/C0123/p1729440000123456?thread_ts=1729430000.000100&cid=C0123"),
            host("acme.slack.com", msg("C0123", "1729440000.123456", Some("1729430000.000100")))
        );
        assert_eq!(
            parse("https://app.slack.com/client/T0001/C0123/thread/C0123-1729430000.000100"),
            Some(AppLink { team: Team::Id("T0001".into()), target: msg("C0123", "1729430000.000100", Some("1729430000.000100")) })
        );
        assert_eq!(parse("https://app.slack.com/client/T0001/C0123"), Some(AppLink { team: Team::Id("T0001".into()), target: conv("C0123") }));
        assert_eq!(parse("https://acme.slack.com/team/U0001"), host("acme.slack.com", Target::Person { user: "U0001".into() }));
        assert_eq!(parse("https://acme.slack.com/files/U0001/F0001/report.pdf"), host("acme.slack.com", Target::File { file: "F0001".into() }));
        assert_eq!(parse("HTTPS://ACME.SLACK.COM/archives/C0123/"), host("acme.slack.com", conv("C0123")));
    }

    #[test]
    fn odd_shapes_are_accepted() {
        assert_eq!(parse("http://acme.slack.com/archives/C0123"), host("acme.slack.com", conv("C0123")));
        assert_eq!(parse("https://acme.slack.com/archives/C0123?foo=1&bar=2"), host("acme.slack.com", conv("C0123")));
        assert_eq!(
            parse("https://acme.slack.com/archives/C0123/p1729440000123456/?x=1&thread_ts=1729430000.000100&y=2"),
            host("acme.slack.com", msg("C0123", "1729440000.123456", Some("1729430000.000100")))
        );
        assert_eq!(parse("https://acme.slack.com/archives/C0123#frag"), host("acme.slack.com", conv("C0123")));
    }

    #[test]
    fn rejects_what_is_not_a_slack_link() {
        for u in [
            "https://example.com/archives/C1",
            "https://acme.slack.com/help",
            "https://acme.slack.com/archives/",
            "https://acme.slack.com/archives/C1/p12",
            "https://acme.slack.com/archives/C1/p172944000012345",
            "https://acme.slack.com/archives/C1/p17294400001234ab",
            "https://acme.slack.com/archives/C1/p1729440000123456/extra",
            "https://acme.slack.com",
            "https://slack.com/archives/C1",
            "https://evilslack.com/archives/C1",
            "https://acme.slack.com.evil.com/archives/C1",
            "https://app.slack.com/client/T1",
            "mailto:x@slack.com",
            "ftp://acme.slack.com/archives/C1",
            "",
            "https://",
            "é",
        ] {
            assert_eq!(parse(u), None, "{u}");
        }
    }

    fn ws() -> Vec<(String, String)> {
        vec![("https://acme.slack.com/".into(), "T1".into()), ("https://other.slack.com".into(), "T2".into()), ("https://acme.slack.com/".into(), "T1".into())]
    }

    #[test]
    fn matches_the_workspace() {
        let by_host = parse("https://other.slack.com/archives/C1").unwrap();
        assert_eq!(workspace_of(&by_host, &ws(), 0), Some(1));
        let by_id = parse("https://app.slack.com/client/T2/C1").unwrap();
        assert_eq!(workspace_of(&by_id, &ws(), 0), Some(1));
        let none = parse("https://nobody.slack.com/archives/C1").unwrap();
        assert_eq!(workspace_of(&none, &ws(), 0), None);
        let none_id = parse("https://app.slack.com/client/T9/C1").unwrap();
        assert_eq!(workspace_of(&none_id, &ws(), 0), None);
        let twice = parse("https://acme.slack.com/archives/C1").unwrap();
        assert_eq!(workspace_of(&twice, &ws(), 2), Some(2));
        assert_eq!(workspace_of(&twice, &ws(), 0), Some(0));
        assert_eq!(workspace_of(&twice, &ws(), 1), Some(0), "shown is not a match: the first match");
        assert_eq!(workspace_of(&twice, &[], 0), None);
    }

    #[test]
    fn converts_a_notification_target() {
        let n = super::super::notify::Target { channel: "C1".into(), ts: Some("1.0".into()), thread_ts: Some("0.5".into()) };
        assert_eq!(Target::from(n), msg("C1", "1.0", Some("0.5")));
        let n = super::super::notify::Target { channel: "C1".into(), ts: None, thread_ts: None };
        let t = Target::from(n);
        assert_eq!(t, conv("C1"));
        assert_eq!(t.channel(), Some("C1"));
        assert_eq!(Target::Person { user: "U1".into() }.channel(), None);
    }

    #[test]
    fn in_app_links_are_labelled() {
        use slack_core::types::{Conversation, File, User};
        let mut ws = Workspace::default();
        let user = |id: &str, name: &str| User { id: id.into(), name: name.into(), ..Default::default() };
        ws.users.insert("U1".into(), user("U1", "ann"));
        ws.users.insert("U2".into(), user("U2", "bob"));
        ws.conversations.insert("C1".into(), Conversation { id: "C1".into(), name: "deploys".into(), is_channel: true, ..Default::default() });
        ws.conversations.insert("D1".into(), Conversation { id: "D1".into(), is_im: true, user: Some("U2".into()), ..Default::default() });
        ws.conversations.insert("G1".into(), Conversation { id: "G1".into(), name: "mpdm-ann--bob--cy-1".into(), is_mpim: true, ..Default::default() });
        let workspaces = vec![("https://acme.slack.com/".to_string(), "T1".to_string()), ("https://beta.slack.com/".to_string(), "T2".to_string())];
        let names = vec!["acme".to_string(), "beta".to_string()];
        let mut authors = Authors::new();
        authors.insert("C1", "1729440000.000200", "Cy");
        let here = Here { ws: &ws, workspaces: &workspaces, names: &names, shown: 0, authors: &authors, message: None };
        let l = |url: &str, here: &Here| label(&parse(url).unwrap(), here, None);
        let a = "https://acme.slack.com";
        assert_eq!(l(&format!("{a}/archives/C1"), &here).as_deref(), Some("#deploys"));
        assert_eq!(l(&format!("{a}/archives/D1"), &here).as_deref(), Some("DM with bob"));
        assert_eq!(l(&format!("{a}/archives/G1"), &here).as_deref(), Some("group DM"));
        assert_eq!(l(&format!("{a}/archives/C1/p1729440000000100"), &here).as_deref(), Some("#deploys › message"));
        assert_eq!(l(&format!("{a}/archives/D1/p1729440000000100"), &here).as_deref(), Some("DM with bob › message"));
        assert_eq!(l(&format!("{a}/archives/G1/p1729440000000100"), &here).as_deref(), Some("group DM › message"));
        assert_eq!(l(&format!("{a}/archives/C1/p1729440000000200"), &here).as_deref(), Some("#deploys › message from Cy"), "the author is known");
        assert_eq!(l(&format!("{a}/archives/C1/p1729440000000300?thread_ts=1729440000.000100&cid=C1"), &here).as_deref(), Some("#deploys › thread reply"));
        assert_eq!(l(&format!("{a}/archives/C1/p1729440000000200?thread_ts=1729440000.000100&cid=C1"), &here).as_deref(), Some("#deploys › reply from Cy"));
        assert_eq!(l("https://app.slack.com/client/T1/C1/thread/C1-1729440000.000100", &here).as_deref(), Some("#deploys › thread"), "the desktop app's thread link: the thread, not a reply");
        assert_eq!(l("https://app.slack.com/client/T1/C1", &here).as_deref(), Some("#deploys"));
        assert_eq!(l(&format!("{a}/team/U2"), &here).as_deref(), Some("bob's profile"));
        assert_eq!(l(&format!("{a}/team/U9"), &here).as_deref(), Some("someone's profile"), "never the id");
        assert_eq!(l(&format!("{a}/files/U2/F1/report.pdf"), &here).as_deref(), Some("a file"));
        // Unknown details fall back, never an id.
        assert_eq!(l(&format!("{a}/archives/C9"), &here).as_deref(), Some("a channel"));
        assert_eq!(l(&format!("{a}/archives/C9/p1729440000000100"), &here).as_deref(), Some("a channel › message"));
        assert_eq!(l(&format!("{a}/archives/D9"), &here).as_deref(), Some("a DM"));
        // Another workspace of the profile: its name first; a channel only when this one knows it.
        assert_eq!(l("https://beta.slack.com/archives/C7/p1729440000000100", &here).as_deref(), Some("beta · a channel › message"));
        assert_eq!(l("https://beta.slack.com/archives/C1", &here).as_deref(), Some("beta · #deploys"));
        assert_eq!(l("https://app.slack.com/client/T2/C7", &here).as_deref(), Some("beta · a channel"));
        assert_eq!(l("https://acme.slack.com/archives/C1", &Here { shown: 1, ..here }).as_deref(), Some("acme · #deploys"));
        // The author's own words, with the arrow still drawn before them.
        assert_eq!(label(&parse(&format!("{a}/archives/C1")).unwrap(), &here, Some("the fix")).as_deref(), Some("the fix"));
        assert_eq!(label(&parse(&format!("{a}/archives/C1")).unwrap(), &here, Some(" ")).as_deref(), Some("#deploys"), "blank words are none");
        // The message's own link preview names the author; the file it carries names the file.
        let link = format!("{a}/archives/C1/p1729440000000500");
        let mut m = Message { ts: "9.0".into(), text: format!("<{link}>"), ..Default::default() };
        m.attachments = vec![serde_json::json!({"from_url": "https://example.com/x", "author_name": "Not me"}), serde_json::json!({"from_url": link, "author_name": "Bob", "is_msg_unfurl": true})];
        m.files = vec![File { id: "F1".into(), name: "report.pdf".into(), ..Default::default() }];
        assert_eq!(l(&link, &here.of(&m)).as_deref(), Some("#deploys › message from Bob"));
        assert_eq!(l(&format!("{a}/files/U2/F1/report.pdf"), &here.of(&m)).as_deref(), Some("file report.pdf"));
        m.attachments = vec![serde_json::json!({"original_url": link, "author_id": "U1"})];
        assert_eq!(l(&link, &here.of(&m)).as_deref(), Some("#deploys › message from ann"), "the author's id, by name");
        m.attachments = vec![serde_json::json!({"original_url": link, "author_id": "U9"})];
        assert_eq!(l(&link, &here.of(&m)).as_deref(), Some("#deploys › message"), "an unknown author: no id");
        // Not in-app: another Slack, or not Slack at all.
        assert_eq!(l("https://nobody.slack.com/archives/C1", &here), None);
        assert_eq!(l("https://app.slack.com/client/T9/C1", &here), None);
        assert_eq!(l("https://acme.slack.com/archives/C1", &Here::alone(&ws)), None);
    }
}

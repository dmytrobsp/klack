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

/// What is known of a linked message's author.
#[derive(Debug, Clone, PartialEq)]
enum Author {
    Known(String),
    /// Asked for, or waiting for a turn.
    Pending,
    /// Deleted, no access or no name: the label stays plain.
    Missing,
}

/// How many authors are asked for at once.
pub const AUTHORS_IN_FLIGHT: usize = 3;

/// How many links wait for a turn at most: beyond that the one drawn longest ago is dropped (asked for again when it
/// is drawn again).
pub const AUTHORS_QUEUED: usize = 20;

/// How long nothing is asked after a request failed (the network, or Slack's rate limits).
pub const AUTHORS_PAUSE: std::time::Duration = std::time::Duration::from_secs(10);

/// Who wrote linked messages, by channel, then ts (looked up by `&str`, as it is on the draw path): what a link to a
/// message is labelled with. Only links on screen are asked for, a few at a time, each once per run when Slack answers.
#[derive(Debug, Default)]
pub struct Authors {
    by_channel: std::collections::HashMap<String, std::collections::HashMap<String, Author>>,
    /// (channel, ts, thread_ts) waiting for a turn, the one drawn longest ago first.
    queue: std::collections::VecDeque<(String, String, Option<String>)>,
    in_flight: usize,
    /// Nothing is asked before this (a request failed).
    paused_until: Option<std::time::Instant>,
    /// How many answers came (a link's label may have changed with each).
    answered: u64,
}

impl Authors {
    pub fn new() -> Authors {
        Authors::default()
    }

    /// How many answers came so far.
    pub fn answered(&self) -> u64 {
        self.answered
    }

    pub fn get(&self, channel: &str, ts: &str) -> Option<&str> {
        match self.by_channel.get(channel)?.get(ts)? {
            Author::Known(name) => Some(name),
            _ => None,
        }
    }

    /// A link to this message is on screen: queue it unless it was asked for already (a queued one moves to the back,
    /// as drawn last).
    pub fn want(&mut self, channel: &str, ts: &str, thread_ts: Option<&str>) {
        match self.by_channel.get(channel).and_then(|m| m.get(ts)) {
            Some(Author::Pending) => {
                if let Some(entry) = self.queue.iter().position(|(c, t, _)| c == channel && t == ts).and_then(|at| self.queue.remove(at)) {
                    self.queue.push_back(entry);
                }
            }
            Some(_) => {}
            None => {
                self.by_channel.entry(channel.to_string()).or_default().insert(ts.to_string(), Author::Pending);
                self.queue.push_back((channel.to_string(), ts.to_string(), thread_ts.map(str::to_string)));
                while self.queue.len() > AUTHORS_QUEUED {
                    if let Some((c, t, _)) = self.queue.pop_front() {
                        self.forget(&c, &t);
                    }
                }
            }
        }
    }

    /// The links to ask for now: queued ones that are `drawn` this frame ((channel, ts, thread_ts) each), up to the
    /// limit in flight; the rest stay queued. None while paused after a failure.
    pub fn next_requests(&mut self, drawn: &[(String, String, Option<String>)], now: std::time::Instant) -> Vec<(String, String, Option<String>)> {
        let mut out = Vec::new();
        if self.paused_until.is_some_and(|until| now < until) {
            return out;
        }
        let mut n = 0;
        while self.in_flight < AUTHORS_IN_FLIGHT && n < self.queue.len() {
            let (c, t, _) = &self.queue[n];
            if !drawn.iter().any(|(dc, dt, _)| dc == c && dt == t) {
                n += 1;
                continue;
            }
            if let Some(r) = self.queue.remove(n) {
                self.in_flight += 1;
                out.push(r);
            }
        }
        out
    }

    /// Slack's answer for a link (None: no such message, or no name for it).
    pub fn finish(&mut self, channel: &str, ts: &str, name: Option<String>) {
        self.in_flight = self.in_flight.saturating_sub(1);
        self.answered += 1;
        let state = name.filter(|n| !n.is_empty()).map_or(Author::Missing, Author::Known);
        self.by_channel.entry(channel.to_string()).or_default().insert(ts.to_string(), state);
    }

    /// Asking for a link failed (the network, or Slack's rate limits): not known either way, so it is asked for again
    /// when it is drawn, after a pause.
    pub fn fail(&mut self, channel: &str, ts: &str, now: std::time::Instant) {
        self.in_flight = self.in_flight.saturating_sub(1);
        self.paused_until = Some(now + AUTHORS_PAUSE);
        if self.by_channel.get(channel).and_then(|m| m.get(ts)) == Some(&Author::Pending) {
            self.forget(channel, ts);
        }
    }

    fn forget(&mut self, channel: &str, ts: &str) {
        if let Some(m) = self.by_channel.get_mut(channel) {
            m.remove(ts);
            if m.is_empty() {
                self.by_channel.remove(channel);
            }
        }
    }

    #[cfg(test)]
    pub fn insert(&mut self, channel: &str, ts: &str, name: &str) {
        self.answered += 1;
        self.by_channel.entry(channel.into()).or_default().insert(ts.into(), Author::Known(name.into()));
    }
}

/// Whether the author of the message `link` points to is worth asking Slack for: a link to a message whose own
/// message's link preview does not name them already (`here.message`: the message the link is in).
pub fn wants_author(link: &AppLink, here: &Here) -> bool {
    matches!(link.target, Target::Message { .. }) && preview_author(here, link).is_none()
}

/// The author's own words for a link, `label`, unless it is only the address again (Slack's label for a pasted link;
/// in blocks the address has `&amp;` where the label has `&`), or empty.
pub fn own_words<'a>(url: &str, label: &'a str) -> Option<&'a str> {
    let bare = |s: &str| slack_core::format::unescape(s.trim()).trim_start_matches("https://").trim_start_matches("http://").to_string();
    (!label.trim().is_empty() && bare(label) != bare(url)).then_some(label)
}

/// `s` without the characters that reorder text (bidi embeddings, overrides, isolates and marks), so a name or label
/// can't make what follows it read backwards.
pub fn without_bidi(s: &str) -> String {
    s.chars().filter(|c| !matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}')).collect()
}

/// The name of whoever wrote `m`, when this workspace knows it (never an id).
pub fn author_name(ws: &Workspace, m: &Message) -> Option<String> {
    let named = |n: &str| Some(without_bidi(n).trim().to_string()).filter(|n| !n.is_empty());
    match m.user.as_deref() {
        Some(u) => known_user(ws, u).and_then(|n| named(&n)),
        None => m.bot_profile.as_ref().and_then(|b| b["name"].as_str()).and_then(named).or_else(|| m.username.as_deref().and_then(named)),
    }
}

/// None known.
static NO_AUTHORS: std::sync::LazyLock<Authors> = std::sync::LazyLock::new(Authors::new);

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
        Here { ws, workspaces: &[], names: &[], shown: 0, authors: &*NO_AUTHORS, message: None }
    }

    /// The same, for links in message `m`.
    pub fn of(self, m: &'a Message) -> Here<'a> {
        Here { message: Some(m), ..self }
    }
}

/// The hover note of an in-app link (`{}`: where it goes).
pub const OPENS_IN_TTYAK: &str = "opens {} in ttyak";

/// The status line's note while the mouse is over a link: where an in-app one goes, or the address of an outside one.
pub fn hover_note(url: &str, app: Option<&AppLink>, here: &Here) -> String {
    match app.and_then(|a| label(a, here, None)) {
        Some(readable) => format!("↪ {}", super::i18n::tr(OPENS_IN_TTYAK).replace("{}", &readable)),
        None => without_bidi(url.trim_start_matches("https://").trim_start_matches("http://").trim_start_matches("mailto:")),
    }
}

/// What a link into the profile's workspaces is shown as (after `↪ `): the author's own words for it
/// (`author_text`), else where it goes (`#deploys › message from Bob`); None when it is not in-app.
/// Characters that reorder text are taken out.
pub fn label(link: &AppLink, here: &Here, author_text: Option<&str>) -> Option<String> {
    readable(link, here, author_text).map(|l| without_bidi(&l))
}

fn readable(link: &AppLink, here: &Here, author_text: Option<&str>) -> Option<String> {
    let at = workspace_of(link, here.workspaces, here.shown)?;
    if let Some(words) = author_text.map(str::trim).filter(|w| !without_bidi(w).trim().is_empty()) {
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

    fn drawn(links: &[&str]) -> Vec<(String, String, Option<String>)> {
        links.iter().map(|ts| ("C1".to_string(), ts.to_string(), None)).collect()
    }

    #[test]
    fn authors_are_fetched_once_and_few_at_a_time() {
        let now = std::time::Instant::now();
        let mut a = Authors::new();
        let all: Vec<String> = (0..10).map(|i| format!("1.{i}")).collect();
        let on_screen = drawn(&all.iter().map(String::as_str).collect::<Vec<_>>());
        for i in 0..10 {
            a.want("C1", &format!("1.{i}"), None);
            a.want("C1", &format!("1.{i}"), None);
        }
        let first = a.next_requests(&on_screen, now);
        assert_eq!(first.len(), 3);
        assert!(a.next_requests(&on_screen, now).is_empty(), "three in flight: the rest wait");
        a.finish("C1", &first[0].1, Some("Bob".into()));
        assert_eq!(a.get("C1", &first[0].1), Some("Bob"));
        let more = a.next_requests(&on_screen, now);
        assert_eq!(more.len(), 1);
        assert!(!first.contains(&more[0]));
        // Asked for once per run: an answered or missing link is never queued again.
        a.want("C1", &first[0].1, None);
        a.finish("C1", &first[1].1, None);
        a.want("C1", &first[1].1, None);
        let mut seen: Vec<_> = first.iter().chain(&more).map(|r| r.1.clone()).collect();
        while a.in_flight > 0 {
            a.finish("C1", "x", None);
            seen.extend(a.next_requests(&on_screen, now).into_iter().map(|r| r.1));
        }
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 10, "each of the ten asked exactly once");
    }

    #[test]
    fn only_links_on_screen_are_asked_for() {
        let now = std::time::Instant::now();
        let mut a = Authors::new();
        for ts in ["1.1", "1.2", "1.3", "1.4"] {
            a.want("C1", ts, None);
        }
        // Scrolled away: they stay queued, never sent.
        assert!(a.next_requests(&[], now).is_empty(), "nothing on screen: nothing asked");
        let asked = a.next_requests(&drawn(&["1.3"]), now);
        assert_eq!(asked.iter().map(|r| r.1.as_str()).collect::<Vec<_>>(), ["1.3"], "only the one drawn now");
        assert!(a.next_requests(&drawn(&["1.3"]), now).is_empty(), "asked once");
        // Drawn again later: its turn comes.
        assert_eq!(a.next_requests(&drawn(&["1.1", "1.4"]), now).len(), 2);
    }

    #[test]
    fn the_queue_is_capped() {
        let now = std::time::Instant::now();
        let mut a = Authors::new();
        let ts = |i: usize| format!("2.{i:03}");
        for i in 0..AUTHORS_QUEUED + 5 {
            a.want("C1", &ts(i), None);
        }
        assert_eq!(a.queue.len(), AUTHORS_QUEUED);
        assert!(a.next_requests(&drawn(&[&ts(0), &ts(4)]), now).is_empty(), "the oldest were dropped");
        assert_eq!(a.next_requests(&drawn(&[&ts(5)]), now).len(), 1, "the newer kept");
        // A dropped one is unknown again: wanted when drawn again.
        a.want("C1", &ts(0), None);
        assert_eq!(a.next_requests(&drawn(&[&ts(0)]), now).len(), 1);
        // One drawn again moves to the back, so it outlives those drawn before it.
        let mut b = Authors::new();
        for i in 0..AUTHORS_QUEUED {
            b.want("C1", &ts(i), None);
        }
        b.want("C1", &ts(0), None);
        b.want("C1", &ts(100), None);
        assert_eq!(b.next_requests(&drawn(&[&ts(0), &ts(1)]), now).iter().map(|r| r.1.clone()).collect::<Vec<_>>(), [ts(0)], "1 was the oldest drawn");
    }

    #[test]
    fn a_failed_request_is_asked_again_later() {
        let now = std::time::Instant::now();
        let mut a = Authors::new();
        a.want("C1", "1.1", None);
        a.want("C1", "1.2", None);
        let asked = a.next_requests(&drawn(&["1.1"]), now);
        assert_eq!(asked.len(), 1);
        a.fail("C1", "1.1", now);
        assert_eq!(a.in_flight, 0);
        assert_eq!(a.get("C1", "1.1"), None);
        a.want("C1", "1.1", None);
        assert!(a.next_requests(&drawn(&["1.1", "1.2"]), now).is_empty(), "a pause after a failure");
        let later = now + AUTHORS_PAUSE;
        assert_eq!(a.next_requests(&drawn(&["1.1"]), later).len(), 1, "unknown again, not missing: wanted again");
        // Slack's own answer that there is no such message is kept.
        a.finish("C1", "1.1", None);
        a.want("C1", "1.1", None);
        assert!(a.next_requests(&drawn(&["1.1"]), later).is_empty());
    }

    #[test]
    fn a_previewed_link_never_wants_its_author() {
        let ws = Workspace::default();
        let workspaces = vec![("https://acme.slack.com/".to_string(), "T1".to_string())];
        let names = vec!["acme".to_string()];
        let authors = Authors::new();
        let here = Here { ws: &ws, workspaces: &workspaces, names: &names, shown: 0, authors: &authors, message: None };
        let url = "https://acme.slack.com/archives/C1/p1729440000000500";
        let link = parse(url).unwrap();
        let mut m = Message { ts: "9.0".into(), text: format!("<{url}>"), ..Default::default() };
        assert!(wants_author(&link, &here.of(&m)), "no preview: asked for");
        m.attachments = vec![serde_json::json!({"from_url": url, "author_name": "Bob", "is_msg_unfurl": true})];
        assert!(!wants_author(&link, &here.of(&m)), "the preview names the author");
        assert!(!wants_author(&parse("https://acme.slack.com/archives/C1").unwrap(), &here), "a conversation has no author");
    }

    #[test]
    fn the_author_s_own_words() {
        assert_eq!(own_words("http://acme.slack.com/archives/C1", "acme.slack.com/archives/C1"), None, "the address again, without its scheme");
        assert_eq!(own_words("https://x.y/?a=1&amp;b=2", "https://x.y/?a=1&b=2"), None, "escaped once in blocks");
        assert_eq!(own_words("https://x.y/a", " "), None);
        assert_eq!(own_words("https://x.y/a", "the fix"), Some("the fix"));
    }

    #[test]
    fn bidi_controls_are_taken_out() {
        assert_eq!(without_bidi("a\u{202E}b\u{2066}c\u{2069}d\u{200F}e\u{200E}f\u{202A}"), "abcdef");
        let mut ws = Workspace::default();
        ws.users.insert("U1".into(), slack_core::types::User { id: "U1".into(), name: "e\u{202E}vil".into(), ..Default::default() });
        let m = Message { user: Some("U1".into()), ..Default::default() };
        assert_eq!(author_name(&ws, &m).as_deref(), Some("evil"));
        let bot = Message { username: Some("\u{2067}bot".into()), ..Default::default() };
        assert_eq!(author_name(&ws, &bot).as_deref(), Some("bot"));
        let workspaces = vec![("https://acme.slack.com/".to_string(), "T1".to_string())];
        let names = vec!["acme".to_string()];
        let mut authors = Authors::new();
        authors.insert("C1", "1729440000.000200", "C\u{202E}y");
        let here = Here { ws: &ws, workspaces: &workspaces, names: &names, shown: 0, authors: &authors, message: None };
        let link = parse("https://acme.slack.com/archives/C1/p1729440000000200").unwrap();
        assert_eq!(label(&link, &here, None).as_deref(), Some("a channel › message from Cy"));
        assert_eq!(label(&link, &here, Some("\u{202E}xif eht")).as_deref(), Some("xif eht"), "the author's words too");
        assert_eq!(hover_note("https://x.y/\u{202E}a", None, &here), "x.y/a");
    }

    #[test]
    fn a_missing_message_keeps_the_plain_label() {
        let ws = Workspace::default();
        let workspaces = vec![("https://acme.slack.com/".to_string(), "T1".to_string())];
        let names = vec!["acme".to_string()];
        let link = parse("https://acme.slack.com/archives/C1/p1729440000000200").unwrap();
        let mut authors = Authors::new();
        let on_screen = vec![("C1".to_string(), "1729440000.000200".to_string(), None)];
        authors.want("C1", "1729440000.000200", None);
        authors.next_requests(&on_screen, std::time::Instant::now());
        authors.finish("C1", "1729440000.000200", None);
        let here = Here { ws: &ws, workspaces: &workspaces, names: &names, shown: 0, authors: &authors, message: None };
        assert_eq!(label(&link, &here, None).as_deref(), Some("a channel › message"));
        authors.want("C1", "1729440000.000200", None);
        assert!(authors.next_requests(&on_screen, std::time::Instant::now()).is_empty(), "a missing message is not asked for again");
    }

    #[test]
    fn an_author_without_a_name_is_not_labelled_with_an_id() {
        let mut ws = Workspace::default();
        let user: slack_core::types::User = serde_json::from_value(serde_json::json!({"id": "U9"})).unwrap();
        ws.users.insert("U9".into(), user);
        let mut m = Message::default();
        m.user = Some("U9".into());
        assert_eq!(author_name(&ws, &m), None, "every name empty: no id in the label");
        m.user = Some("U8".into());
        assert_eq!(author_name(&ws, &m), None, "an unknown user");
        m.user = None;
        m.username = Some("deploybot".into());
        assert_eq!(author_name(&ws, &m).as_deref(), Some("deploybot"));
    }
}

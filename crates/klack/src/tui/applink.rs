//! Slack links that point into a workspace this profile has open: parsing and matching.
// Z2 and Z3 use this; Z2 removes the allow.
#![cfg_attr(not(test), allow(dead_code))]

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
}

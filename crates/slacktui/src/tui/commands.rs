//! Slash commands typed into a composer. Anything not recognised is sent as text.

use super::files::{UploadCommand, parse_upload};

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Upload(UploadCommand),
    /// Join the named channel, or the one being previewed.
    Join(Option<String>),
    Leave,
    Create { name: String, private: bool },
    /// Open a DM, or a group DM with several people.
    Dm(Vec<Recipient>),
    /// Star or unstar the current conversation.
    Star,
    /// Open the preferences.
    Prefs,
    /// List the profile's workspaces, or switch to the one matching the name.
    Workspace(Option<String>),
    /// Set your status (empty emoji and text clear it).
    Status { emoji: String, text: String, expiry: Expiry },
    /// Appear away (true) or active.
    Away(bool),
    /// Pause notifications for this many minutes; None resumes them.
    Dnd(Option<u64>),
    /// Rename the section holding the current conversation.
    SectionRename(String),
    /// Delete the section holding the current conversation.
    SectionDelete,
    /// Post the lines after `/snippet [lang] [title]` as a text snippet.
    Snippet { lang: String, title: String, body: String },
}

#[derive(Debug, PartialEq, Eq)]
pub enum Recipient {
    /// An autocompleted mention.
    Id(String),
    /// A typed handle or display name without spaces.
    Name(String),
}

/// When a status clears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expiry {
    Never,
    Minutes(u64),
    /// At the end of the local day.
    Today,
}

/// `30m`, `2h`, `1d` or `90` (minutes).
pub fn minutes(s: &str) -> Option<u64> {
    let s = s.trim().to_lowercase();
    let (n, unit) = match s.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
        Some((i, _)) => s.split_at(i),
        None => (s.as_str(), "m"),
    };
    let n: u64 = n.parse().ok().filter(|n| *n > 0)?;
    match unit.trim() {
        "m" | "min" | "mins" | "minutes" => Some(n),
        "h" | "hr" | "hour" | "hours" => Some(n * 60),
        "d" | "day" | "days" => Some(n * 1440),
        _ => None,
    }
}

/// `/status [:emoji:] [text] [for <30m|2h|1d|today>]`.
fn parse_status(rest: &str) -> Result<Command, String> {
    let usage = "usage: /status [:emoji:] [text] [for 30m|2h|1d|today] · /status clear";
    if rest.is_empty() {
        return Err(usage.into());
    }
    if rest == "clear" {
        return Ok(Command::Status { emoji: String::new(), text: String::new(), expiry: Expiry::Never });
    }
    let (body, expiry) = match rest.rsplit_once(" for ") {
        Some((body, "today")) => (body, Expiry::Today),
        Some((body, d)) => match minutes(d) {
            Some(m) => (body, Expiry::Minutes(m)),
            None => return Err(format!("cannot read the duration '{d}' ({usage})")),
        },
        None => (rest, Expiry::Never),
    };
    let body = body.trim();
    let (emoji, text) = match body.split_once(char::is_whitespace).unwrap_or((body, "")) {
        (e, t) if e.len() > 2 && e.starts_with(':') && e.ends_with(':') => (e.to_string(), t.trim()),
        _ => (String::new(), body),
    };
    if emoji.is_empty() && text.is_empty() {
        return Err(usage.into());
    }
    Ok(Command::Status { emoji: if emoji.is_empty() { ":speech_balloon:".into() } else { emoji }, text: text.to_string(), expiry })
}

/// Most people in a group DM besides yourself (Slack allows nine in all).
pub const MAX_GROUP: usize = 8;

/// Parse a composer's contents. `mrkdwn` is the same text with autocompleted mentions as
/// `<@U…>`, which `/dm` needs because display names may contain spaces.
pub fn parse(text: &str, mrkdwn: &str) -> Option<Result<Command, String>> {
    if let Some(rest) = mrkdwn.trim().strip_prefix("/dm").filter(|r| r.is_empty() || r.starts_with(char::is_whitespace)) {
        let people: Vec<Recipient> = rest
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|t| !t.is_empty())
            .map(|t| match t.strip_prefix("<@").and_then(|t| t.strip_suffix('>')) {
                Some(id) => Recipient::Id(id.to_string()),
                None => Recipient::Name(t.trim_start_matches('@').to_string()),
            })
            .collect();
        return Some(match people.len() {
            0 => Err("usage: /dm @person [@person…]".into()),
            n if n > MAX_GROUP => Err(format!("a group DM can have at most {MAX_GROUP} other people")),
            _ => Ok(Command::Dm(people)),
        });
    }
    if let Some(rest) = text.strip_prefix("/snippet").filter(|r| r.is_empty() || r.starts_with(char::is_whitespace)) {
        let (head, body) = rest.split_once('\n').unwrap_or((rest, ""));
        let (lang, title) = head.trim().split_once(char::is_whitespace).unwrap_or((head.trim(), ""));
        return Some(if body.trim().is_empty() {
            Err("usage: /snippet [lang] [title], then the snippet on the next lines (Alt+Enter)".into())
        } else {
            Ok(Command::Snippet {
                lang: if lang.is_empty() { "text".into() } else { lang.to_lowercase() },
                title: title.trim().to_string(),
                body: body.to_string(),
            })
        });
    }
    if let Some(upload) = parse_upload(text) {
        return Some(upload.map(Command::Upload));
    }
    let (word, rest) = text.trim().split_once(char::is_whitespace).unwrap_or((text.trim(), ""));
    let rest = rest.trim();
    Some(match word {
        "/join" if rest.contains(char::is_whitespace) => Err("usage: /join [#channel]".into()),
        "/join" => Ok(Command::Join(Some(rest.trim_start_matches('#').to_string()).filter(|n| !n.is_empty()))),
        "/star" if rest.is_empty() => Ok(Command::Star),
        "/prefs" | "/preferences" if rest.is_empty() => Ok(Command::Prefs),
        "/workspace" => Ok(Command::Workspace(Some(rest.to_string()).filter(|r| !r.is_empty()))),
        "/status" => parse_status(rest),
        "/away" if rest.is_empty() => Ok(Command::Away(true)),
        "/active" | "/back" if rest.is_empty() => Ok(Command::Away(false)),
        "/dnd" if rest == "off" => Ok(Command::Dnd(None)),
        "/dnd" => match minutes(rest) {
            Some(m) => Ok(Command::Dnd(Some(m))),
            None => Err("usage: /dnd 30m|2h|1d · /dnd off".into()),
        },
        "/section" => match rest.split_once(char::is_whitespace).unwrap_or((rest, "")) {
            ("rename", name) if !name.trim().is_empty() => Ok(Command::SectionRename(name.trim().to_string())),
            ("delete", "") => Ok(Command::SectionDelete),
            _ => Err("usage: /section rename <name> · /section delete (m in the sidebar moves a conversation)".into()),
        },
        "/leave" if rest.is_empty() => Ok(Command::Leave),
        "/leave" => Err("/leave leaves the current channel; it takes no arguments".into()),
        "/create" | "/create-private" => {
            let name = slack_core::types::channel_name(rest);
            if name.is_empty() {
                Err(format!("usage: {word} <name>"))
            } else {
                Ok(Command::Create { name, private: word == "/create-private" })
            }
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Option<Result<Command, String>> {
        super::parse(text, text)
    }

    #[test]
    fn dm_recipients() {
        assert_eq!(
            super::parse("/dm @Ann Lee @bob", "/dm <@U1> @bob"),
            Some(Ok(Command::Dm(vec![Recipient::Id("U1".into()), Recipient::Name("bob".into())])))
        );
        assert_eq!(parse("/dm bob,carol"), Some(Ok(Command::Dm(vec![Recipient::Name("bob".into()), Recipient::Name("carol".into())]))));
        assert!(matches!(parse("/dm"), Some(Err(_))));
        assert!(matches!(parse("/dm a b c d e f g h i"), Some(Err(_))));
        assert_eq!(parse("/dmx"), None);
    }

    #[test]
    fn snippets() {
        assert_eq!(
            parse("/snippet python my script.py\nprint(1)\n  x = 2"),
            Some(Ok(Command::Snippet { lang: "python".into(), title: "my script.py".into(), body: "print(1)\n  x = 2".into() }))
        );
        assert_eq!(parse("/snippet\nhello"), Some(Ok(Command::Snippet { lang: "text".into(), title: String::new(), body: "hello".into() })));
        assert!(matches!(parse("/snippet rust"), Some(Err(_))), "nothing to post");
        assert_eq!(parse("/snippets"), None);
    }

    #[test]
    fn availability() {
        assert_eq!(minutes("30m"), Some(30));
        assert_eq!(minutes("2h"), Some(120));
        assert_eq!(minutes("1 day"), Some(1440));
        assert_eq!(minutes("45"), Some(45));
        assert_eq!((minutes("0m"), minutes("soon")), (None, None));
        assert_eq!(parse("/status :coffee: on a break for 30m"), Some(Ok(Command::Status { emoji: ":coffee:".into(), text: "on a break".into(), expiry: Expiry::Minutes(30) })));
        assert_eq!(parse("/status in a meeting for today"), Some(Ok(Command::Status { emoji: ":speech_balloon:".into(), text: "in a meeting".into(), expiry: Expiry::Today })));
        assert_eq!(parse("/status :palm_tree:"), Some(Ok(Command::Status { emoji: ":palm_tree:".into(), text: String::new(), expiry: Expiry::Never })));
        assert_eq!(parse("/status clear"), Some(Ok(Command::Status { emoji: String::new(), text: String::new(), expiry: Expiry::Never })));
        assert!(matches!(parse("/status hi for ever"), Some(Err(_))));
        assert!(matches!(parse("/status"), Some(Err(_))));
        assert_eq!(parse("/away"), Some(Ok(Command::Away(true))));
        assert_eq!(parse("/back"), Some(Ok(Command::Away(false))));
        assert_eq!(parse("/dnd 1h"), Some(Ok(Command::Dnd(Some(60)))));
        assert_eq!(parse("/dnd off"), Some(Ok(Command::Dnd(None))));
        assert!(matches!(parse("/dnd"), Some(Err(_))));
    }

    #[test]
    fn commands() {
        assert_eq!(parse("/join"), Some(Ok(Command::Join(None))));
        assert_eq!(parse("/join #design "), Some(Ok(Command::Join(Some("design".into())))));
        assert!(matches!(parse("/join a b"), Some(Err(_))));
        assert_eq!(parse("/leave"), Some(Ok(Command::Leave)));
        assert!(matches!(parse("/leave now"), Some(Err(_))));
        assert_eq!(parse("/create Team Plans"), Some(Ok(Command::Create { name: "team-plans".into(), private: false })));
        assert_eq!(parse("/create-private x"), Some(Ok(Command::Create { name: "x".into(), private: true })));
        assert!(matches!(parse("/create  "), Some(Err(_))));
        assert!(matches!(parse("/upload a.png hi"), Some(Ok(Command::Upload(_)))));
        assert_eq!(parse("/star"), Some(Ok(Command::Star)));
        assert_eq!(parse("/workspace"), Some(Ok(Command::Workspace(None))));
        assert_eq!(parse("/workspace acme corp"), Some(Ok(Command::Workspace(Some("acme corp".into())))));
        assert_eq!(parse("/section rename  Side projects "), Some(Ok(Command::SectionRename("Side projects".into()))));
        assert_eq!(parse("/section delete"), Some(Ok(Command::SectionDelete)));
        assert!(matches!(parse("/section rename"), Some(Err(_))));
        assert!(matches!(parse("/section"), Some(Err(_))));
        assert_eq!(parse("/shrug"), None);
        assert_eq!(parse("/joinx"), None);
        assert_eq!(parse("hello /join"), None);
    }
}

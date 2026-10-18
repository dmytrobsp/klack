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
}

#[derive(Debug, PartialEq, Eq)]
pub enum Recipient {
    /// An autocompleted mention.
    Id(String),
    /// A typed handle or display name without spaces.
    Name(String),
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
    if let Some(upload) = parse_upload(text) {
        return Some(upload.map(Command::Upload));
    }
    let (word, rest) = text.trim().split_once(char::is_whitespace).unwrap_or((text.trim(), ""));
    let rest = rest.trim();
    Some(match word {
        "/join" if rest.contains(char::is_whitespace) => Err("usage: /join [#channel]".into()),
        "/join" => Ok(Command::Join(Some(rest.trim_start_matches('#').to_string()).filter(|n| !n.is_empty()))),
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
        assert_eq!(parse("/shrug"), None);
        assert_eq!(parse("/joinx"), None);
        assert_eq!(parse("hello /join"), None);
    }
}

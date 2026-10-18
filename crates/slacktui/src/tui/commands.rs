//! Slash commands typed into a composer. Anything not recognised is sent as text.

use super::files::{UploadCommand, parse_upload};

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Upload(UploadCommand),
    /// Join the named channel, or the one being previewed.
    Join(Option<String>),
    Leave,
    Create { name: String, private: bool },
}

pub fn parse(text: &str) -> Option<Result<Command, String>> {
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

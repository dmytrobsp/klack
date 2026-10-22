//! Slash commands typed into a composer. Others of the form `/name …` go to Slack (its own and
//! apps' commands); text with a leading space, or not shaped like a command, is sent as text.

use slack_core::dnd::{ScheduleDays, ScheduleEdit};

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
    /// Pause or resume notifications.
    Dnd(DndAction),
    /// Change the notification schedule (None: say what it is).
    DndSchedule(Option<Vec<ScheduleEdit>>),
    /// Rename the section holding the current conversation.
    SectionRename(String),
    /// Delete the section holding the current conversation.
    SectionDelete,
    /// Post the lines after `/snippet [lang] [title]` as a text snippet.
    Snippet { lang: String, title: String, body: String },
    /// A command ttyak does not know, run by Slack (`/remind`, `/giphy`, apps' commands).
    Slack { command: String, text: String },
}

#[derive(Debug, PartialEq, Eq)]
pub enum Recipient {
    /// An autocompleted mention.
    Id(String),
    /// A typed handle or display name without spaces.
    Name(String),
}

/// What `/dnd` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DndAction {
    /// Plain `/dnd`: resume when paused, else pause for 30 minutes.
    Toggle,
    /// Pause for this many minutes.
    Pause(u64),
    /// End whatever pauses them: a snooze and the current scheduled quiet period.
    Resume,
}

impl DndAction {
    /// What it does now: a toggle resumes when `paused`, else pauses for 30 minutes.
    pub fn decided(self, paused: bool) -> DndAction {
        match self {
            DndAction::Toggle if paused => DndAction::Resume,
            DndAction::Toggle => DndAction::Pause(30),
            other => other,
        }
    }
}

/// What plain `/dnd` does now (its suggestion says so).
pub fn dnd_doing(paused: bool) -> &'static str {
    if paused { "resume notifications" } else { "pause notifications for 30 minutes" }
}

/// `/dnd schedule`'s rest: `off`, `every day [9:00-17:00]` or `weekdays [9:00-17:00]` (the hours
/// notifications come through); nothing says what it is.
fn parse_schedule(rest: &str) -> Result<Command, String> {
    let usage = "usage: /dnd schedule off | every day 9:00-17:00 | weekdays 9:00-17:00 (the hours you receive notifications)";
    let rest = rest.trim().to_lowercase();
    let after = |word: &str| rest.strip_prefix(word).filter(|r| r.is_empty() || r.starts_with(char::is_whitespace)).map(str::trim);
    let (days, hours) = match (after("off"), after("every day"), after("weekdays")) {
        (Some(""), ..) => return Ok(Command::DndSchedule(Some(vec![ScheduleEdit::Days(ScheduleDays::Off)]))),
        (_, Some(hours), _) => (ScheduleDays::EveryDay, hours),
        (_, _, Some(hours)) => (ScheduleDays::Weekdays, hours),
        _ if rest.is_empty() => return Ok(Command::DndSchedule(None)),
        _ if after("custom").is_some() => return Err("per-day hours are set in F6 → Availability".into()),
        _ => return Err(usage.into()),
    };
    let mut edits = vec![ScheduleEdit::Days(days)];
    if !hours.is_empty() {
        let (from, to) = receive_hours(hours).map_err(|e| format!("{e} ({usage})"))?;
        edits.push(ScheduleEdit::Range(from, to));
    }
    Ok(Command::DndSchedule(Some(edits)))
}

/// `9:00-17:00` (spaces or an en dash allowed): the hours notifications come through, as Slack
/// writes them.
pub fn receive_hours(text: &str) -> Result<(String, String), String> {
    let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let (from, to) = text.split_once(['-', '–']).ok_or("the hours as from-to, e.g. 9:00-17:00")?;
    let time = |t: &str| slack_core::dnd::slack_time(t).ok_or_else(|| format!("{t:?} is not a time of day (H:MM)"));
    let (from, to) = (time(from)?, time(to)?);
    if from == to {
        return Err("from and to are the same time".into());
    }
    Ok((from, to))
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
/// The snippet languages Slack accepts as `snippet_type` (the web client's snippet editor list,
/// plus `text`); anything else is refused with `unknown_snippet_type`.
pub const SNIPPET_LANGS: &[&str] = &[
    "text", "apl", "applescript", "asciiarmor", "asn.1", "aspx", "asterisk", "brainfuck", "c", "cfm", "clojure",
    "cmake", "cobol", "coffeescript", "cpp", "crystal", "csharp", "css", "csv", "cypher", "d", "dart", "diff",
    "django", "dockerfile", "dtd", "dylan", "ebnf", "ecl", "eiffel", "ejs", "elixir", "elm", "erb", "erlang",
    "factor", "fcl", "forth", "fortran", "fsharp", "gas", "gherkin", "go", "graphql", "groovy", "gss", "haml",
    "handlebars", "haskell", "haskell_literate", "haxe", "html", "http", "idl", "java", "javascript", "jinja2",
    "json", "json_ld", "jsp", "jsx", "julia", "kotlin", "latex", "less", "lisp", "livescript", "lua", "markdown",
    "mathematica", "matlab", "mbox", "mirc", "modelica", "mscgen", "mumps", "mysql", "nginx", "nsis", "ntriples",
    "objc", "ocaml", "oz", "pascal", "pegjs", "perl", "php", "pig", "powershell", "properties", "protobuf", "pug",
    "puppet", "python", "q", "r", "rpm", "rst", "ruby", "rust", "sas", "sass", "scala", "scheme", "scss", "shell",
    "sieve", "slim", "smalltalk", "smarty", "solr", "soy", "sparql", "spreadsheet", "sql", "stylus", "swift", "tcl",
    "textile", "tiddlywiki", "tiki", "toml", "tornado", "troff", "tsv", "tsx", "ttcn", "ttcn_cfg", "turtle", "twig",
    "typescript", "vb", "vbscript", "velocity", "verilog", "vhdl", "vue", "webidl", "xml", "xquery", "yacas", "yaml",
    "z80",
];

/// Usual names and file extensions for Slack's snippet languages.
const SNIPPET_ALIASES: &[(&str, &str)] = &[
    ("rs", "rust"), ("py", "python"), ("js", "javascript"), ("mjs", "javascript"), ("cjs", "javascript"), ("ts", "typescript"),
    ("sh", "shell"), ("bash", "shell"), ("zsh", "shell"), ("md", "markdown"), ("yml", "yaml"), ("kt", "kotlin"), ("kts", "kotlin"),
    ("rb", "ruby"), ("cs", "csharp"), ("c#", "csharp"), ("c++", "cpp"), ("cc", "cpp"), ("cxx", "cpp"), ("hpp", "cpp"), ("h", "c"),
    ("hs", "haskell"), ("pl", "perl"), ("ex", "elixir"), ("exs", "elixir"), ("erl", "erlang"), ("txt", "text"), ("plain", "text"),
    ("ps1", "powershell"), ("proto", "protobuf"), ("clj", "clojure"), ("cljs", "clojure"), ("coffee", "coffeescript"), ("htm", "html"),
    ("jl", "julia"), ("tex", "latex"), ("ml", "ocaml"), ("fs", "fsharp"), ("f90", "fortran"), ("ini", "properties"), ("golang", "go"),
];

/// Slack's snippet language for a name or file extension (any case), if it has one.
pub fn snippet_lang(word: &str) -> Option<&'static str> {
    let w = word.trim().to_lowercase();
    SNIPPET_LANGS.iter().find(|l| **l == w).copied().or_else(|| SNIPPET_ALIASES.iter().find(|(a, _)| *a == w).map(|(_, l)| *l))
}

/// `/snippet`'s head: `[lang] [title]`, or just a file name whose extension gives the language.
fn snippet_head(head: &str) -> Result<(String, String), String> {
    let head = head.trim();
    let (first, rest) = head.split_once(char::is_whitespace).unwrap_or((head, ""));
    let from_name = |name: &str| name.rsplit_once('.').and_then(|(_, ext)| snippet_lang(ext)).unwrap_or("text");
    if first.is_empty() {
        Ok(("text".into(), String::new()))
    } else if let Some(lang) = snippet_lang(first) {
        Ok((lang.into(), rest.trim().to_string()))
    } else if first.contains('.') {
        Ok((from_name(head).into(), head.to_string()))
    } else {
        Err(format!("unknown snippet language {first:?} — e.g. rust, python, javascript, shell, json, text"))
    }
}

/// ttyak's own commands for suggestions: name, what it takes, what it does. They stand in for
/// Slack's commands of the same name.
pub const OWN: &[(&str, &str, &str)] = &[
    ("/upload", "<path> [text]", "upload a file"),
    ("/snippet", "[lang] [title]", "post the following lines (Alt+Enter) as a snippet"),
    ("/join", "[#channel]", "join a channel (default: the one you are previewing)"),
    ("/leave", "", "leave this channel"),
    ("/create", "<name>", "create a channel"),
    ("/create-private", "<name>", "create a private channel"),
    ("/dm", "@a [@b…]", "open a DM, or a group DM with several people"),
    ("/star", "", "star or unstar this conversation"),
    ("/section", "rename <name> | delete", "rename or delete this conversation's section"),
    ("/status", "[:emoji:] [text] [for 30m|today] | clear", "set or clear your status"),
    ("/away", "", "appear away"),
    ("/active", "", "appear active"),
    ("/dnd", "[30m|2h|1d | off | schedule …]", "pause notifications, or resume them"),
    ("/workspace", "[name]", "list this profile's workspaces, or switch"),
    ("/prefs", "", "preferences (also F6)"),
];

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
        return Some(if body.trim().is_empty() {
            Err("usage: /snippet [lang] [title], then the snippet on the next lines (Alt+Enter)".into())
        } else {
            snippet_head(head).map(|(lang, title)| Command::Snippet { lang, title, body: body.to_string() })
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
        "/dnd" if rest.is_empty() => Ok(Command::Dnd(DndAction::Toggle)),
        "/dnd" if rest.eq_ignore_ascii_case("off") => Ok(Command::Dnd(DndAction::Resume)),
        "/dnd" if rest.split_whitespace().next().is_some_and(|w| w.eq_ignore_ascii_case("schedule")) => parse_schedule(&rest["schedule".len()..]),
        "/dnd" => match minutes(rest) {
            Some(m) => Ok(Command::Dnd(DndAction::Pause(m))),
            None => Err("usage: /dnd (pause 30 min, or resume) · /dnd 30m|2h|1d · /dnd off".into()),
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
        // Slack's own and apps' commands: one word of letters, digits, `-` and `_` (so a pasted
        // `/usr/bin/x` stays text), typed without a leading space.
        _ if !text.starts_with(char::is_whitespace) && word.starts_with('/') && word.len() > 1 && word[1..].chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') => {
            let (_, rest) = mrkdwn.trim().split_once(char::is_whitespace).unwrap_or(("", ""));
            Ok(Command::Slack { command: word.to_lowercase(), text: rest.trim().to_string() })
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn other_commands_go_to_slack() {
        let slack = |command: &str, text: &str| Some(Ok(Command::Slack { command: command.into(), text: text.into() }));
        assert_eq!(super::parse("/remind me in 10 minutes to stretch", "/remind me in 10 minutes to stretch"), slack("/remind", "me in 10 minutes to stretch"));
        assert_eq!(super::parse("/giphy", "/giphy"), slack("/giphy", ""));
        assert_eq!(super::parse("/remind @bob tomorrow", "/remind <@U2> tomorrow"), slack("/remind", "<@U2> tomorrow"), "mentions as Slack writes them");
        assert_eq!(super::parse("/Deploy-App_2 now", "/Deploy-App_2 now"), slack("/deploy-app_2", "now"));
        assert_eq!(super::parse("/usr/bin/env is where", "/usr/bin/env is where"), None, "a path is text");
        assert_eq!(super::parse(" /remind me", " /remind me"), None, "a leading space sends it as text");
        assert_eq!(super::parse("/", "/"), None);
        assert_eq!(super::parse("/star", "/star"), Some(Ok(Command::Star)), "ttyak's own commands first");
    }

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
        assert_eq!(parse("/dmx"), Some(Ok(Command::Slack { command: "/dmx".into(), text: "".into() })), "not /dm: Slack's to judge");
    }

    #[test]
    fn snippets() {
        assert_eq!(
            parse("/snippet python my script.py\nprint(1)\n  x = 2"),
            Some(Ok(Command::Snippet { lang: "python".into(), title: "my script.py".into(), body: "print(1)\n  x = 2".into() }))
        );
        assert_eq!(parse("/snippet\nhello"), Some(Ok(Command::Snippet { lang: "text".into(), title: String::new(), body: "hello".into() })));
        assert!(matches!(parse("/snippet rust"), Some(Err(_))), "nothing to post");
        // Aliases and file names (languages and values from Slack's list).
        let lang_title = |head: &str| match parse(&format!("/snippet {head}\nx")) {
            Some(Ok(Command::Snippet { lang, title, .. })) => Ok((lang, title)),
            Some(Err(e)) => Err(e),
            other => panic!("{other:?}"),
        };
        assert_eq!(lang_title("rs main.rs"), Ok(("rust".into(), "main.rs".into())));
        assert_eq!(lang_title("TOML Cargo.toml"), Ok(("toml".into(), "Cargo.toml".into())));
        assert_eq!(lang_title("build.sh"), Ok(("shell".into(), "build.sh".into())), "the language from the file name");
        assert_eq!(lang_title("notes.unknownext"), Ok(("text".into(), "notes.unknownext".into())));
        assert!(lang_title("klingon hello").unwrap_err().contains("unknown snippet language"), "refused before Slack refuses it");
        assert_eq!(snippet_lang("c++"), Some("cpp"));
        assert!(SNIPPET_LANGS.contains(&"rust") && SNIPPET_LANGS.len() == 143);
        assert_eq!(parse("/snippets"), Some(Ok(Command::Slack { command: "/snippets".into(), text: "".into() })), "not /snippet");
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
        assert_eq!(parse("/dnd 1h"), Some(Ok(Command::Dnd(DndAction::Pause(60)))));
        assert_eq!(parse("/dnd 2h"), Some(Ok(Command::Dnd(DndAction::Pause(120)))));
        assert_eq!(parse("/dnd off"), Some(Ok(Command::Dnd(DndAction::Resume))));
        assert_eq!(parse("/dnd"), Some(Ok(Command::Dnd(DndAction::Toggle))), "plain /dnd toggles, as in Slack");
        assert!(matches!(parse("/dnd soon"), Some(Err(_))));
        assert_eq!((DndAction::Toggle.decided(false), DndAction::Toggle.decided(true)), (DndAction::Pause(30), DndAction::Resume));
        assert_eq!((DndAction::Pause(60).decided(true), DndAction::Resume.decided(false)), (DndAction::Pause(60), DndAction::Resume), "only a toggle decides");
        assert_eq!((dnd_doing(false), dnd_doing(true)), ("pause notifications for 30 minutes", "resume notifications"), "what plain /dnd does now");
    }

    #[test]
    fn dnd_schedule() {
        use slack_core::dnd::{ScheduleDays::*, ScheduleEdit::*};
        let edits = |text: &str| match parse(text) {
            Some(Ok(Command::DndSchedule(edits))) => Ok(edits),
            Some(Err(e)) => Err(e),
            other => panic!("{text}: {other:?}"),
        };
        assert_eq!(edits("/dnd schedule"), Ok(None), "says what it is");
        assert_eq!(edits("/dnd schedule off"), Ok(Some(vec![Days(Off)])));
        assert_eq!(edits("/dnd schedule every day 9:00-17:00"), Ok(Some(vec![Days(EveryDay), Range("9:00".into(), "17:00".into())])));
        assert_eq!(edits("/dnd schedule weekdays 22:00 - 08:30"), Ok(Some(vec![Days(Weekdays), Range("22:00".into(), "8:30".into())])), "as Slack writes them");
        assert_eq!(edits("/dnd schedule Weekdays 10:00–18:00"), Ok(Some(vec![Days(Weekdays), Range("10:00".into(), "18:00".into())])), "any case, an en dash");
        assert_eq!(edits("/dnd schedule weekdays"), Ok(Some(vec![Days(Weekdays)])), "the hours as they are");
        assert_eq!(edits("/dnd Schedule off"), Ok(Some(vec![Days(Off)])), "the word in any case too");
        assert_eq!(edits("/dnd SCHEDULE"), Ok(None));
        assert_eq!(parse("/dnd Off"), Some(Ok(Command::Dnd(DndAction::Resume))));
        assert!(edits("/dnd schedule weekdays 25:00-8:00").unwrap_err().contains("time of day"));
        assert!(edits("/dnd schedule every day 9:00").is_err());
        assert!(edits("/dnd schedule every day 9:00-9:00").is_err(), "no hours at all");
        assert!(edits("/dnd schedule weekdaysx").is_err());
        assert!(edits("/dnd schedule custom").unwrap_err().contains("F6"), "per-day hours are set in F6");
        assert!(edits("/dnd schedule sometimes").unwrap_err().starts_with("usage"));
        assert_eq!(receive_hours(" 7:05-23:15 "), Ok(("7:05".into(), "23:15".into())));
        assert_eq!(parse("/dnd 30m"), Some(Ok(Command::Dnd(DndAction::Pause(30)))), "durations as before");
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
        // Not ttyak's: Slack runs them (it knows /shrug; /joinx gets "not a command" back).
        assert_eq!(parse("/shrug"), Some(Ok(Command::Slack { command: "/shrug".into(), text: "".into() })));
        assert_eq!(parse("/joinx"), Some(Ok(Command::Slack { command: "/joinx".into(), text: "".into() })));
        assert_eq!(parse("hello /join"), None);
    }
}

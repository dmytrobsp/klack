//! Key bindings: every action the app reacts to, its default keys, and the user's overrides
//! from `keys.toml` (one table per context, `action = "key"` or a list; a sequence such as
//! `"<leader> s"` uses the leader key). The help overlay and the status bar are drawn from
//! the same table, so they always show the keys in effect.

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// One key press: a code and the Ctrl/Alt/Shift modifiers. Shift is folded into characters
/// ('G') and Shift+Tab is `BackTab`, as terminals report them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl Key {
    pub fn new(code: KeyCode, mods: KeyModifiers) -> Key {
        let mut mods = mods & (KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT);
        if matches!(code, KeyCode::Char(_) | KeyCode::BackTab) {
            mods.remove(KeyModifiers::SHIFT);
        }
        Key { code, mods }
    }

    pub fn from_event(k: &KeyEvent) -> Key {
        Key::new(k.code, k.modifiers)
    }

    /// A printable character without Ctrl/Alt: text in a typing context.
    pub fn is_text(&self) -> bool {
        matches!(self.code, KeyCode::Char(_)) && !self.mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    }

    /// `ctrl+k`, `alt+up`, `shift+tab`, `G`, `f5`, `space`, `pageup`, `?` …
    pub fn parse(s: &str) -> Result<Key, String> {
        let s = s.trim();
        if s.is_empty() {
            return Err("empty key".into());
        }
        let mut mods = KeyModifiers::NONE;
        let mut parts: Vec<&str> = s.split('+').collect();
        // A trailing "+" is the plus key itself ("ctrl++").
        if s.ends_with("++") || s == "+" {
            parts.retain(|p| !p.is_empty());
            parts.push("+");
        }
        let (name, modifiers) = parts.split_last().ok_or("empty key")?;
        for m in modifiers {
            match m.to_lowercase().as_str() {
                "ctrl" | "control" | "c" => mods |= KeyModifiers::CONTROL,
                "alt" | "meta" | "m" | "a" => mods |= KeyModifiers::ALT,
                "shift" | "s" => mods |= KeyModifiers::SHIFT,
                other => return Err(format!("unknown modifier '{other}' in '{s}'")),
            }
        }
        let lower = name.to_lowercase();
        let code = match lower.as_str() {
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "page_up" | "pgup" => KeyCode::PageUp,
            "pagedown" | "page_down" | "pgdn" => KeyCode::PageDown,
            "tab" if mods.contains(KeyModifiers::SHIFT) => KeyCode::BackTab,
            "tab" => KeyCode::Tab,
            "backtab" => KeyCode::BackTab,
            "enter" | "return" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "space" => KeyCode::Char(' '),
            "backspace" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "insert" | "ins" => KeyCode::Insert,
            "plus" => KeyCode::Char('+'),
            f if f.len() > 1 && f.starts_with('f') && f[1..].parse::<u8>().is_ok_and(|n| (1..=24).contains(&n)) => KeyCode::F(f[1..].parse().unwrap_or(1)),
            _ => {
                let mut chars = name.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) if mods.contains(KeyModifiers::SHIFT) && c.is_alphabetic() => KeyCode::Char(c.to_uppercase().next().unwrap_or(c)),
                    // Ctrl/Alt with a letter: terminals report the lowercase letter.
                    (Some(c), None) if mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) && c.is_alphabetic() && !mods.contains(KeyModifiers::SHIFT) => KeyCode::Char(c.to_lowercase().next().unwrap_or(c)),
                    (Some(c), None) => KeyCode::Char(c),
                    _ => return Err(format!("unknown key '{name}' in '{s}'")),
                }
            }
        };
        Ok(Key::new(code, mods))
    }

    /// The key as written in keys.toml (`ctrl+k`, `alt+up`, `shift+tab`, `G`, `space`); parses back to itself.
    pub fn spec(&self) -> String {
        let mut out = String::new();
        for (m, name) in [(KeyModifiers::CONTROL, "ctrl+"), (KeyModifiers::ALT, "alt+"), (KeyModifiers::SHIFT, "shift+")] {
            if self.mods.contains(m) {
                out += name;
            }
        }
        out += &match self.code {
            KeyCode::Up => "up".into(),
            KeyCode::Down => "down".into(),
            KeyCode::Left => "left".into(),
            KeyCode::Right => "right".into(),
            KeyCode::Home => "home".into(),
            KeyCode::End => "end".into(),
            KeyCode::PageUp => "pageup".into(),
            KeyCode::PageDown => "pagedown".into(),
            KeyCode::Tab => "tab".into(),
            KeyCode::BackTab => "shift+tab".into(),
            KeyCode::Enter => "enter".into(),
            KeyCode::Esc => "esc".into(),
            KeyCode::Backspace => "backspace".into(),
            KeyCode::Delete => "delete".into(),
            KeyCode::Insert => "insert".into(),
            KeyCode::Char(' ') => "space".into(),
            KeyCode::Char('+') => "plus".into(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::F(n) => format!("f{n}"),
            other => format!("{other:?}").to_lowercase(),
        };
        out
    }

    /// How the key is shown in help: `Ctrl+K`, `Alt+↑`, `Shift+Tab`, `G`, `F5`.
    pub fn label(&self) -> String {
        let mut out = String::new();
        if self.mods.contains(KeyModifiers::CONTROL) {
            out += "Ctrl+";
        }
        if self.mods.contains(KeyModifiers::ALT) {
            out += "Alt+";
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            out += "Shift+";
        }
        out += &match self.code {
            KeyCode::Up => "↑".into(),
            KeyCode::Down => "↓".into(),
            KeyCode::Left => "←".into(),
            KeyCode::Right => "→".into(),
            KeyCode::Home => "Home".into(),
            KeyCode::End => "End".into(),
            KeyCode::PageUp => "PgUp".into(),
            KeyCode::PageDown => "PgDn".into(),
            KeyCode::Tab => "Tab".into(),
            KeyCode::BackTab => "Shift+Tab".into(),
            KeyCode::Enter => "Enter".into(),
            KeyCode::Esc => "Esc".into(),
            KeyCode::Backspace => "Backspace".into(),
            KeyCode::Delete => "Delete".into(),
            KeyCode::Insert => "Insert".into(),
            KeyCode::Char(' ') => "Space".into(),
            KeyCode::Char(c) if self.mods.is_empty() => c.to_string(),
            KeyCode::Char(c) => c.to_uppercase().to_string(),
            KeyCode::F(n) => format!("F{n}"),
            other => format!("{other:?}"),
        };
        out
    }
}

/// Where a binding applies. Main-screen contexts (`sidebar`, `messages`, `composer`) are
/// looked up after `global`; overlays take keys first and alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Context {
    Global,
    Sidebar,
    Messages,
    Composer,
    Completion,
    Switcher,
    Search,
    Picker,
    Details,
    Profile,
    Activity,
    Mover,
    TextField,
    Snippet,
    Viewer,
    Player,
    Confirm,
    Help,
    Preferences,
}

impl Context {
    pub const ALL: [Context; 19] = [
        Context::Global,
        Context::Sidebar,
        Context::Messages,
        Context::Composer,
        Context::Completion,
        Context::Switcher,
        Context::Search,
        Context::Picker,
        Context::Details,
        Context::Profile,
        Context::Activity,
        Context::Mover,
        Context::TextField,
        Context::Snippet,
        Context::Viewer,
        Context::Player,
        Context::Confirm,
        Context::Help,
        Context::Preferences,
    ];

    /// The table name in `keys.toml`.
    pub fn name(self) -> &'static str {
        match self {
            Context::Global => "global",
            Context::Sidebar => "sidebar",
            Context::Messages => "messages",
            Context::Composer => "composer",
            Context::Completion => "completion",
            Context::Switcher => "switcher",
            Context::Search => "search",
            Context::Picker => "reaction_picker",
            Context::Details => "details",
            Context::Profile => "profile",
            Context::Activity => "activity",
            Context::Mover => "move_to_section",
            Context::TextField => "text_field",
            Context::Snippet => "snippet",
            Context::Viewer => "image_viewer",
            Context::Player => "video_player",
            Context::Confirm => "confirm",
            Context::Help => "help",
            Context::Preferences => "preferences",
        }
    }

    /// The heading in the help overlay.
    pub fn title(self) -> &'static str {
        match self {
            Context::Global => "Everywhere",
            Context::Sidebar => "Sidebar",
            Context::Messages => "Messages and thread",
            Context::Composer => "Composer",
            Context::Completion => "Suggestions (@ # :)",
            Context::Switcher => "Jump to (Ctrl+K)",
            Context::Search => "Search",
            Context::Picker => "Reaction picker",
            Context::Details => "Conversation details",
            Context::Profile => "Profile card",
            Context::Activity => "Activity",
            Context::Mover => "Move to section",
            Context::TextField => "Text fields (new section name, keywords)",
            Context::Snippet => "Snippet view",
            Context::Viewer => "Image viewer",
            Context::Player => "Video player",
            Context::Confirm => "Yes / no questions",
            Context::Help => "This help",
            Context::Preferences => "Preferences",
        }
    }

    /// Typing goes into a text field here, so plain characters cannot be bound (in the
    /// global context they would be eaten in the composer).
    pub fn types_text(self) -> bool {
        matches!(self, Context::Global | Context::Composer | Context::Switcher | Context::Search | Context::Picker | Context::TextField)
    }

    fn from_name(name: &str) -> Option<Context> {
        Context::ALL.into_iter().find(|c| c.name() == name)
    }
}

/// Everything a key can do. One action may appear in several contexts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Quit,
    Help,
    Switcher,
    Search,
    Details,
    Huddle,
    Mute,
    Activity,
    Unbind,
    Reset,
    Vip,
    Preferences,
    PrevConversation,
    NextConversation,
    ScrollUp,
    ScrollDown,
    FocusNext,
    FocusPrev,
    Workspace1,
    Workspace2,
    Workspace3,
    Workspace4,
    Workspace5,
    Workspace6,
    Workspace7,
    Workspace8,
    Workspace9,
    Up,
    Down,
    Top,
    Bottom,
    Open,
    Star,
    MoveToSection,
    OpenThread,
    Back,
    Type,
    React,
    Edit,
    Delete,
    Copy,
    CopyLink,
    Save,
    Pin,
    Profile,
    ThreadHuddle,
    Send,
    Newline,
    Leave,
    Bold,
    Italic,
    Strike,
    Code,
    DeleteWord,
    DeleteLine,
    LineStart,
    LineEnd,
    WordLeft,
    WordRight,
    Left,
    Right,
    Backspace,
    DeleteForward,
    AlsoToChannel,
    Accept,
    Dismiss,
    Close,
    Erase,
    PageUp,
    PageDown,
    PrevTab,
    NextTab,
    Refresh,
    Pause,
    Yes,
}

macro_rules! actions {
    ($( $ctx:ident { $( $variant:ident $name:literal $desc:literal [$($key:literal),*] )* } )*) => {
        /// Every (context, action) pair with its name in `keys.toml`, help text and defaults.
        pub const ACTIONS: &[ActionInfo] = &[ $($( ActionInfo { context: Context::$ctx, action: Action::$variant, name: $name, description: $desc, defaults: &[$($key),*] }, )*)* ];
    };
}

pub struct ActionInfo {
    pub context: Context,
    pub action: Action,
    pub name: &'static str,
    pub description: &'static str,
    pub defaults: &'static [&'static str],
}

actions! {
    Global {
        Quit "quit" "quit" ["ctrl+q"]
        Help "help" "this help" ["f1"]
        Switcher "jump_to" "jump to a conversation or person (# alone: every channel)" ["ctrl+k"]
        Search "search" "search messages" ["ctrl+f"]
        Details "details" "conversation details: about, members, pins" ["f2"]
        Huddle "huddle" "join or leave the huddle" ["f3"]
        Mute "mute" "mute or unmute in the huddle" ["f4"]
        Activity "activity" "mentions, reactions, threads" ["f5"]
        Preferences "preferences" "preferences" ["f6"]
        PrevConversation "previous_conversation" "previous conversation" ["alt+up"]
        NextConversation "next_conversation" "next conversation" ["alt+down"]
        ScrollUp "scroll_up" "scroll messages up" ["pageup"]
        ScrollDown "scroll_down" "scroll messages down" ["pagedown"]
        FocusNext "focus_next" "next pane" ["tab"]
        FocusPrev "focus_previous" "previous pane" ["shift+tab"]
        Workspace1 "workspace_1" "workspace 1 of the profile" ["alt+1"]
        Workspace2 "workspace_2" "workspace 2" ["alt+2"]
        Workspace3 "workspace_3" "workspace 3" ["alt+3"]
        Workspace4 "workspace_4" "workspace 4" ["alt+4"]
        Workspace5 "workspace_5" "workspace 5" ["alt+5"]
        Workspace6 "workspace_6" "workspace 6" ["alt+6"]
        Workspace7 "workspace_7" "workspace 7" ["alt+7"]
        Workspace8 "workspace_8" "workspace 8" ["alt+8"]
        Workspace9 "workspace_9" "workspace 9" ["alt+9"]
    }
    Sidebar {
        Up "up" "previous conversation" ["up", "k"]
        Down "down" "next conversation" ["down", "j"]
        Top "top" "first conversation" ["home", "g"]
        Bottom "bottom" "last conversation" ["end", "G"]
        Open "open" "type in the chosen conversation" ["enter", "i", "esc"]
        Star "star" "star / unstar" ["*"]
        MoveToSection "move_to_section" "move to a section (or a new one)" ["m"]
        Help "help" "this help" ["?"]
    }
    Messages {
        Up "up" "previous message" ["up", "k"]
        Down "down" "next message" ["down", "j"]
        Top "top" "first message" ["home", "g"]
        Bottom "bottom" "last message" ["end", "G"]
        OpenThread "open_thread" "open the message's thread" ["enter"]
        Back "back" "back to the composer (in a thread: close it)" ["esc"]
        Type "type" "type (in a thread: reply)" ["i"]
        React "react" "add a reaction" ["r"]
        Edit "edit" "edit your message" ["e"]
        Delete "delete" "delete your message" ["d"]
        Copy "copy" "copy the text" ["y"]
        CopyLink "copy_link" "copy a link to the message" ["Y"]
        Open "open" "open image, play video, or view a snippet" ["o"]
        Save "save" "save attachments" ["s"]
        Pin "pin" "pin / unpin" ["P"]
        Profile "profile" "profile card: author, ←/→ people mentioned" ["u"]
        ThreadHuddle "thread_huddle" "start or join a huddle in the message's thread" ["H"]
        Help "help" "this help" ["?"]
    }
    Composer {
        Send "send" "send" ["enter"]
        Newline "newline" "new line" ["alt+enter", "shift+enter", "ctrl+j"]
        Leave "leave" "cancel an edit, else leave the composer" ["esc"]
        Bold "bold" "bold" ["alt+b"]
        Italic "italic" "italic" ["alt+i"]
        Strike "strike" "strikethrough" ["alt+s"]
        Code "code" "code" ["alt+c"]
        DeleteWord "delete_word" "delete the word before the cursor" ["ctrl+w", "ctrl+backspace", "alt+backspace"]
        DeleteLine "delete_line" "delete to the line start" ["ctrl+u"]
        LineStart "line_start" "line start" ["ctrl+a", "home"]
        LineEnd "line_end" "line end" ["ctrl+e", "end"]
        WordLeft "word_left" "word left" ["ctrl+left"]
        WordRight "word_right" "word right" ["ctrl+right"]
        Left "left" "cursor left" ["left"]
        Right "right" "cursor right" ["right"]
        Up "up" "line up; in an empty composer: edit your last message" ["up"]
        Down "down" "line down / newer sent text" ["down"]
        Backspace "backspace" "delete before the cursor" ["backspace"]
        DeleteForward "delete_forward" "delete at the cursor" ["delete"]
        AlsoToChannel "also_to_channel" "reply box: also send to the channel" ["alt+a"]
    }
    Completion {
        Up "up" "previous suggestion" ["up", "shift+tab"]
        Down "down" "next suggestion" ["down"]
        Accept "accept" "insert the suggestion" ["tab", "enter"]
        Dismiss "dismiss" "close the suggestions" ["esc"]
    }
    Switcher {
        Up "up" "previous entry" ["up"]
        Down "down" "next entry" ["down", "tab"]
        Open "open" "go there" ["enter"]
        Close "close" "close" ["esc"]
        Erase "erase" "delete a character" ["backspace"]
    }
    Search {
        Up "up" "previous result" ["up"]
        Down "down" "next result" ["down", "tab"]
        Open "open" "search, or go to the result" ["enter"]
        Close "close" "close" ["esc"]
        Erase "erase" "delete a character" ["backspace"]
    }
    Picker {
        Up "up" "previous emoji" ["up"]
        Down "down" "next emoji" ["down"]
        PageUp "page_up" "ten up" ["pageup"]
        PageDown "page_down" "ten down" ["pagedown"]
        Open "choose" "react with it" ["enter"]
        Close "close" "close" ["esc"]
        Erase "erase" "delete a character" ["backspace"]
    }
    Details {
        Close "close" "close" ["esc", "f2", "q"]
        PrevTab "previous_tab" "previous tab" ["left", "h", "shift+tab"]
        NextTab "next_tab" "next tab" ["right", "l", "tab"]
        Up "up" "previous row" ["up", "k"]
        Down "down" "next row" ["down", "j"]
        Open "open" "member: profile · pin: go to the message" ["enter"]
    }
    Profile {
        Close "close" "close" ["esc", "q"]
        PrevTab "previous_person" "previous person in the message" ["left", "shift+tab"]
        NextTab "next_person" "next person in the message" ["right", "tab"]
        Open "message" "message them" ["enter", "m"]
        Huddle "huddle" "start a huddle with them" ["h"]
        Vip "vip" "add to / remove from your VIPs" ["v"]
    }
    Activity {
        Close "close" "close" ["esc", "f5", "q"]
        Up "up" "previous item" ["up", "k"]
        Down "down" "next item" ["down", "j"]
        PageUp "page_up" "page up" ["pageup"]
        PageDown "page_down" "page down" ["pagedown"]
        Refresh "refresh" "load again" ["r"]
        Open "open" "go to the message" ["enter"]
    }
    Mover {
        Close "close" "close" ["esc", "q"]
        Up "up" "previous choice" ["up", "k"]
        Down "down" "next choice" ["down", "j"]
        Top "top" "first choice" ["home", "g"]
        Bottom "bottom" "last choice" ["end", "G"]
        Open "choose" "move there (New section…: name it)" ["enter"]
    }
    TextField {
        Open "done" "done (create the section / save)" ["enter"]
        Close "cancel" "cancel" ["esc"]
        Erase "erase" "delete a character" ["backspace"]
    }
    Snippet {
        Close "close" "close" ["esc", "q"]
        Up "up" "line up" ["up", "k"]
        Down "down" "line down" ["down", "j"]
        PageUp "page_up" "page up" ["pageup"]
        PageDown "page_down" "page down" ["pagedown", "space"]
        Top "top" "start" ["home", "g"]
        Bottom "bottom" "end" ["end", "G"]
        Copy "copy" "copy the whole snippet" ["y"]
    }
    Viewer {
        Close "close" "close" ["esc", "q"]
        PrevTab "previous" "previous image" ["left", "h"]
        NextTab "next" "next image" ["right", "l"]
    }
    Player {
        Close "close" "stop" ["esc", "q"]
        Pause "pause" "pause / play" ["space", "p"]
    }
    Confirm {
        Yes "yes" "confirm (any other key: no)" ["y", "Y"]
    }
    Preferences {
        Up "up" "previous setting" ["up", "k"]
        Down "down" "next setting" ["down", "j"]
        PrevTab "previous_category" "previous category" ["left", "h", "shift+tab"]
        NextTab "next_category" "next category" ["right", "l", "tab"]
        Open "change" "change the setting (Keys: press the new key next)" ["enter", "space"]
        Unbind "unbind" "Keys: remove the action's keys" ["delete", "backspace"]
        Reset "reset" "Keys: back to the default keys" ["r"]
        Close "close" "close" ["esc", "q", "f6"]
    }
    Help {
        Up "up" "scroll up" ["up", "k"]
        Down "down" "scroll down" ["down", "j"]
        PageUp "page_up" "page up" ["pageup"]
        PageDown "page_down" "page down" ["pagedown", "space"]
        Close "close" "close" ["esc", "q", "f1", "?"]
    }
}

/// What a key press means in a context, given keys pressed before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lookup {
    Action(Action),
    /// The start of a longer sequence (e.g. the leader): wait for the next key.
    Pending,
    None,
}

#[derive(Debug, Clone, Default)]
pub struct Keymap {
    pub leader: Option<Key>,
    /// (context, key sequence) → action.
    bindings: HashMap<(Context, Vec<Key>), Action>,
    /// (context, action name) → its sequences, in the order given (for help).
    keys: HashMap<(Context, &'static str), Vec<Vec<Key>>>,
}

impl Keymap {
    /// Defaults overridden by `keys.toml` text; problems are reported, and the entries
    /// with problems keep their defaults.
    pub fn load(toml_text: &str) -> (Keymap, Vec<String>) {
        let mut warnings = Vec::new();
        let table: toml::Table = match toml::from_str(toml_text) {
            Ok(t) => t,
            Err(e) => {
                warnings.push(format!("keys.toml: {e}"));
                toml::Table::new()
            }
        };
        let mut map = Keymap::default();
        if let Some(leader) = table.get("leader") {
            match leader.as_str().map(Key::parse) {
                Some(Ok(k)) => map.leader = Some(k),
                Some(Err(e)) => warnings.push(format!("keys.toml: leader: {e}")),
                None => warnings.push("keys.toml: leader must be a string such as \"ctrl+b\"".into()),
            }
        }
        // Overrides: (context, action name) → key strings.
        let mut overrides: HashMap<(Context, String), Vec<String>> = HashMap::new();
        for (name, value) in &table {
            if name == "leader" {
                continue;
            }
            let Some(context) = Context::from_name(name) else {
                warnings.push(format!("keys.toml: unknown section [{name}]"));
                continue;
            };
            let Some(entries) = value.as_table() else {
                warnings.push(format!("keys.toml: [{name}] must be a table"));
                continue;
            };
            for (action, keys) in entries {
                if !ACTIONS.iter().any(|a| a.context == context && a.name == action) {
                    warnings.push(format!("keys.toml: [{name}] has no action '{action}'"));
                    continue;
                }
                let list: Option<Vec<String>> = match keys {
                    toml::Value::String(s) => Some(vec![s.clone()]),
                    toml::Value::Array(a) => a.iter().map(|v| v.as_str().map(String::from)).collect(),
                    _ => None,
                };
                match list {
                    Some(l) => {
                        overrides.insert((context, action.clone()), l);
                    }
                    None => warnings.push(format!("keys.toml: [{name}] {action} must be a key or a list of keys")),
                }
            }
        }
        for info in ACTIONS {
            let defaults = || info.defaults.iter().map(|s| s.to_string()).collect::<Vec<_>>();
            let wanted = overrides.get(&(info.context, info.name.to_string())).cloned();
            let parsed = match wanted.as_ref().map(|w| map.parse_sequences(info.context, w)) {
                Some(Ok(seqs)) => seqs,
                Some(Err(e)) => {
                    warnings.push(format!("keys.toml: [{}] {}: {e} (keeping the default)", info.context.name(), info.name));
                    map.parse_sequences(info.context, &defaults()).unwrap_or_default()
                }
                None => map.parse_sequences(info.context, &defaults()).unwrap_or_default(),
            };
            map.keys.insert((info.context, info.name), parsed);
        }
        // Bind, reporting keys claimed twice in one context; the first claim wins.
        for info in ACTIONS {
            for seq in map.keys[&(info.context, info.name)].clone() {
                match map.bindings.get(&(info.context, seq.clone())) {
                    Some(other) if *other != info.action => {
                        let owner = ACTIONS.iter().find(|a| a.context == info.context && a.action == *other).map_or("?", |a| a.name);
                        warnings.push(format!("keys.toml: [{}] {} is bound to both {owner} and {}; {owner} keeps it", info.context.name(), sequence_label(&seq), info.name));
                        map.keys.get_mut(&(info.context, info.name)).map(|v| v.retain(|s| *s != seq));
                    }
                    _ => {
                        map.bindings.insert((info.context, seq), info.action);
                    }
                }
            }
        }
        // Main-screen keys that a global binding would take first.
        for ((context, seq), action) in &map.bindings {
            if matches!(context, Context::Sidebar | Context::Messages | Context::Composer) {
                if let Some(global) = map.bindings.get(&(Context::Global, seq.clone())) {
                    if global != action {
                        warnings.push(format!("keys.toml: {} in [{}] is shadowed by a [global] binding", sequence_label(seq), context.name()));
                    }
                }
            }
        }
        warnings.sort();
        (map, warnings)
    }

    fn parse_sequences(&self, context: Context, keys: &[String]) -> Result<Vec<Vec<Key>>, String> {
        keys.iter()
            .map(|k| {
                let seq = k
                    .split_whitespace()
                    .map(|part| match part {
                        "<leader>" => self.leader.ok_or_else(|| "uses <leader> but no leader key is set".to_string()),
                        p => Key::parse(p),
                    })
                    .collect::<Result<Vec<Key>, String>>()?;
                if seq.is_empty() {
                    return Err("empty key".into());
                }
                if seq.len() == 1 && seq[0].is_text() && context.types_text() {
                    return Err(format!("'{}' would be typed as text here; use a key with Ctrl/Alt or a <leader> sequence", seq[0].label()));
                }
                Ok(seq)
            })
            .collect()
    }

    /// What `key` does in `context` after the keys in `pending`.
    pub fn lookup(&self, context: Context, pending: &[Key], key: Key) -> Lookup {
        let mut seq = pending.to_vec();
        seq.push(key);
        if let Some(a) = self.bindings.get(&(context, seq.clone())) {
            return Lookup::Action(*a);
        }
        if self.bindings.keys().any(|(c, s)| *c == context && s.len() > seq.len() && s.starts_with(&seq)) {
            return Lookup::Pending;
        }
        Lookup::None
    }

    /// The keys of an action in a context, as shown in help ("↑ / k").
    pub fn keys_label(&self, context: Context, name: &str) -> String {
        self.keys.iter().find(|((c, n), _)| *c == context && *n == name).map(|(_, seqs)| seqs.iter().map(|s| sequence_label(s)).collect::<Vec<_>>().join(" / ")).unwrap_or_default()
    }

    /// The first key of an action, for short hints ("F1 help").
    pub fn first_key(&self, context: Context, action: Action) -> Option<String> {
        let info = ACTIONS.iter().find(|a| a.context == context && a.action == action)?;
        self.keys.get(&(context, info.name))?.first().map(|s| sequence_label(s))
    }
}

/// The few actions worth a hint in the status bar for each context, with short words.
pub fn hints_for(context: Context) -> &'static [(Action, &'static str)] {
    use Action::*;
    match context {
        Context::Global => &[(Switcher, "jump"), (Search, "search"), (FocusNext, "focus")],
        Context::Sidebar => &[(Open, "open"), (Star, "star"), (MoveToSection, "section")],
        Context::Messages => &[(OpenThread, "thread"), (React, "react"), (Edit, "edit"), (Profile, "profile"), (Open, "open"), (Back, "back")],
        Context::Composer => &[(Send, "send"), (Newline, "new line"), (Leave, "leave")],
        Context::Completion => &[(Accept, "insert"), (Dismiss, "close")],
        Context::Switcher | Context::Search => &[(Open, "go"), (Down, "next"), (Close, "close")],
        Context::Picker => &[(Open, "react"), (Close, "close")],
        Context::Details => &[(NextTab, "next tab"), (Open, "open"), (Close, "close")],
        Context::Profile => &[(Open, "message"), (Huddle, "huddle"), (Vip, "VIP"), (NextTab, "next person"), (Close, "close")],
        Context::Activity => &[(Open, "open"), (Refresh, "refresh"), (Close, "close")],
        Context::Mover => &[(Open, "move"), (Close, "close")],
        Context::TextField => &[(Open, "done"), (Close, "cancel")],
        Context::Snippet => &[(PageDown, "page"), (Copy, "copy"), (Close, "close")],
        Context::Viewer => &[(NextTab, "next"), (Close, "close")],
        Context::Player => &[(Pause, "pause"), (Close, "stop")],
        Context::Confirm => &[(Yes, "yes")],
        Context::Help => &[(PageDown, "page"), (Close, "close")],
        Context::Preferences => &[(Open, "change"), (NextTab, "category"), (Close, "close")],
    }
}

/// Every word used in status-bar hints (for translation templates).
pub fn hint_words() -> Vec<&'static str> {
    let mut out: Vec<&str> = Context::ALL.iter().flat_map(|c| hints_for(*c).iter().map(|(_, w)| *w)).collect();
    out.push("help");
    out
}

impl Keymap {
    /// Status-bar hints for the active contexts ("r react · e edit · F1 help · Ctrl+K jump"):
    /// the most specific context first, then help, then the global ones (which a narrow
    /// window cuts off first); actions without a key are left out.
    pub fn hints(&self, contexts: &[Context]) -> String {
        let hint = |c: Context, (action, word): &(Action, &str)| self.first_key(c, *action).map(|k| format!("{k} {}", super::i18n::tr(word)));
        let mut parts: Vec<String> = contexts.iter().rev().filter(|c| **c != Context::Global).flat_map(|&c| hints_for(c).iter().filter_map(move |h| hint(c, h))).collect();
        parts.extend(hint(Context::Global, &(Action::Help, "help")));
        parts.extend(hints_for(Context::Global).iter().filter_map(|h| hint(Context::Global, h)));
        parts.join(" · ")
    }
}

impl Keymap {
    /// The sequences bound to an action now.
    pub fn sequences(&self, context: Context, name: &str) -> Vec<Vec<Key>> {
        self.keys.iter().find(|((c, n), _)| *c == context && *n == name).map(|(_, s)| s.clone()).unwrap_or_default()
    }

    /// keys.toml text for these bindings, with `change` applied (an action's new sequences,
    /// or a new leader); only entries that differ from the defaults are written.
    pub fn to_toml(&self, change: Option<(Context, &str, Vec<Vec<Key>>)>, leader: Option<Option<Key>>) -> String {
        let leader = leader.unwrap_or(self.leader);
        let defaults = Keymap::load("").0;
        let spec = |seq: &[Key]| seq.iter().map(|k| if Some(*k) == leader && seq.len() > 1 { "<leader>".to_string() } else { k.spec() }).collect::<Vec<_>>().join(" ");
        let mut out = String::new();
        if let Some(l) = leader {
            out += &format!("leader = \"{}\"\n", l.spec());
        }
        for context in Context::ALL {
            let mut entries = Vec::new();
            for info in ACTIONS.iter().filter(|a| a.context == context) {
                let seqs = match &change {
                    Some((c, n, s)) if *c == context && *n == info.name => s.clone(),
                    _ => self.sequences(context, info.name),
                };
                if seqs != defaults.sequences(context, info.name) {
                    let list: Vec<String> = seqs.iter().map(|s| format!("\"{}\"", spec(s).replace('\\', "\\\\").replace('"', "\\\""))).collect();
                    entries.push(format!("{} = [{}]\n", info.name, list.join(", ")));
                }
            }
            if !entries.is_empty() {
                out += &format!("\n[{}]\n{}", context.name(), entries.concat());
            }
        }
        out
    }
}

pub fn sequence_label(seq: &[Key]) -> String {
    seq.iter().map(Key::label).collect::<Vec<_>>().join(" ")
}

/// Where the user's bindings live: `keys.toml` next to the profiles.
pub fn config_path() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("io.github", "rslack", "slacktui").map(|d| d.config_dir().join("keys.toml"))
}

/// The user's keymap: defaults plus `keys.toml` if there is one.
pub fn load_user() -> (Keymap, Vec<String>) {
    match config_path().map(std::fs::read_to_string) {
        Some(Ok(text)) => Keymap::load(&text),
        Some(Err(e)) if e.kind() != std::io::ErrorKind::NotFound => {
            let (map, _) = Keymap::load("");
            (map, vec![format!("cannot read keys.toml: {e}")])
        }
        _ => Keymap::load(""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> Key {
        Key::new(code, mods)
    }

    #[test]
    fn parsing_and_labels() {
        assert_eq!(Key::parse("ctrl+k"), Ok(key(KeyCode::Char('k'), KeyModifiers::CONTROL)));
        assert_eq!(Key::parse("Ctrl+K"), Ok(key(KeyCode::Char('k'), KeyModifiers::CONTROL)));
        assert_eq!(Key::parse("G"), Ok(key(KeyCode::Char('G'), KeyModifiers::NONE)));
        assert_eq!(Key::parse("shift+g"), Ok(key(KeyCode::Char('G'), KeyModifiers::NONE)));
        assert_eq!(Key::parse("shift+tab"), Ok(key(KeyCode::BackTab, KeyModifiers::NONE)));
        assert_eq!(Key::parse("alt+up"), Ok(key(KeyCode::Up, KeyModifiers::ALT)));
        assert_eq!(Key::parse("f12"), Ok(key(KeyCode::F(12), KeyModifiers::NONE)));
        assert_eq!(Key::parse("space"), Ok(key(KeyCode::Char(' '), KeyModifiers::NONE)));
        assert_eq!(Key::parse("ctrl++"), Ok(key(KeyCode::Char('+'), KeyModifiers::CONTROL)));
        assert!(Key::parse("hyper+x").is_err() && Key::parse("foo").is_err() && Key::parse("").is_err());
        assert_eq!(Key::parse("alt+up").unwrap().label(), "Alt+↑");
        assert_eq!(Key::parse("ctrl+k").unwrap().label(), "Ctrl+K");
        assert_eq!(Key::parse("shift+tab").unwrap().label(), "Shift+Tab");
        // Terminals report Shift with uppercase letters and with BackTab.
        assert_eq!(Key::new(KeyCode::Char('G'), KeyModifiers::SHIFT), Key::parse("G").unwrap());
        assert_eq!(Key::new(KeyCode::BackTab, KeyModifiers::SHIFT), Key::parse("shift+tab").unwrap());
    }

    #[test]
    fn defaults_are_today_s_keys_and_conflict_free() {
        let (map, warnings) = Keymap::load("");
        assert!(warnings.is_empty(), "{warnings:?}");
        let k = |s: &str| Key::parse(s).unwrap();
        assert_eq!(map.lookup(Context::Global, &[], k("ctrl+k")), Lookup::Action(Action::Switcher));
        assert_eq!(map.lookup(Context::Messages, &[], k("Y")), Lookup::Action(Action::CopyLink));
        assert_eq!(map.lookup(Context::Messages, &[], k("y")), Lookup::Action(Action::Copy));
        assert_eq!(map.lookup(Context::Composer, &[], k("ctrl+j")), Lookup::Action(Action::Newline));
        assert_eq!(map.lookup(Context::Details, &[], k("tab")), Lookup::Action(Action::NextTab));
        assert_eq!(map.lookup(Context::Sidebar, &[], k("x")), Lookup::None);
        assert_eq!(map.keys_label(Context::Sidebar, "up"), "↑ / k");
        assert_eq!(map.first_key(Context::Global, Action::Help).as_deref(), Some("F1"));
        // Every action is reachable by default.
        for a in ACTIONS {
            assert!(!map.keys_label(a.context, a.name).is_empty(), "{}.{} has no key", a.context.name(), a.name);
        }
    }

    #[test]
    fn user_overrides_and_leader() {
        let (map, warnings) = Keymap::load(
            r#"
            leader = "ctrl+b"
            [global]
            jump_to = ["ctrl+p", "<leader> k"]
            [messages]
            react = "R"
            pin = []
            "#,
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        let k = |s: &str| Key::parse(s).unwrap();
        assert_eq!(map.lookup(Context::Global, &[], k("ctrl+k")), Lookup::None, "replaced, not added");
        assert_eq!(map.lookup(Context::Global, &[], k("ctrl+p")), Lookup::Action(Action::Switcher));
        assert_eq!(map.lookup(Context::Global, &[], k("ctrl+b")), Lookup::Pending);
        assert_eq!(map.lookup(Context::Global, &[k("ctrl+b")], k("k")), Lookup::Action(Action::Switcher));
        assert_eq!(map.lookup(Context::Messages, &[], k("R")), Lookup::Action(Action::React));
        assert_eq!(map.lookup(Context::Messages, &[], k("r")), Lookup::None);
        assert_eq!(map.lookup(Context::Messages, &[], k("P")), Lookup::None, "unbound with []");
        assert_eq!(map.keys_label(Context::Global, "jump_to"), "Ctrl+P / Ctrl+B k");
    }

    #[test]
    fn bad_entries_keep_their_defaults() {
        let (map, warnings) = Keymap::load(
            r#"
            [global]
            search = "q"
            nonsense = "x"
            jump_to = "<leader> k"
            [messages]
            copy = "r"
            [nowhere]
            "#,
        );
        let k = |s: &str| Key::parse(s).unwrap();
        assert_eq!(map.lookup(Context::Global, &[], k("ctrl+f")), Lookup::Action(Action::Search), "plain q would be typed in the composer");
        assert_eq!(map.lookup(Context::Global, &[], k("ctrl+k")), Lookup::Action(Action::Switcher), "no leader set");
        assert_eq!(map.lookup(Context::Messages, &[], k("r")), Lookup::Action(Action::React), "react was there first");
        assert_eq!(warnings.len(), 5, "{warnings:#?}");
        assert!(warnings.iter().any(|w| w.contains("nowhere")) && warnings.iter().any(|w| w.contains("bound to both react and copy")));
    }

    #[test]
    fn hints_follow_context_and_bindings() {
        let (map, _) = Keymap::load("[messages]\nreact = \"R\"\nedit = []\n");
        let h = map.hints(&[Context::Global, Context::Messages]);
        assert!(h.starts_with("Enter thread · R react · u profile"), "{h}");
        assert!(!h.contains("edit"), "unbound actions get no hint: {h}");
        assert!(h.ends_with("Esc back · F1 help · Ctrl+K jump · Ctrl+F search · Tab focus"), "{h}");
        assert_eq!(map.hints(&[Context::Global, Context::Confirm]), "y yes · F1 help · Ctrl+K jump · Ctrl+F search · Tab focus");
    }

    #[test]
    fn specs_round_trip_and_overrides_are_written() {
        for s in ["ctrl+k", "alt+up", "shift+tab", "G", "f5", "space", "pageup", "?", "*", "plus", "ctrl+alt+x", "shift+enter"] {
            let k = Key::parse(s).unwrap();
            assert_eq!(Key::parse(&k.spec()), Ok(k), "{s} → {}", k.spec());
        }
        let (map, _) = Keymap::load("");
        assert_eq!(map.to_toml(None, None), "", "defaults write nothing");
        let k = |s: &str| Key::parse(s).unwrap();
        let text = map.to_toml(Some((Context::Messages, "react", vec![vec![k("R")]])), Some(Some(k("ctrl+b"))));
        assert!(text.starts_with("leader = \"ctrl+b\""), "{text}");
        let (again, warnings) = Keymap::load(&text);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(again.lookup(Context::Messages, &[], k("R")), Lookup::Action(Action::React));
        let chord = again.to_toml(Some((Context::Global, "jump_to", vec![vec![k("ctrl+b"), k("k")]])), None);
        assert!(chord.contains("jump_to = [\"<leader> k\"]"), "{chord}");
        assert!(chord.contains("react = [\"R\"]"), "keeps earlier changes: {chord}");
    }

    #[test]
    fn shadowing_by_global_is_reported() {
        let (_, warnings) = Keymap::load("[global]\nhelp = \"ctrl+e\"\n");
        assert!(warnings.iter().any(|w| w.contains("Ctrl+E in [composer] is shadowed")), "{warnings:?}");
    }
}

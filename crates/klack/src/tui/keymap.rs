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

/// `<leader>` in a sequence: the leader or the insert leader, whichever was pressed (shown
/// as the leader's key, see [`Keymap::show`]).
pub const LEADER: Key = Key { code: KeyCode::Null, mods: KeyModifiers::NONE };

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
            "menu" => KeyCode::Menu,
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
            KeyCode::Null => "<leader>".into(),
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
            KeyCode::Null => "<leader>".into(),
            other => format!("{other:?}"),
        };
        out
    }
}

/// Where a binding applies. The focused pane (`sidebar`, `messages`, `composer`, …) or the
/// open dialog is looked up before `global`.
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
    Threads,
    Mover,
    TextField,
    Snippet,
    Viewer,
    Player,
    Confirm,
    Help,
    Preferences,
    Menu,
    Save,
    Huddle,
}

impl Context {
    pub const ALL: [Context; 23] = [
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
        Context::Threads,
        Context::Mover,
        Context::TextField,
        Context::Snippet,
        Context::Viewer,
        Context::Player,
        Context::Confirm,
        Context::Help,
        Context::Preferences,
        Context::Menu,
        Context::Save,
        Context::Huddle,
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
            Context::Threads => "threads",
            Context::Mover => "move_to_section",
            Context::TextField => "text_field",
            Context::Snippet => "snippet",
            Context::Viewer => "image_viewer",
            Context::Player => "video_player",
            Context::Confirm => "confirm",
            Context::Help => "help",
            Context::Preferences => "preferences",
            Context::Menu => "menu",
            Context::Save => "save",
            Context::Huddle => "huddle_view",
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
            Context::Threads => "Threads (Ctrl+T)",
            Context::Mover => "Move to section",
            Context::TextField => "Text fields (new section name, keywords)",
            Context::Snippet => "Snippet view",
            Context::Viewer => "Image viewer",
            Context::Player => "Video player",
            Context::Confirm => "Yes / no questions",
            Context::Help => "This help",
            Context::Preferences => "Preferences",
            Context::Menu => "Context menu (right-click)",
            Context::Save => "Save files (s)",
            Context::Huddle => "Huddle view (F8)",
        }
    }

    /// Typing goes into a text field here, so plain characters cannot be bound (a plain
    /// character in `global` acts only in NORMAL panes).
    pub fn types_text(self) -> bool {
        matches!(self, Context::Composer | Context::Switcher | Context::Search | Context::Picker | Context::TextField)
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
    Threads,
    HuddleThread,
    HuddleView,
    Camera,
    ShareScreen,
    DeclineHuddle,
    Unbind,
    Reset,
    Increase,
    Decrease,
    Vip,
    Preferences,
    PrevConversation,
    NextConversation,
    ScrollUp,
    ScrollDown,
    HalfPageUp,
    HalfPageDown,
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
    ContextMenu,
    Paste,
    Tick,
    TickAll,
    Folder,
    MoveToSection,
    FoldSection,
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
    SeekBack,
    SeekForward,
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
        HuddleThread "huddle_thread" "open the huddle's thread (yours, or this conversation's)" ["f7"]
        HuddleView "huddle_view" "show or hide the huddle view (in place of the messages)" ["f8"]
        Camera "camera" "switch your camera on or off in the huddle (chosen in F6)" ["f9"]
        ShareScreen "share_screen" "share your screen in the huddle, or stop (the desktop asks what to share)" ["f10"]
        DeclineHuddle "decline_huddle" "decline the huddle invitation that is ringing (the huddle key joins it)" ["shift+f3"]
        Activity "activity" "mentions, reactions, threads" ["f5"]
        Threads "threads" "every thread you are in, unread first" ["ctrl+t"]
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
        HalfPageUp "half_page_up" "half a page up" ["ctrl+u"]
        HalfPageDown "half_page_down" "half a page down" ["ctrl+d"]
        Open "open" "type in the chosen conversation" ["enter", "i"]
        Star "star" "star / unstar" ["*"]
        MoveToSection "move_to_section" "move to a section (or a new one)" ["m"]
        FoldSection "fold_section" "fold / unfold the section (folded: what is unread in it)" ["space", "z"]
        Help "help" "this help" ["?"]
    }
    Messages {
        Up "up" "previous message" ["up", "k"]
        Down "down" "next message" ["down", "j"]
        Top "top" "first message" ["home", "g"]
        Bottom "bottom" "last message" ["end", "G"]
        HalfPageUp "half_page_up" "half a page up" ["ctrl+u"]
        HalfPageDown "half_page_down" "half a page down" ["ctrl+d"]
        OpenThread "open_thread" "open the message's thread" ["enter"]
        Back "back" "close the thread (in the messages pane: nothing)" ["esc"]
        Type "type" "type (in a thread: reply)" ["i"]
        React "react" "add a reaction" ["r"]
        Edit "edit" "edit your message" ["e"]
        Delete "delete" "delete your message" ["d"]
        Copy "copy" "copy the text" ["y"]
        CopyLink "copy_link" "copy a link to the message" ["Y"]
        Open "open" "open a link, picture, video or snippet (a list when there are several)" ["o"]
        Save "save" "save attachments" ["s"]
        Pin "pin" "pin / unpin" ["P"]
        Profile "profile" "profile card: author, ←/→ people mentioned" ["u"]
        ThreadHuddle "thread_huddle" "start or join a huddle in the message's thread" ["H"]
        ContextMenu "menu" "the message's menu (as a right-click does)" ["m", "menu", "shift+f10"]
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
        Paste "paste" "paste from the clipboard: text, a picture, copied files" ["ctrl+v"]
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
        PageUp "page_up" "a page up" ["pageup"]
        PageDown "page_down" "a page down" ["pagedown"]
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
    Threads {
        Close "close" "close" ["esc", "ctrl+t", "q"]
        Up "up" "previous thread" ["up", "k"]
        Down "down" "next thread" ["down", "j"]
        PageUp "page_up" "page up" ["pageup"]
        PageDown "page_down" "page down" ["pagedown"]
        Refresh "refresh" "load again" ["r"]
        Open "open" "open the thread at the reply shown" ["enter"]
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
        SeekBack "seek_back" "10 seconds back" ["left", "h"]
        SeekForward "seek_forward" "10 seconds forward" ["right", "l"]
    }
    Confirm {
        Yes "yes" "confirm (any other key: no)" ["y", "Y"]
    }
    Huddle {
        Up "up" "the tile above" ["up", "k"]
        Down "down" "the tile below" ["down", "j"]
        PrevTab "left" "the tile to the left" ["left", "h"]
        NextTab "right" "the tile to the right" ["right", "l"]
        Open "maximize" "maximize the tile, or all tiles again" ["enter", "space", "f"]
        Close "back" "all tiles again, or leave the tiles" ["esc"]
    }
    Preferences {
        Up "up" "previous setting" ["up", "k"]
        Down "down" "next setting" ["down", "j"]
        PrevTab "previous_category" "previous category" ["left", "h", "shift+tab"]
        NextTab "next_category" "next category" ["right", "l", "tab"]
        Open "change" "change the setting (Keys: press the new key next)" ["enter", "space"]
        Unbind "unbind" "Keys: remove the action's keys" ["delete", "backspace"]
        Reset "reset" "Keys: back to the default keys" ["r"]
        Increase "increase" "raise a level (volumes, gain, threshold)" ["+", "="]
        Decrease "decrease" "lower a level" ["-"]
        Close "close" "close" ["esc", "q", "f6"]
    }
    Menu {
        Up "up" "previous item" ["up", "k"]
        Down "down" "next item" ["down", "j"]
        Open "choose" "do it" ["enter", "space"]
        Close "close" "close" ["esc", "q"]
    }
    Save {
        Up "up" "previous file" ["up", "k"]
        Down "down" "next file" ["down", "j"]
        Tick "tick" "tick / untick the file" ["space"]
        TickAll "tick_all" "tick all / none" ["a"]
        Folder "folder" "edit the folder (Tab there completes it)" ["tab"]
        Open "save" "save the ticked files" ["enter"]
        Close "close" "close" ["esc", "q"]
    }
    Help {
        Up "up" "scroll up" ["up", "k"]
        Down "down" "scroll down" ["down", "j"]
        PageUp "page_up" "page up" ["pageup"]
        PageDown "page_down" "page down" ["pagedown", "space"]
        Close "close" "close" ["esc", "q", "f1", "?"]
    }
}

/// Where input goes: a dialog takes it all; otherwise the composer (typing) or the
/// main-screen panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Dialog,
}

/// What a key press means in the active contexts, given keys pressed before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolved {
    /// The action and the context it was found in.
    Action(Context, Action),
    /// The start of a longer sequence (e.g. the leader): wait for the next key.
    Pending,
    None,
}

/// The keys of a sequence typed so far, and the mode it was started in.
#[derive(Debug, Clone, Default)]
pub struct Pending {
    /// As looked up ([`LEADER`] for a leader).
    keys: Vec<Key>,
    /// As pressed (Space or Ctrl+Space), for the status line.
    typed: Vec<Key>,
    mode: Option<Mode>,
}

impl Pending {
    /// `key`, pressed as itself.
    #[allow(dead_code)]
    pub fn push(&mut self, key: Key, mode: Mode) {
        self.push_typed(key, key, mode);
    }

    /// `key` as looked up, pressed as `typed`.
    pub fn push_typed(&mut self, key: Key, typed: Key, mode: Mode) {
        if self.keys.is_empty() {
            self.mode = Some(mode);
        }
        self.keys.push(key);
        self.typed.push(typed);
    }

    /// One key back (which-key's Backspace).
    #[allow(dead_code)]
    pub fn pop(&mut self) {
        self.keys.pop();
        self.typed.pop();
        if self.keys.is_empty() {
            self.mode = None;
        }
    }

    pub fn clear(&mut self) {
        self.keys.clear();
        self.typed.clear();
        self.mode = None;
    }

    pub fn keys(&self) -> &[Key] {
        &self.keys
    }

    pub fn typed(&self) -> &[Key] {
        &self.typed
    }

    /// Drop the sequence once the mode is not the one it was started in (a dialog opened).
    pub fn sync(&mut self, mode: Mode) {
        if self.mode.is_some_and(|m| m != mode) {
            self.clear();
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Keymap {
    /// The leader of NORMAL panes (Space unless keys.toml says otherwise).
    pub leader: Option<Key>,
    /// The leader where the leader would type or belongs to a dialog (Ctrl+Space; none if empty).
    pub insert_leader: Option<Key>,
    /// (context, key sequence) → action.
    bindings: HashMap<(Context, Vec<Key>), Action>,
    /// (context, action name) → its sequences, in the order given (for help).
    keys: HashMap<(Context, &'static str), Vec<Vec<Key>>>,
}

/// The leaders when keys.toml does not set them: Space and Ctrl+Space.
fn default_leaders() -> (Key, Key) {
    (Key::new(KeyCode::Char(' '), KeyModifiers::NONE), Key::new(KeyCode::Char(' '), KeyModifiers::CONTROL))
}

impl Keymap {
    /// Defaults overridden by `keys.toml` text; problems are reported, and the entries
    /// with problems keep their defaults. The user's bindings win over the defaults.
    pub fn load(toml_text: &str) -> (Keymap, Vec<String>) {
        let mut warnings = Vec::new();
        let table: toml::Table = match toml::from_str(toml_text) {
            Ok(t) => t,
            Err(e) => {
                warnings.push(format!("keys.toml: {e}"));
                toml::Table::new()
            }
        };
        let (leader, insert_leader) = default_leaders();
        let mut map = Keymap { leader: Some(leader), insert_leader: Some(insert_leader), ..Default::default() };
        for (name, slot) in [("leader", &mut map.leader), ("insert_leader", &mut map.insert_leader)] {
            match table.get(name).map(|v| v.as_str()) {
                Some(Some(s)) if s.trim().is_empty() => *slot = None,
                Some(Some(s)) => match Key::parse(s) {
                    Ok(k) if name == "insert_leader" && k.is_text() => warnings.push(format!("keys.toml: insert_leader '{}' would be typed as text; use a key with Ctrl/Alt", k.label())),
                    Ok(k) => *slot = Some(k),
                    Err(e) => warnings.push(format!("keys.toml: {name}: {e}")),
                },
                Some(None) => warnings.push(format!("keys.toml: {name} must be a string such as \"ctrl+b\"")),
                None => {}
            }
        }
        // Overrides: (context, action name) → key strings.
        let mut overrides: HashMap<(Context, String), Vec<String>> = HashMap::new();
        for (name, value) in &table {
            if name == "leader" || name == "insert_leader" {
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
        // The actions whose keys the user chose (a bad entry keeps the default ones).
        let mut by_user = std::collections::HashSet::new();
        for info in ACTIONS {
            let defaults = || info.defaults.iter().map(|s| s.to_string()).collect::<Vec<_>>();
            let wanted = overrides.get(&(info.context, info.name.to_string())).cloned();
            let parsed = match wanted.as_ref().map(|w| map.parse_sequences(info.context, w)) {
                Some(Ok(seqs)) => {
                    by_user.insert((info.context, info.name));
                    seqs
                }
                Some(Err(e)) => {
                    warnings.push(format!("keys.toml: [{}] {}: {e} (keeping the default)", info.context.name(), info.name));
                    map.parse_sequences(info.context, &defaults()).unwrap_or_default()
                }
                None => map.parse_sequences(info.context, &defaults()).unwrap_or_default(),
            };
            map.keys.insert((info.context, info.name), parsed);
        }
        // Bind the user's sequences first, then the defaults. A sequence that equals, starts or
        // extends one bound before it in its context could never be told apart, so it is dropped:
        // a default gives way to the user's; otherwise the first in the table keeps it.
        let mut bound: Vec<(Context, Vec<Key>, &'static str, bool)> = Vec::new();
        for user in [true, false] {
            for info in ACTIONS.iter().filter(|a| by_user.contains(&(a.context, a.name)) == user) {
                for seq in map.keys[&(info.context, info.name)].clone() {
                    let clash = bound.iter().find(|(c, s, ..)| *c == info.context && (s.starts_with(&seq) || seq.starts_with(s)));
                    let Some((_, other, owner, owner_by_user)) = clash else {
                        bound.push((info.context, seq.clone(), info.name, user));
                        map.bindings.insert((info.context, seq), info.action);
                        continue;
                    };
                    if *owner == info.name && *other == seq {
                        continue;
                    }
                    let context = info.context.name();
                    warnings.push(if !user && *owner_by_user {
                        format!("keys.toml: [{context}] default {} of {} gives way to {owner}", map.label(&seq), info.name)
                    } else if *other == seq {
                        format!("keys.toml: [{context}] {} is bound to both {owner} and {}; {owner} keeps it", map.label(&seq), info.name)
                    } else {
                        format!("keys.toml: [{context}] {} of {} collides with {} of {owner}; {owner} keeps it", map.label(&seq), info.name, map.label(other))
                    });
                    map.keys.get_mut(&(info.context, info.name)).map(|v| v.retain(|s| *s != seq));
                }
            }
        }
        // The user's [global] sequences that a pane or dialog binding hides where it applies (a
        // default written out again is still a default).
        let default_of = |name: &str| ACTIONS.iter().find(|a| a.context == Context::Global && a.name == name).map(|a| map.parse_sequences(Context::Global, &a.defaults.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap_or_default()).unwrap_or_default();
        for (_, seq, name, _) in bound.iter().filter(|(c, s, name, user)| *c == Context::Global && *user && !default_of(name).contains(s)) {
            for (context, other, owner, _) in bound.iter().filter(|(c, ..)| *c != Context::Global) {
                let applies = match context {
                    Context::Sidebar | Context::Messages | Context::Huddle => true,
                    Context::Composer | Context::Completion => !seq[0].is_text(),
                    _ => !seq[0].is_text() && (seq[0] == LEADER || *name == "quit"),
                };
                if applies && (other.starts_with(seq) || seq.starts_with(other)) {
                    warnings.push(format!("keys.toml: [global] {} is hidden by [{}] {owner}", map.label(seq), context.name()));
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
                        "<leader>" if self.leader.is_none() && self.insert_leader.is_none() => Err("uses <leader> but no leader key is set".to_string()),
                        "<leader>" => Ok(LEADER),
                        p => Key::parse(p),
                    })
                    .collect::<Result<Vec<Key>, String>>()?;
                if seq.is_empty() {
                    return Err("empty key".into());
                }
                // A leader spelled out at the start of a sequence (v0.5.0 files) is the leader.
                let mut seq = seq;
                if seq.len() > 1 && self.leader == Some(seq[0]) {
                    seq[0] = LEADER;
                }
                if seq.len() == 1 && seq[0].is_text() && context.types_text() {
                    return Err(format!("'{}' would be typed as text here; use a key with Ctrl/Alt or a <leader> sequence", seq[0].label()));
                }
                Ok(seq)
            })
            .collect()
    }

    /// What `key` does after the keys in `pending`, in `contexts` (most specific first: the
    /// dialog, or the pane, then `Global`).
    pub fn resolve(&self, contexts: &[Context], mode: Mode, pending: &[Key], key: Key) -> Resolved {
        self.press(contexts, mode, pending, key).0
    }

    /// [`Keymap::resolve`], with what the key stands for in the sequence: [`LEADER`] when it is
    /// a leader that acts here and starts something, else the key itself.
    pub fn press(&self, contexts: &[Context], mode: Mode, pending: &[Key], key: Key) -> (Resolved, Key) {
        let leader = Some(key) == self.insert_leader || Some(key) == self.leader && (mode == Mode::Normal || !key.is_text());
        let mut seq = pending.to_vec();
        if leader {
            seq.push(LEADER);
            let found = self.lookup(contexts, mode, &seq);
            if found != Resolved::None {
                return (found, LEADER);
            }
            seq.pop();
        }
        seq.push(key);
        (self.lookup(contexts, mode, &seq), key)
    }

    /// A whole sequence: the first context that knows it decides (its action, or Pending for a
    /// longer one). In INSERT and dialogs a `[global]` sequence that starts with text is left
    /// out; in a dialog only `[global]` leader sequences and quit count.
    fn lookup(&self, contexts: &[Context], mode: Mode, seq: &[Key]) -> Resolved {
        for &context in contexts {
            let counts = |s: &[Key], action: Action| context != Context::Global || mode == Mode::Normal || !s[0].is_text() && (mode == Mode::Insert || s[0] == LEADER || action == Action::Quit);
            if let Some(a) = self.bindings.get(&(context, seq.to_vec())).filter(|a| counts(seq, **a)) {
                return Resolved::Action(context, *a);
            }
            if self.bindings.iter().any(|((c, s), a)| *c == context && s.len() > seq.len() && s.starts_with(seq) && counts(s, *a)) {
                return Resolved::Pending;
            }
        }
        Resolved::None
    }

    /// The key `<leader>` is shown as: the leader, else the insert leader.
    fn leader_key(&self) -> Option<Key> {
        self.leader.or(self.insert_leader)
    }

    /// A sequence as help shows it, the leader as its key ("Space h m", "Ctrl+B k").
    pub fn label(&self, seq: &[Key]) -> String {
        seq.iter().map(|k| if *k == LEADER { self.leader_key().unwrap_or(LEADER) } else { *k }.label()).collect::<Vec<_>>().join(" ")
    }

    /// A sequence in short form, for the status bar, which-key and menus: Space `␣`, Ctrl+X
    /// `^X`, Ctrl+Space `^␣`, Alt+x `M-x`, others as in help; no separators (`␣h`, `^␣hm`).
    pub fn show(&self, seq: &[Key]) -> String {
        let short = |k: Key| {
            let KeyCode::Char(c) = k.code else { return k.label() };
            let c = if c == ' ' { '␣' } else { c };
            let ctrl = k.mods.contains(KeyModifiers::CONTROL);
            let alt = if k.mods.contains(KeyModifiers::ALT) { "M-" } else { "" };
            if ctrl { format!("^{alt}{}", c.to_uppercase()) } else { format!("{alt}{c}") }
        };
        seq.iter().map(|k| if *k == LEADER { self.leader_key().map_or_else(|| LEADER.label(), short) } else { short(*k) }).collect()
    }

    /// The keys of an action in a context, as shown in help ("↑ / k").
    pub fn keys_label(&self, context: Context, name: &str) -> String {
        self.keys.iter().find(|((c, n), _)| *c == context && *n == name).map(|(_, seqs)| seqs.iter().map(|s| self.label(s)).collect::<Vec<_>>().join(" / ")).unwrap_or_default()
    }

    /// The first key of an action, for short hints ("F1 help").
    pub fn first_key(&self, context: Context, action: Action) -> Option<String> {
        let info = ACTIONS.iter().find(|a| a.context == context && a.action == action)?;
        self.keys.get(&(context, info.name))?.first().map(|s| self.label(s))
    }
}

/// The few actions worth a hint in the status bar for each context, with short words.
pub fn hints_for(context: Context) -> &'static [(Action, &'static str)] {
    use Action::*;
    match context {
        Context::Global => &[(Switcher, "jump"), (Search, "search"), (FocusNext, "focus")],
        Context::Sidebar => &[(Up, "move"), (Open, "open"), (Help, "help")],
        Context::Messages => &[(Up, "move"), (OpenThread, "thread"), (Type, "type"), (Help, "help")],
        Context::Composer => &[(Newline, "new line"), (Send, "send"), (Leave, "normal")],
        Context::Completion => &[(Accept, "insert"), (Dismiss, "close")],
        Context::Switcher | Context::Search => &[(Open, "go"), (Down, "next"), (Close, "close")],
        Context::Picker => &[(Open, "react"), (Close, "close")],
        Context::Details => &[(NextTab, "next tab"), (Open, "open"), (Close, "close")],
        Context::Profile => &[(Open, "message"), (Huddle, "huddle"), (Vip, "VIP"), (NextTab, "next person"), (Close, "close")],
        Context::Activity | Context::Threads => &[(Open, "open"), (Refresh, "refresh"), (Close, "close")],
        Context::Mover => &[(Open, "move"), (Close, "close")],
        Context::TextField => &[(Open, "done"), (Close, "cancel")],
        Context::Snippet => &[(PageDown, "page"), (Copy, "copy"), (Close, "close")],
        Context::Viewer => &[(NextTab, "next"), (Close, "close")],
        Context::Player => &[(Pause, "pause"), (SeekBack, "−10 s"), (SeekForward, "+10 s"), (Close, "stop")],
        Context::Confirm => &[(Yes, "yes")],
        Context::Help => &[(PageDown, "page"), (Close, "close")],
        Context::Preferences => &[(Open, "change"), (NextTab, "category"), (Close, "close")],
        Context::Menu => &[(Open, "choose"), (Close, "close")],
        Context::Save => &[(Open, "save"), (Tick, "tick"), (TickAll, "all"), (Folder, "folder"), (Close, "close")],
        Context::Huddle => &[(Open, "maximize"), (NextTab, "next tile"), (Close, "back")],
    }
}

/// Every word used in status-bar hints (for translation templates).
pub fn hint_words() -> Vec<&'static str> {
    let mut out: Vec<&str> = Context::ALL.iter().flat_map(|c| hints_for(*c).iter().map(|(_, w)| *w)).collect();
    out.extend(["help", "actions"]);
    out
}

impl Keymap {
    /// One context's own key hints as (key, word), least useful first, for the status line;
    /// actions without a key are left out. "move" shows the Up and Down keys ("↑/↓"); in the
    /// panes the leader's "actions" comes before "help".
    pub fn context_hints(&self, context: Context) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for (action, word) in hints_for(context) {
            let key = if *action == Action::Up && *word == "move" {
                match (self.first_key(context, Action::Up), self.first_key(context, Action::Down)) {
                    (Some(up), Some(down)) => Some(format!("{up}/{down}")),
                    (up, down) => up.or(down),
                }
            } else {
                self.first_key(context, *action)
            };
            if *action == Action::Help && matches!(context, Context::Sidebar | Context::Messages) && self.leader_key().is_some() {
                out.push((self.show(&[LEADER]), super::i18n::tr("actions").into_owned()));
            }
            if let Some(key) = key {
                out.push((key, super::i18n::tr(word).into_owned()));
            }
        }
        out
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
        let spec = |seq: &[Key]| seq.iter().map(Key::spec).collect::<Vec<_>>().join(" ");
        let mut out = String::new();
        // A leader set to none is written empty (left out, it would be the default again).
        for (name, key, default) in [("leader", leader, defaults.leader), ("insert_leader", self.insert_leader, defaults.insert_leader)] {
            if key != default {
                out += &format!("{name} = \"{}\"\n", key.map(|k| k.spec()).unwrap_or_default());
            }
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

/// Where the user's bindings live: `keys.toml` next to the profiles.
pub fn config_path() -> Option<std::path::PathBuf> {
    slack_core::session::project_dirs().map(|d| d.config_dir().join("keys.toml"))
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

    /// `key` after `pending` in one context alone, in the mode that context is used in.
    fn one(map: &Keymap, context: Context, pending: &[Key], key: Key) -> Resolved {
        let mode = match context {
            Context::Global | Context::Sidebar | Context::Messages | Context::Huddle => Mode::Normal,
            Context::Composer | Context::Completion => Mode::Insert,
            _ => Mode::Dialog,
        };
        map.resolve(&[context], mode, pending, key)
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
        assert_eq!(one(&map, Context::Global, &[], k("ctrl+k")), Resolved::Action(Context::Global, Action::Switcher));
        assert_eq!(one(&map, Context::Messages, &[], k("Y")), Resolved::Action(Context::Messages, Action::CopyLink));
        assert_eq!(one(&map, Context::Messages, &[], k("y")), Resolved::Action(Context::Messages, Action::Copy));
        assert_eq!(one(&map, Context::Composer, &[], k("ctrl+j")), Resolved::Action(Context::Composer, Action::Newline));
        assert_eq!(one(&map, Context::Details, &[], k("tab")), Resolved::Action(Context::Details, Action::NextTab));
        assert_eq!(one(&map, Context::Sidebar, &[], k("x")), Resolved::None);
        assert_eq!(map.keys_label(Context::Sidebar, "up"), "↑ / k");
        assert_eq!(map.first_key(Context::Global, Action::Help).as_deref(), Some("F1"));
        // The message menu from the keyboard: m, the Menu key (as kitty's protocol reports it)
        // and Shift+F10 (F10 alone shares the screen).
        for key in [k("m"), Key::new(KeyCode::Menu, KeyModifiers::NONE), Key::new(KeyCode::F(10), KeyModifiers::SHIFT)] {
            assert_eq!(one(&map, Context::Messages, &[], key), Resolved::Action(Context::Messages, Action::ContextMenu), "{key:?}");
        }
        assert_eq!(one(&map, Context::Global, &[], k("f10")), Resolved::Action(Context::Global, Action::ShareScreen));
        assert_eq!(map.keys_label(Context::Messages, "menu"), "m / Menu / Shift+F10");
        assert_eq!(k("menu").spec(), "menu", "written back as read");
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
        assert_eq!(one(&map, Context::Global, &[], k("ctrl+k")), Resolved::None, "replaced, not added");
        assert_eq!(one(&map, Context::Global, &[], k("ctrl+p")), Resolved::Action(Context::Global, Action::Switcher));
        assert_eq!(map.press(&[Context::Global], Mode::Normal, &[], k("ctrl+b")), (Resolved::Pending, LEADER));
        assert_eq!(one(&map, Context::Global, &[LEADER], k("k")), Resolved::Action(Context::Global, Action::Switcher));
        assert_eq!(one(&map, Context::Messages, &[], k("R")), Resolved::Action(Context::Messages, Action::React));
        assert_eq!(one(&map, Context::Messages, &[], k("r")), Resolved::None);
        assert_eq!(one(&map, Context::Messages, &[], k("P")), Resolved::None, "unbound with []");
        assert_eq!(map.keys_label(Context::Global, "jump_to"), "Ctrl+P / Ctrl+B k");
    }

    #[test]
    fn bad_entries_keep_their_defaults() {
        let (map, warnings) = Keymap::load(
            r#"
            leader = ""
            insert_leader = ""
            [global]
            nonsense = "x"
            jump_to = "<leader> k"
            [composer]
            send = "q"
            [messages]
            copy = "x"
            react = "x"
            [nowhere]
            "#,
        );
        assert_eq!(one(&map, Context::Composer, &[], k("enter")), Resolved::Action(Context::Composer, Action::Send), "plain q would be typed in the composer");
        assert_eq!(one(&map, Context::Global, &[], k("ctrl+k")), Resolved::Action(Context::Global, Action::Switcher), "no leader set");
        assert_eq!(one(&map, Context::Messages, &[], k("x")), Resolved::Action(Context::Messages, Action::React), "react is first in the table");
        assert_eq!(warnings.len(), 5, "{warnings:#?}");
        assert!(warnings.iter().any(|w| w.contains("nowhere")) && warnings.iter().any(|w| w.contains("bound to both react and copy")));
    }

    #[test]
    fn hints_follow_context_and_bindings() {
        let (map, _) = Keymap::load("[messages]\nreact = \"R\"\nedit = []\n");
        let words = |c| map.context_hints(c).into_iter().map(|(k, w)| format!("{k} {w}")).collect::<Vec<_>>().join(" · ");
        assert_eq!(words(Context::Messages), "↑/↓ move · Enter thread · i type · ␣ actions · ? help");
        assert_eq!(words(Context::Sidebar), "↑/↓ move · Enter open · ␣ actions · ? help");
        assert_eq!(words(Context::Composer), "Alt+Enter new line · Enter send · Esc normal");
        assert_eq!(words(Context::Confirm), "y yes");
        let (map, _) = Keymap::load("[messages]\ntype = []\nopen_thread = \"R\"\n");
        let h = map.context_hints(Context::Messages);
        assert_eq!(h[1], ("R".to_string(), "thread".to_string()));
        assert!(!h.iter().any(|(_, w)| w == "type"), "unbound actions get no hint: {h:?}");
    }

    #[test]
    fn specs_round_trip_and_overrides_are_written() {
        for s in ["ctrl+k", "alt+up", "shift+tab", "G", "f5", "space", "pageup", "?", "*", "plus", "ctrl+alt+x", "shift+enter"] {
            let k = Key::parse(s).unwrap();
            assert_eq!(Key::parse(&k.spec()), Ok(k), "{s} → {}", k.spec());
        }
        let (map, _) = Keymap::load("");
        assert_eq!(map.to_toml(None, None), "", "defaults write nothing");
        let none = map.to_toml(None, Some(None));
        assert!(none.starts_with("leader = \"\"\n"), "no leader is written empty: {none}");
        assert_eq!(Keymap::load(&none).0.leader, None);
        let text = map.to_toml(Some((Context::Messages, "react", vec![vec![k("R")]])), Some(Some(k("ctrl+b"))));
        assert!(text.starts_with("leader = \"ctrl+b\""), "{text}");
        let (again, warnings) = Keymap::load(&text);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(one(&again, Context::Messages, &[], k("R")), Resolved::Action(Context::Messages, Action::React));
        let chord = again.to_toml(Some((Context::Global, "jump_to", vec![vec![LEADER, k("k")]])), None);
        assert!(chord.contains("jump_to = [\"<leader> k\"]"), "{chord}");
        assert!(chord.contains("react = [\"R\"]"), "keeps earlier changes: {chord}");
    }

    #[test]
    fn global_keys_hidden_by_a_pane_are_reported() {
        let (_, warnings) = Keymap::load("[global]\nhelp = \"ctrl+e\"\n");
        assert_eq!(warnings, vec!["keys.toml: [global] Ctrl+E is hidden by [composer] line_end".to_string()]);
        assert!(Keymap::load("[global]\nfocus_next = \"tab\"\n").1.is_empty(), "the default written out: Tab in the suggestions is on purpose");
        // Defaults may do it on purpose; a printable global key does not act where it types.
        assert!(Keymap::load("").1.is_empty());
        let (_, warnings) = Keymap::load("[global]\nmute = \"j\"\n");
        assert_eq!(warnings.len(), 3, "sidebar, messages, huddle view: {warnings:?}");
        assert!(warnings.iter().all(|w| w.starts_with("keys.toml: [global] j is hidden by")));
    }

    fn k(s: &str) -> Key {
        Key::parse(s).unwrap()
    }

    #[test]
    fn leaders_default_and_stand_for_leader() {
        use Context::*;
        let (map, warnings) = Keymap::load("[global]\nmute = \"<leader> h m\"\n");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!((map.leader, map.insert_leader), (Some(k("space")), Some(k("ctrl+space"))));
        assert_eq!(map.resolve(&[Messages, Global], Mode::Normal, &[], k("space")), Resolved::Pending);
        assert_eq!(map.resolve(&[Composer, Global], Mode::Insert, &[], k("space")), Resolved::None, "types");
        assert_eq!(map.resolve(&[Completion, Composer, Global], Mode::Insert, &[], k("space")), Resolved::None);
        assert_eq!(map.resolve(&[Composer, Global], Mode::Insert, &[], k("ctrl+space")), Resolved::Pending);
        assert_eq!(map.resolve(&[Search, Global], Mode::Dialog, &[], k("space")), Resolved::None, "Space belongs to the dialog");
        assert_eq!(map.resolve(&[Search, Global], Mode::Dialog, &[LEADER, k("h")], k("m")), Resolved::Action(Global, Action::Mute));
        // The leader that starts no sequence here is its plain key (Space folds in the sidebar).
        assert_eq!(map.resolve(&[Sidebar, Global], Mode::Normal, &[], k("space")), Resolved::Pending);
        let (plain, _) = Keymap::load("");
        assert_eq!(plain.resolve(&[Sidebar, Global], Mode::Normal, &[], k("space")), Resolved::Action(Sidebar, Action::FoldSection));
        // insert_leader = "" is none; a leader that never types works everywhere.
        let (none, warnings) = Keymap::load("leader = \"ctrl+b\"\ninsert_leader = \"\"\n[global]\nmute = \"<leader> h m\"\n");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(none.insert_leader, None);
        assert_eq!(none.resolve(&[Composer, Global], Mode::Insert, &[], k("ctrl+space")), Resolved::None);
        assert_eq!(none.resolve(&[Composer, Global], Mode::Insert, &[], k("ctrl+b")), Resolved::Pending);
        assert_eq!(none.resolve(&[Search, Global], Mode::Dialog, &[], k("ctrl+b")), Resolved::Pending);
    }

    #[test]
    fn pending_drops_on_mode_change() {
        let mut p = Pending::default();
        p.push(LEADER, Mode::Normal);
        p.sync(Mode::Normal);
        assert_eq!(p.keys(), &[LEADER], "same mode keeps the keys");
        p.push_typed(LEADER, k("ctrl+space"), Mode::Normal);
        assert_eq!((p.keys(), p.typed()), (&[LEADER, LEADER][..], &[LEADER, k("ctrl+space")][..]));
        p.pop();
        p.push(k("h"), Mode::Normal);
        p.pop();
        assert_eq!(p.keys(), &[LEADER]);
        p.sync(Mode::Dialog);
        assert!(p.keys().is_empty());
        p.push(k("x"), Mode::Insert);
        p.clear();
        assert!(p.keys().is_empty());
    }

    #[test]
    fn pane_beats_global() {
        let (map, _) = Keymap::load("[global]\nactivity = \"ctrl+a\"\n");
        assert_eq!(map.resolve(&[Context::Composer, Context::Global], Mode::Insert, &[], k("ctrl+a")), Resolved::Action(Context::Composer, Action::LineStart));
        assert_eq!(map.resolve(&[Context::Messages, Context::Global], Mode::Normal, &[], k("ctrl+a")), Resolved::Action(Context::Global, Action::Activity));
    }

    #[test]
    fn printable_global_keys_act_only_in_normal() {
        let (map, warnings) = Keymap::load("[global]\nmute = \"x\"\n");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(map.resolve(&[Context::Messages, Context::Global], Mode::Normal, &[], k("x")), Resolved::Action(Context::Global, Action::Mute));
        assert_eq!(map.resolve(&[Context::Composer, Context::Global], Mode::Insert, &[], k("x")), Resolved::None);
        assert_eq!(map.resolve(&[Context::Search, Context::Global], Mode::Dialog, &[], k("x")), Resolved::None);
        let (_, warnings) = Keymap::load("[composer]\nsend = \"x\"\n");
        assert!(warnings.iter().any(|w| w.contains("would be typed")), "{warnings:?}");
    }

    #[test]
    fn user_binding_beats_default_prefix() {
        let (map, warnings) = Keymap::load("[global]\nmute = \"<leader> h m\"\nhuddle = \"<leader> h\"\n");
        assert_eq!(map.resolve(&[Context::Messages, Context::Global], Mode::Normal, &[LEADER], k("h")), Resolved::Action(Context::Global, Action::Huddle));
        assert!(map.sequences(Context::Global, "mute").is_empty(), "mute is dropped");
        assert!(warnings.iter().any(|w| w.contains("mute")), "{warnings:?}");
        // A v0.5.0 file still means what it meant.
        let (old, warnings) = Keymap::load("leader = \"ctrl+b\"\n[messages]\nreact = \"r\"\n[global]\nhuddle = \"f3\"\n");
        assert!(!warnings.iter().any(|w| w.contains("keeping the default")), "{warnings:?}");
        assert_eq!(old.resolve(&[Context::Messages, Context::Global], Mode::Normal, &[], k("r")), Resolved::Action(Context::Messages, Action::React));
        assert_eq!(old.resolve(&[Context::Messages, Context::Global], Mode::Normal, &[], k("f3")), Resolved::Action(Context::Global, Action::Huddle));
    }

    #[test]
    fn user_binding_beats_a_default_of_another_action() {
        let (map, warnings) = Keymap::load("[messages]\nreact = \"e\"\n");
        assert_eq!(map.resolve(&[Context::Messages, Context::Global], Mode::Normal, &[], k("e")), Resolved::Action(Context::Messages, Action::React));
        assert!(map.sequences(Context::Messages, "edit").is_empty());
        assert_eq!(warnings, vec!["keys.toml: [messages] default e of edit gives way to react".to_string()]);
        // A user sequence that a default starts.
        let (map, warnings) = Keymap::load("[messages]\nreact = \"e x\"\n");
        assert_eq!(map.resolve(&[Context::Messages, Context::Global], Mode::Normal, &[], k("e")), Resolved::Pending);
        assert_eq!(map.resolve(&[Context::Messages, Context::Global], Mode::Normal, &[k("e")], k("x")), Resolved::Action(Context::Messages, Action::React));
        assert!(warnings.iter().any(|w| w.contains("default e of edit gives way to react")), "{warnings:?}");
    }

    #[test]
    fn normal_insert_defaults() {
        use Context::*;
        let (map, warnings) = Keymap::load("");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(map.resolve(&[Sidebar, Global], Mode::Normal, &[], k("esc")), Resolved::None);
        assert_eq!(map.resolve(&[Messages, Global], Mode::Normal, &[], k("ctrl+d")), Resolved::Action(Messages, Action::HalfPageDown));
        assert_eq!(map.resolve(&[Messages, Global], Mode::Normal, &[], k("ctrl+u")), Resolved::Action(Messages, Action::HalfPageUp));
        assert_eq!(map.resolve(&[Sidebar, Global], Mode::Normal, &[], k("ctrl+d")), Resolved::Action(Sidebar, Action::HalfPageDown));
        assert_eq!(map.resolve(&[Sidebar, Global], Mode::Normal, &[], k("i")), Resolved::Action(Sidebar, Action::Open));
        assert_eq!(map.resolve(&[Messages, Global], Mode::Normal, &[], k("esc")), Resolved::Action(Messages, Action::Back));
    }

    #[test]
    fn composer_sequence_and_spelled_out_leader() {
        use Context::*;
        // A v0.5.0 file spells the leader out in a sequence: it stands for the leader.
        let (map, warnings) = Keymap::load("leader = \"ctrl+b\"\n[global]\njump_to = \"ctrl+b k\"\n");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(map.resolve(&[Messages, Global], Mode::Normal, &[], k("ctrl+b")), Resolved::Pending);
        assert_eq!(map.resolve(&[Messages, Global], Mode::Normal, &[LEADER], k("k")), Resolved::Action(Global, Action::Switcher));
        // A leader alone stays the key itself.
        let (map, _) = Keymap::load("leader = \"ctrl+b\"\n[global]\njump_to = \"ctrl+b\"\n");
        assert_eq!(map.resolve(&[Messages, Global], Mode::Normal, &[], k("ctrl+b")), Resolved::Action(Global, Action::Switcher));
    }

    #[test]
    fn insert_leader_must_not_type() {
        let (map, warnings) = Keymap::load("insert_leader = \"x\"\n");
        assert_eq!(warnings, vec!["keys.toml: insert_leader 'x' would be typed as text; use a key with Ctrl/Alt".to_string()]);
        assert_eq!(map.insert_leader, Some(k("ctrl+space")));
    }

    #[test]
    fn show_is_short() {
        let (map, _) = Keymap::load("");
        assert_eq!(map.show(&[LEADER, k("h")]), "␣h");
        assert_eq!(map.show(&[k("ctrl+space"), k("h"), k("m")]), "^␣hm");
        assert_eq!(map.show(&[k("ctrl+x"), k("alt+x"), k("f5")]), "^XM-xF5");
        let (map, _) = Keymap::load("leader = \"ctrl+b\"\n");
        assert_eq!(map.show(&[LEADER, k("h")]), "^Bh");
    }
}

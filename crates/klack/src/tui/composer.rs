//! Multi-line message composer with mrkdwn formatting helpers and mention tracking.

use crossterm::event::{KeyCode, KeyEvent};

use super::keymap::{Action, Context, Key, Keymap, Lookup};
use unicode_width::UnicodeWidthChar;

/// What a key press in the composer asks the app to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOutcome {
    /// Handled inside the composer (or ignored).
    Done,
    /// A character was typed (send a typing notification).
    Typed,
    /// Enter: send, or save the edit.
    Submit,
    /// Esc with nothing to cancel: leave the composer.
    Leave,
    /// ↑ in an empty composer: edit our last message.
    UpWhenEmpty,
}

/// What an autocomplete token looks like at the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trigger {
    User(String),
    Channel(String),
    Emoji(String),
}

/// What is typed in a composer — text, cursor, mentions and the message being edited — kept
/// while you are in another conversation or thread (the sent-message history stays put), and
/// across runs (see `drafts.rs`).
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Draft {
    text: String,
    /// In characters.
    cursor: usize,
    mentions: Vec<(String, String)>,
    editing: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct Composer {
    chars: Vec<char>,
    cursor: usize,
    /// `@display` / `#name` inserted by autocomplete → Slack id, used on send.
    mentions: Vec<(String, String)>,
    /// When editing an existing message: its ts.
    pub editing: Option<String>,
    history: Vec<String>,
    history_pos: Option<usize>,
}

impl Composer {
    /// Apply an editing key (bindings from the `composer` context of `keys`); plain
    /// characters are typed.
    pub fn handle_key(&mut self, k: KeyEvent, keys: &Keymap) -> KeyOutcome {
        let key = Key::from_event(&k);
        let action = match keys.lookup(Context::Composer, &[], key) {
            Lookup::Action(a) => Some(a),
            Lookup::Pending | Lookup::None => None,
        };
        match action {
            Some(Action::Leave) if self.editing.is_some() => self.clear(),
            Some(Action::Leave) => return KeyOutcome::Leave,
            Some(Action::Newline) => self.insert('\n'),
            Some(Action::Send) => return KeyOutcome::Submit,
            Some(Action::Bold) => self.toggle_marker('*'),
            Some(Action::Italic) => self.toggle_marker('_'),
            Some(Action::Strike) => self.toggle_marker('~'),
            Some(Action::Code) => self.toggle_marker('`'),
            Some(Action::DeleteWord) => self.delete_word_left(),
            Some(Action::DeleteLine) => self.kill_line(),
            Some(Action::LineStart) => self.home(),
            Some(Action::LineEnd) => self.end(),
            Some(Action::WordLeft) => self.word_left(),
            Some(Action::WordRight) => self.word_right(),
            Some(Action::Left) => self.left(),
            Some(Action::Right) => self.right(),
            Some(Action::Backspace) => self.backspace(),
            Some(Action::DeleteForward) => self.delete(),
            Some(Action::Up) if self.is_empty() && self.editing.is_none() => return KeyOutcome::UpWhenEmpty,
            Some(Action::Up) => {
                self.up();
            }
            Some(Action::Down) => {
                self.down();
            }
            Some(_) => {}
            None => {
                if let (KeyCode::Char(ch), true) = (k.code, key.is_text()) {
                    self.insert(ch);
                    return KeyOutcome::Typed;
                }
            }
        }
        KeyOutcome::Done
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.iter().all(|c| c.is_whitespace())
    }

    #[cfg(test)]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn set(&mut self, text: &str) {
        self.chars = text.chars().collect();
        self.cursor = self.chars.len();
    }

    /// Take what is typed out, leaving the composer empty; None when there is nothing to keep.
    pub fn take_draft(&mut self) -> Option<Draft> {
        let draft = self.draft();
        self.clear();
        draft
    }

    /// What is typed, as a draft (None when nothing is), leaving it in place.
    pub fn draft(&self) -> Option<Draft> {
        (!self.is_empty() || self.editing.is_some()).then(|| Draft { text: self.chars.iter().collect(), cursor: self.cursor, mentions: self.mentions.clone(), editing: self.editing.clone() })
    }

    /// Put a draft back in place of what is there.
    pub fn restore(&mut self, d: Draft) {
        self.clear();
        self.chars = d.text.chars().collect();
        (self.cursor, self.mentions, self.editing) = (d.cursor.min(self.chars.len()), d.mentions, d.editing);
    }

    pub fn clear(&mut self) {
        self.chars.clear();
        self.cursor = 0;
        self.mentions.clear();
        self.editing = None;
        self.history_pos = None;
    }

    pub fn insert(&mut self, c: char) {
        self.chars.insert(self.cursor, c);
        self.cursor += 1;
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            self.insert(c);
        }
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.chars.remove(self.cursor);
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
        }
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.chars.len());
    }

    fn is_word(c: char) -> bool {
        c.is_alphanumeric() || c == '_'
    }

    pub fn word_left(&mut self) {
        while self.cursor > 0 && !Self::is_word(self.chars[self.cursor - 1]) {
            self.cursor -= 1;
        }
        while self.cursor > 0 && Self::is_word(self.chars[self.cursor - 1]) {
            self.cursor -= 1;
        }
    }

    pub fn word_right(&mut self) {
        let n = self.chars.len();
        while self.cursor < n && !Self::is_word(self.chars[self.cursor]) {
            self.cursor += 1;
        }
        while self.cursor < n && Self::is_word(self.chars[self.cursor]) {
            self.cursor += 1;
        }
    }

    pub fn delete_word_left(&mut self) {
        let end = self.cursor;
        self.word_left();
        self.chars.drain(self.cursor..end);
    }

    fn line_start(&self) -> usize {
        self.chars[..self.cursor].iter().rposition(|&c| c == '\n').map(|i| i + 1).unwrap_or(0)
    }

    fn line_end(&self) -> usize {
        self.chars[self.cursor..].iter().position(|&c| c == '\n').map(|i| self.cursor + i).unwrap_or(self.chars.len())
    }

    pub fn home(&mut self) {
        self.cursor = self.line_start();
    }

    pub fn end(&mut self) {
        self.cursor = self.line_end();
    }

    pub fn kill_line(&mut self) {
        let start = self.line_start();
        self.chars.drain(start..self.cursor);
        self.cursor = start;
    }

    /// Move to the previous line; returns false if already on the first line.
    pub fn up(&mut self) -> bool {
        let start = self.line_start();
        if start == 0 {
            return false;
        }
        let col = self.cursor - start;
        let prev_end = start - 1;
        let prev_start = self.chars[..prev_end].iter().rposition(|&c| c == '\n').map(|i| i + 1).unwrap_or(0);
        self.cursor = prev_start + col.min(prev_end - prev_start);
        true
    }

    /// Move to the next line; returns false if already on the last line.
    pub fn down(&mut self) -> bool {
        let end = self.line_end();
        if end == self.chars.len() {
            return false;
        }
        let col = self.cursor - self.line_start();
        let next_start = end + 1;
        let next_end = self.chars[next_start..].iter().position(|&c| c == '\n').map(|i| next_start + i).unwrap_or(self.chars.len());
        self.cursor = next_start + col.min(next_end - next_start);
        true
    }

    /// Wrap the word at the cursor in `marker` (`*`, `_`, `~`, `` ` ``); with no word,
    /// insert a pair and place the cursor inside.
    pub fn toggle_marker(&mut self, marker: char) {
        let n = self.chars.len();
        let mut start = self.cursor;
        while start > 0 && !self.chars[start - 1].is_whitespace() {
            start -= 1;
        }
        let mut end = self.cursor;
        while end < n && !self.chars[end].is_whitespace() {
            end += 1;
        }
        if start == end {
            self.insert(marker);
            self.insert(marker);
            self.cursor -= 1;
            return;
        }
        let wrapped = self.chars[start] == marker && self.chars[end - 1] == marker && end - start >= 2;
        if wrapped {
            self.chars.remove(end - 1);
            self.chars.remove(start);
            self.cursor = (end - 2).min(self.chars.len());
        } else {
            self.chars.insert(end, marker);
            self.chars.insert(start, marker);
            self.cursor = end + 2;
        }
    }

    /// The autocomplete token immediately before the cursor, if any.
    pub fn trigger(&self) -> Option<(usize, Trigger)> {
        let mut start = self.cursor;
        while start > 0 {
            let c = self.chars[start - 1];
            if c.is_whitespace() {
                break;
            }
            start -= 1;
            if matches!(c, '@' | '#' | ':') {
                // The sigil must start a word.
                if start > 0 && !self.chars[start - 1].is_whitespace() {
                    return None;
                }
                let word: String = self.chars[start + 1..self.cursor].iter().collect();
                return match c {
                    '@' => Some((start, Trigger::User(word))),
                    '#' => Some((start, Trigger::Channel(word))),
                    _ if word.chars().count() >= 2 && !word.contains(':') => Some((start, Trigger::Emoji(word))),
                    _ => None,
                };
            }
        }
        None
    }

    /// Replace the token starting at `start` (up to the cursor) with `replacement`.
    /// `id` records a mention to resolve on send.
    pub fn complete(&mut self, start: usize, replacement: &str, id: Option<&str>) {
        self.chars.drain(start..self.cursor);
        self.cursor = start;
        self.insert_str(replacement);
        if let Some(id) = id {
            self.mentions.push((replacement.trim_end().to_string(), id.to_string()));
        }
    }

    /// Load a sent message (Slack mrkdwn) for editing message `ts`: `<@U…>`, `<#C…|name>`
    /// and `<!here>` become `@display`, `#name` and `@here` and are registered as mentions;
    /// links stay as they are and `&amp;`/`&lt;`/`&gt;` are unescaped.
    pub fn load_for_edit(&mut self, ts: &str, text: &str, user_name: impl Fn(&str) -> String, channel_name: impl Fn(&str) -> String) {
        self.clear();
        let mut out = String::new();
        let mut rest = text;
        while let Some(pos) = rest.find('<') {
            out.push_str(&slack_core::format::unescape(&rest[..pos]));
            let Some(len) = rest[pos..].find('>') else {
                rest = &rest[pos..];
                break;
            };
            let inner = &rest[pos + 1..pos + len];
            rest = &rest[pos + len + 1..];
            let (target, label) = match inner.split_once('|') {
                Some((t, l)) => (t, Some(l)),
                None => (inner, None),
            };
            let mention = if let Some(id) = target.strip_prefix('@') {
                Some((format!("@{}", label.map(String::from).unwrap_or_else(|| user_name(id))), id))
            } else if let Some(id) = target.strip_prefix('#') {
                Some((format!("#{}", label.map(String::from).unwrap_or_else(|| channel_name(id))), id))
            } else if matches!(target, "!here" | "!channel" | "!everyone") {
                Some((format!("@{}", &target[1..]), target))
            } else {
                None
            };
            match mention {
                Some((display, id)) => {
                    out.push_str(&display);
                    self.mentions.push((display, id.to_string()));
                }
                None => {
                    out.push('<');
                    out.push_str(inner);
                    out.push('>');
                }
            }
        }
        out.push_str(&slack_core::format::unescape(rest));
        self.set(&out);
        self.editing = Some(ts.to_string());
    }

    /// Final mrkdwn for the API: autocompleted mentions become `<@U…>` / `<#C…>` / `<!here>`.
    pub fn to_mrkdwn(&self) -> String {
        let mut text = self.text().trim_end().to_string();
        let mut mentions = self.mentions.clone();
        // Longest first so `@ann` does not clobber `@anna`.
        mentions.sort_by_key(|(display, _)| std::cmp::Reverse(display.len()));
        for (display, id) in mentions {
            let replacement = if display.starts_with('#') {
                format!("<#{id}>")
            } else if id.starts_with('!') {
                format!("<{id}>")
            } else {
                format!("<@{id}>")
            };
            text = replace_whole_token(&text, &display, &replacement);
        }
        // Slack expects these three escaped in text.
        escape_text(&text)
    }

    /// Remember a sent message for Up-arrow recall.
    pub fn push_history(&mut self, text: String) {
        if !text.trim().is_empty() {
            self.history.push(text);
        }
        self.history_pos = None;
    }

    /// Cursor (row, col) in display cells after wrapping at `width`.
    pub fn cursor_position(&self, width: usize) -> (u16, u16) {
        let width = width.max(1);
        let (mut row, mut col) = (0usize, 0usize);
        for &c in &self.chars[..self.cursor] {
            if c == '\n' {
                row += 1;
                col = 0;
                continue;
            }
            let w = c.width().unwrap_or(0);
            if col + w > width {
                row += 1;
                col = 0;
            }
            col += w;
        }
        if col >= width {
            row += 1;
            col = 0;
        }
        (row as u16, col as u16)
    }

    /// Rows needed to show the text at `width`.
    pub fn rows(&self, width: usize) -> u16 {
        let width = width.max(1);
        let (mut rows, mut col) = (1usize, 0usize);
        for &c in &self.chars {
            if c == '\n' {
                rows += 1;
                col = 0;
                continue;
            }
            let w = c.width().unwrap_or(0);
            if col + w > width {
                rows += 1;
                col = 0;
            }
            col += w;
        }
        rows.max(self.cursor_position(width).0 as usize + 1) as u16
    }
}

fn replace_whole_token(text: &str, token: &str, replacement: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(pos) = rest.find(token) {
        let before = rest[..pos].chars().last().or_else(|| out.chars().last());
        let before_ok = before.is_none_or(|c| c.is_whitespace() || c == '(');
        let after = &rest[pos + token.len()..];
        let after_ok = after.chars().next().is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '.' && after.chars().nth(1).is_some_and(char::is_alphanumeric)));
        out.push_str(&rest[..pos]);
        if before_ok && after_ok {
            out.push_str(replacement);
        } else {
            out.push_str(token);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Escape `&`, `<`, `>` but keep `<@…>`, `<#…>`, `<!…>` and `<http…>` entities intact.
pub fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '<' {
            let rest: String = chars[i + 1..].iter().collect();
            let is_entity = ["@", "#", "!", "http://", "https://", "mailto:"].iter().any(|p| rest.starts_with(p));
            if let (true, Some(end)) = (is_entity, rest.find('>')) {
                out.push('<');
                out.push_str(&rest[..end]);
                out.push('>');
                i += rest[..end].chars().count() + 2;
                continue;
            }
            out.push_str("&lt;");
        } else if c == '>' {
            out.push_str("&gt;");
        } else if c == '&' {
            out.push_str("&amp;");
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    #[test]
    fn editing_and_lines() {
        let mut c = Composer::default();
        c.insert_str("hello\nworld");
        assert!(c.up());
        assert_eq!(c.cursor(), 5);
        assert!(!c.up());
        assert!(c.down());
        c.end();
        c.delete_word_left();
        assert_eq!(c.text(), "hello\n");
        c.backspace();
        assert_eq!(c.text(), "hello");
    }

    #[test]
    fn drafts() {
        let mut c = Composer::default();
        assert_eq!(c.take_draft(), None, "nothing typed");
        c.set("  \n");
        assert_eq!(c.take_draft(), None, "only whitespace");
        c.set("hello world");
        c.left();
        c.mentions.push(("@bob".into(), "U2".into()));
        c.history.push("sent before".into());
        let d = c.take_draft().expect("a draft");
        assert!(c.is_empty() && c.mentions.is_empty());
        assert_eq!(c.history, vec!["sent before".to_string()], "the history stays");
        c.set("other");
        c.restore(d.clone());
        assert_eq!((c.text(), c.cursor(), c.mentions.clone()), ("hello world".to_string(), 10, vec![("@bob".to_string(), "U2".to_string())]));
        // An edit in progress is kept even when its text was cleared.
        c.clear();
        c.editing = Some("10.0".into());
        let d = c.take_draft().expect("the edit");
        assert_eq!(c.editing, None);
        c.restore(d);
        assert_eq!(c.editing.as_deref(), Some("10.0"));
    }

    #[test]
    fn key_outcomes() {
        let key = |code, mods| KeyEvent::new(code, mods);
        let keys = Keymap::load("").0;
        let mut c = Composer::default();
        assert_eq!(c.handle_key(key(KeyCode::Up, KeyModifiers::NONE), &keys), KeyOutcome::UpWhenEmpty);
        assert_eq!(c.handle_key(key(KeyCode::Char('h'), KeyModifiers::NONE), &keys), KeyOutcome::Typed);
        assert_eq!(c.handle_key(key(KeyCode::Enter, KeyModifiers::ALT), &keys), KeyOutcome::Done);
        assert_eq!(c.text(), "h\n");
        assert_eq!(c.handle_key(key(KeyCode::Enter, KeyModifiers::NONE), &keys), KeyOutcome::Submit);
        assert_eq!(c.handle_key(key(KeyCode::Esc, KeyModifiers::NONE), &keys), KeyOutcome::Leave);
        c.editing = Some("1.0".into());
        assert_eq!(c.handle_key(key(KeyCode::Esc, KeyModifiers::NONE), &keys), KeyOutcome::Done);
        assert!(c.editing.is_none() && c.text().is_empty());
        // Rebound: Ctrl+S sends, Enter does nothing, Ctrl+W is no longer delete-word.
        let (keys, warnings) = Keymap::load("[composer]\nsend = \"ctrl+s\"\ndelete_word = \"alt+w\"\n");
        assert!(warnings.is_empty(), "{warnings:?}");
        c.insert_str("two words");
        assert_eq!(c.handle_key(key(KeyCode::Enter, KeyModifiers::NONE), &keys), KeyOutcome::Done);
        assert_eq!(c.handle_key(key(KeyCode::Char('s'), KeyModifiers::CONTROL), &keys), KeyOutcome::Submit);
        c.handle_key(key(KeyCode::Char('w'), KeyModifiers::CONTROL), &keys);
        assert_eq!(c.text(), "two words");
        c.handle_key(key(KeyCode::Char('w'), KeyModifiers::ALT), &keys);
        assert_eq!(c.text(), "two ");
    }

    #[test]
    fn markers() {
        let mut c = Composer::default();
        c.insert_str("make bold");
        c.toggle_marker('*');
        assert_eq!(c.text(), "make *bold*");
        c.left();
        c.toggle_marker('*');
        assert_eq!(c.text(), "make bold");
        let mut e = Composer::default();
        e.toggle_marker('_');
        e.insert_str("it");
        assert_eq!(e.text(), "_it_");
    }

    #[test]
    fn triggers_and_completion() {
        let mut c = Composer::default();
        c.insert_str("hi @an");
        assert_eq!(c.trigger(), Some((3, Trigger::User("an".into()))));
        c.complete(3, "@Anna ", Some("U1"));
        c.insert_str("see #gen");
        let (s, t) = c.trigger().unwrap();
        assert_eq!(t, Trigger::Channel("gen".into()));
        c.complete(s, "#general ", Some("C9"));
        c.insert_str(":wa");
        assert_eq!(c.trigger().unwrap().1, Trigger::Emoji("wa".into()));
        assert_eq!(c.to_mrkdwn(), "hi <@U1> see <#C9> :wa");
        let mut n = Composer::default();
        n.insert_str("email@x");
        assert_eq!(n.trigger(), None);
    }

    #[test]
    fn load_for_edit_round_trips() {
        let mut c = Composer::default();
        let original = "hi <@U1> in <#C9|general> and <#C8>, <!here> &lt;b&gt; &amp; <https://x.y|site>";
        c.load_for_edit("5.0", original, |id| if id == "U1" { "Anna Bee".into() } else { id.into() }, |id| if id == "C8" { "random".into() } else { id.into() });
        assert_eq!(c.text(), "hi @Anna Bee in #general and #random, @here <b> & <https://x.y|site>");
        assert_eq!(c.editing.as_deref(), Some("5.0"));
        assert_eq!(c.to_mrkdwn(), "hi <@U1> in <#C9> and <#C8>, <!here> &lt;b&gt; &amp; <https://x.y|site>");
    }

    #[test]
    fn escaping_keeps_entities() {
        assert_eq!(escape_text("a < b && <@U1> <https://x.y> c>"), "a &lt; b &amp;&amp; <@U1> <https://x.y> c&gt;");
    }

    #[test]
    fn cursor_wrap() {
        let mut c = Composer::default();
        c.insert_str("abcdef");
        assert_eq!(c.cursor_position(4), (1, 2));
        assert_eq!(c.rows(4), 2);
    }
}

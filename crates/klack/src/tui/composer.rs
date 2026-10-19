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
    /// Ctrl+V: paste from the system clipboard.
    Paste,
}

/// What an autocomplete token looks like at the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trigger {
    /// A command name at the start of the text (without its `/`).
    Command(String),
    /// The path typed so far after `/upload` (without its opening quote when `quoted`).
    Path { partial: String, quoted: bool },
    /// A word typed after a command (`partial`), with the command and the words before it.
    Argument { command: String, before: Vec<String>, partial: String },
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
    /// Kept while klack runs, not saved with the text.
    #[serde(skip)]
    attachments: Vec<Attachment>,
}

impl Draft {
    /// Whether it is worth saving across runs (attachments are not saved).
    pub fn has_text(&self) -> bool {
        !self.text.trim().is_empty() || self.editing.is_some()
    }

    pub fn attachments(&self) -> Vec<Attachment> {
        self.attachments.clone()
    }
}

/// A file waiting in a composer to be sent with the message (pasted with Ctrl+V).
#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    pub name: String,
    pub size: u64,
    pub data: AttachmentData,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AttachmentData {
    /// A file on disk (copied in a file manager), read when sent.
    File(std::path::PathBuf),
    /// A pasted picture, as PNG.
    Png(std::sync::Arc<Vec<u8>>),
}

/// Attachment rows above the text: one per attachment up to this many, else one fewer and
/// "… and N more".
const ATTACHMENT_ROWS: usize = 4;

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
    /// Sent with the message.
    pub attachments: Vec<Attachment>,
    /// The width the text was last drawn at (↑/↓ go by its rows); 0 before the first draw.
    pub wrap_width: std::cell::Cell<usize>,
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
            Some(Action::Backspace) if self.chars.is_empty() && !self.attachments.is_empty() => {
                self.attachments.pop();
            }
            Some(Action::Backspace) => self.backspace(),
            Some(Action::DeleteForward) => self.delete(),
            Some(Action::Up) if self.is_empty() && self.editing.is_none() => return KeyOutcome::UpWhenEmpty,
            Some(Action::Paste) => return KeyOutcome::Paste,
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
        (!self.is_empty() || self.editing.is_some() || !self.attachments.is_empty()).then(|| Draft { text: self.chars.iter().collect(), cursor: self.cursor, mentions: self.mentions.clone(), editing: self.editing.clone(), attachments: self.attachments.clone() })
    }

    /// Put a draft back in place of what is there.
    pub fn restore(&mut self, d: Draft) {
        self.clear();
        self.chars = d.text.chars().collect();
        (self.cursor, self.mentions, self.editing, self.attachments) = (d.cursor.min(self.chars.len()), d.mentions, d.editing, d.attachments);
    }

    pub fn clear(&mut self) {
        self.chars.clear();
        self.cursor = 0;
        self.mentions.clear();
        self.editing = None;
        self.history_pos = None;
        self.attachments.clear();
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

    /// To the start of the row the cursor is on, as the box draws it (wrapped rows count).
    pub fn home(&mut self) {
        let (rows, row) = self.drawn_rows();
        if let Some(&(start, _)) = rows.get(row) {
            self.cursor = start;
        }
    }

    /// To the end of the row the cursor is on; a wrapped row ends before its break.
    pub fn end(&mut self) {
        let (rows, row) = self.drawn_rows();
        if let Some(&(start, end)) = rows.get(row) {
            let wrapped = rows.get(row + 1).is_some_and(|next| next.0 == end);
            self.cursor = if wrapped && end > start { end - 1 } else { end };
        }
    }

    /// The rows at the last drawn width (by lines before the first draw) and the cursor's row.
    fn drawn_rows(&self) -> (Vec<(usize, usize)>, usize) {
        let width = match self.wrap_width.get() {
            0 => usize::MAX,
            w => w,
        };
        (self.wrap(width), self.cursor_position(width).0 as usize)
    }

    pub fn kill_line(&mut self) {
        let start = self.line_start();
        self.chars.drain(start..self.cursor);
        self.cursor = start;
    }

    /// Move to the row above as the box draws it (wrapped rows count); false on the first.
    pub fn up(&mut self) -> bool {
        self.step_row(-1)
    }

    /// Move to the row below as the box draws it; false on the last.
    pub fn down(&mut self) -> bool {
        self.step_row(1)
    }

    /// Move `delta` rows at the last drawn width, keeping the column as far as the row allows.
    fn step_row(&mut self, delta: isize) -> bool {
        let width = match self.wrap_width.get() {
            0 => usize::MAX,
            w => w,
        };
        let (rows, _) = self.drawn_rows();
        let (row, col) = self.cursor_position(width);
        let target = row as isize + delta;
        if target < 0 || target as usize >= rows.len() {
            return false;
        }
        let target = target as usize;
        let (start, end) = rows[target];
        let mut at = start;
        let mut used = 0;
        while at < end && used + self.chars[at].width().unwrap_or(0) <= col as usize {
            used += self.chars[at].width().unwrap_or(0);
            at += 1;
        }
        // The end of a wrapped row is the next row's start: stay on this one.
        if at == end && at > start && rows.get(target + 1).is_some_and(|next| next.0 == end) {
            at -= 1;
        }
        self.cursor = at;
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
        // A command name: `/` at the very start, then letters, digits, `-` and `_` up to the cursor.
        if self.chars.first() == Some(&'/') && self.cursor > 0 {
            let word = &self.chars[1..self.cursor];
            if word.iter().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')) {
                return Some((0, Trigger::Command(word.iter().collect())));
            }
        }
        if let Some(found) = self.upload_path().or_else(|| self.command_argument()) {
            return Some(found);
        }
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

    /// The path being typed after `/upload`: from its start (with an opening quote) to the
    /// cursor, while the cursor is still inside it.
    fn upload_path(&self) -> Option<(usize, Trigger)> {
        let before: String = self.chars[..self.cursor].iter().collect();
        let rest = before.strip_prefix("/upload")?;
        let arg = rest.trim_start();
        if rest.len() == arg.len() {
            return None;
        }
        let start = self.cursor - arg.chars().count();
        match arg.strip_prefix('"') {
            Some(partial) if !partial.contains('"') => Some((start, Trigger::Path { partial: partial.into(), quoted: true })),
            Some(_) => None,
            None if !arg.contains(char::is_whitespace) => Some((start, Trigger::Path { partial: arg.into(), quoted: false })),
            None => None,
        }
    }

    /// The word at the cursor after a command and what comes before it; words starting with
    /// `@`, `#` or `:` are mentions, channels and emoji instead.
    fn command_argument(&self) -> Option<(usize, Trigger)> {
        let before: String = self.chars[..self.cursor].iter().collect();
        if !before.starts_with('/') || before.contains('\n') {
            return None;
        }
        let mut words: Vec<&str> = before.split(' ').collect();
        let partial = words.pop().filter(|p| !p.starts_with(['@', '#', ':']))?;
        let command = (!words.is_empty()).then(|| words.remove(0).to_string())?;
        let before = words.into_iter().filter(|w| !w.is_empty()).map(String::from).collect();
        Some((self.cursor - partial.chars().count(), Trigger::Argument { command, before, partial: partial.into() }))
    }

    /// The text from `start` up to the cursor.
    pub fn token(&self, start: usize) -> String {
        self.chars[start.min(self.cursor)..self.cursor].iter().collect()
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

    /// The text's rows at `width`, as char ranges without their `\n`: a word that doesn't
    /// fit moves whole to the next row, a word longer than a row is split, and a space that
    /// overflows hangs at the end of its row.
    fn wrap(&self, width: usize) -> Vec<(usize, usize)> {
        let width = width.max(1);
        let mut rows = Vec::new();
        let (mut start, mut col, mut brk) = (0, 0, None);
        for (i, &c) in self.chars.iter().enumerate() {
            if c == '\n' {
                rows.push((start, i));
                (start, col, brk) = (i + 1, 0, None);
                continue;
            }
            let w = c.width().unwrap_or(0);
            if col > 0 && col + w > width {
                if c == ' ' {
                    rows.push((start, i + 1));
                    (start, col, brk) = (i + 1, 0, None);
                    continue;
                }
                let at = brk.unwrap_or(i);
                rows.push((start, at));
                start = at;
                col = self.chars[at..i].iter().map(|c| c.width().unwrap_or(0)).sum();
                brk = None;
            }
            col += w;
            if c == ' ' {
                brk = Some(i + 1);
            }
        }
        rows.push((start, self.chars.len()));
        rows
    }

    /// The text as drawn at `width`, one string per row.
    pub fn wrapped(&self, width: usize) -> Vec<String> {
        self.wrap(width).into_iter().map(|(s, e)| self.chars[s..e].iter().collect()).collect()
    }

    /// Cursor (row, col) in display cells after wrapping at `width`.
    pub fn cursor_position(&self, width: usize) -> (u16, u16) {
        let rows = self.wrap(width);
        // At a soft break the cursor starts the next row.
        let row = rows.iter().rposition(|&(s, _)| s <= self.cursor).unwrap_or(0);
        let col: usize = self.chars[rows[row].0..self.cursor].iter().map(|c| c.width().unwrap_or(0)).sum();
        if col >= width.max(1) { (row as u16 + 1, 0) } else { (row as u16, col as u16) }
    }

    /// The attachment lines drawn above the text, and how many of them stand for one
    /// attachment each (the rest is "… and N more").
    pub fn attachment_lines(&self) -> (Vec<String>, usize) {
        let n = self.attachments.len();
        let shown = if n <= ATTACHMENT_ROWS { n } else { ATTACHMENT_ROWS - 1 };
        let mut lines: Vec<String> = self.attachments[..shown].iter().map(|a| format!("📎 {} · {}", a.name, super::text::human_size(a.size))).collect();
        if shown < n {
            lines.push(format!("📎 … and {} more", n - shown));
        }
        (lines, shown)
    }

    /// The composer's height with its border at `width` (inside the border): attachments
    /// and up to eight rows of text.
    pub fn height(&self, width: usize) -> u16 {
        self.rows(width).clamp(1, 8) + self.attachment_lines().0.len() as u16 + 2
    }

    /// Rows needed to show the text at `width`.
    pub fn rows(&self, width: usize) -> u16 {
        (self.wrap(width).len()).max(self.cursor_position(width).0 as usize + 1) as u16
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
        let mut cmd = Composer::default();
        cmd.insert_str("/");
        assert_eq!(cmd.trigger(), Some((0, Trigger::Command(String::new()))));
        cmd.insert_str("up-load_2");
        assert_eq!(cmd.trigger(), Some((0, Trigger::Command("up-load_2".into()))));
        assert_eq!(cmd.token(0), "/up-load_2");
        cmd.set("/usr/bin");
        assert_eq!(cmd.trigger(), None, "a path is not a command");
        cmd.set("/remind @an");
        assert_eq!(cmd.trigger(), Some((8, Trigger::User("an".into()))), "arguments complete as before");
        cmd.set(" /away");
        assert_eq!(cmd.trigger(), None, "a leading space: text");
        let path = |partial: &str, quoted| Some((8, Trigger::Path { partial: partial.into(), quoted }));
        cmd.set("/upload ");
        assert_eq!(cmd.trigger(), path("", false));
        cmd.set("/upload ~/Pic");
        assert_eq!(cmd.trigger(), path("~/Pic", false));
        cmd.set("/upload \"My Fi");
        assert_eq!(cmd.trigger(), path("My Fi", true), "a quoted name may hold spaces");
        cmd.set("/upload \"My File.png\" lo");
        assert!(!matches!(cmd.trigger(), Some((_, Trigger::Path { .. }))), "past the path: the message (free text)");
        cmd.set("/upload a.png @an");
        assert_eq!(cmd.trigger(), Some((14, Trigger::User("an".into()))), "mentions in the message");
        let arg = |start, command: &str, before: &[&str], partial: &str| Some((start, Trigger::Argument { command: command.into(), before: before.iter().map(|w| w.to_string()).collect(), partial: partial.into() }));
        cmd.set("/uploads x");
        assert_eq!(cmd.trigger(), arg(9, "/uploads", &[], "x"));
        cmd.set("/dnd ");
        assert_eq!(cmd.trigger(), arg(5, "/dnd", &[], ""));
        cmd.set("/status :tada: party for t");
        assert_eq!(cmd.trigger(), arg(25, "/status", &[":tada:", "party", "for"], "t"));
        cmd.set("/status :ta");
        assert_eq!(cmd.trigger(), Some((8, Trigger::Emoji("ta".into()))), "emoji as before");
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

    #[test]
    fn attachments_wait_in_the_composer() {
        let keys = Keymap::load("").0;
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        let file = |name: &str| Attachment { name: name.into(), size: 2048, data: AttachmentData::File(name.into()) };
        let mut c = Composer::default();
        c.attachments = vec![file("a.png"), file("b.pdf")];
        assert_eq!(c.attachment_lines(), (vec!["📎 a.png · 2.0 KB".to_string(), "📎 b.pdf · 2.0 KB".into()], 2));
        assert_eq!(c.height(20), 5, "two attachments, one row of text, the border");
        let draft = c.take_draft().expect("attachments alone are a draft");
        assert!(!draft.has_text(), "not saved across runs");
        assert!(c.attachments.is_empty());
        c.restore(draft);
        c.insert('x');
        c.handle_key(key(KeyCode::Backspace), &keys);
        assert_eq!((c.text(), c.attachments.len()), (String::new(), 2), "Backspace deletes text first");
        c.handle_key(key(KeyCode::Backspace), &keys);
        assert_eq!(c.attachments, vec![file("a.png")], "then the last attachment");
        c.attachments = (0..6).map(|i| file(&format!("{i}.txt"))).collect();
        let (lines, shown) = c.attachment_lines();
        assert_eq!((lines.len(), shown, lines[3].as_str()), (4, 3, "📎 … and 3 more"));
        c.clear();
        assert!(c.attachments.is_empty(), "sent or cancelled with the text");
    }

    #[test]
    fn up_and_down_go_by_drawn_rows() {
        let mut c = Composer::default();
        c.insert_str("hello wonderful world\nend");
        c.wrap_width.set(12);
        assert_eq!(c.wrapped(12), ["hello ", "wonderful ", "world", "end"]);
        assert_eq!(c.cursor_position(12), (3, 3));
        assert!(c.up());
        assert_eq!(c.cursor_position(12), (2, 3), "over a newline, as before");
        assert!(c.up());
        assert_eq!(c.cursor_position(12), (1, 3), "and over a wrap (it did nothing)");
        assert!(c.up());
        assert_eq!(c.cursor_position(12), (0, 3));
        assert!(!c.up(), "the first row");
        // The end of a wrapped row stays on it.
        c.cursor = 15;
        assert_eq!(c.cursor_position(12), (1, 9), "after \"wonderful\"");
        assert!(c.up());
        assert_eq!((c.cursor, c.cursor_position(12)), (5, (0, 5)), "\"hello\" is shorter: its end, not the next row");
        assert!(c.down() && c.down() && c.down());
        assert_eq!(c.cursor_position(12), (3, 3), "the last row, as far as it goes");
        assert!(!c.down());
        // Home / End: the drawn row's start and end.
        c.cursor = 9;
        c.home();
        assert_eq!(c.cursor, 6, "\"wonderful\" starts its row (it did go to the line's start)");
        c.end();
        assert_eq!((c.cursor, c.cursor_position(12)), (15, (1, 9)), "the wrapped row's end, before the break");
        c.cursor = 18;
        c.home();
        assert_eq!(c.cursor, 16);
        c.end();
        assert_eq!(c.cursor, 21, "the last row of the line: its end");
        c.cursor = 23;
        c.end();
        assert_eq!(c.cursor, 25, "after the newline: \"end\"");
        // Not drawn yet: by lines.
        let mut fresh = Composer::default();
        fresh.insert_str("one two three four five six\nb");
        assert!(fresh.up());
        assert_eq!(fresh.cursor, 1);
    }

    #[test]
    fn the_cursor_follows_wrapped_words() {
        let mut c = Composer::default();
        c.insert_str("hello wonderful world");
        assert_eq!(c.wrapped(12), ["hello ", "wonderful ", "world"]);
        assert_eq!(c.cursor_position(12), (2, 5), "after the last word");
        c.set("hello wonderful");
        c.cursor = 6;
        assert_eq!(c.cursor_position(12), (1, 0), "at a soft break: the start of the next row");
        c.set("abcd efgh");
        assert_eq!(c.wrapped(4), ["abcd ", "efgh"], "a space that doesn't fit hangs");
        c.cursor = 4;
        assert_eq!(c.cursor_position(4), (1, 0), "a full row puts the cursor on the next one");
        c.set("a\nlongerword x");
        assert_eq!(c.wrapped(4), ["a", "long", "erwo", "rd x"], "long words are split");
        assert_eq!(c.cursor_position(4), (4, 0), "the cursor wraps past a full last row");
        assert_eq!(c.rows(4), 5);
        c.set("ab\n");
        assert_eq!((c.wrapped(4).len(), c.cursor_position(4)), (2, (1, 0)), "a trailing newline");
        c.set("");
        assert_eq!((c.wrapped(4), c.cursor_position(4), c.rows(4)), (vec![String::new()], (0, 0), 1));
    }
}

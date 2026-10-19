//! The save dialog (`s`): a message's files, ticked to be saved (all to begin with), and the
//! folder they go to, which can be edited (Tab there completes it, as `/upload` does paths).

use std::path::PathBuf;

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use slack_core::types::File;

/// File rows shown at once (the list scrolls).
const ROWS: usize = 10;
/// Folder suggestions shown under the folder line while it is edited.
const SUGGESTIONS: usize = 4;

pub struct SaveDialog {
    pub files: Vec<File>,
    pub ticked: Vec<bool>,
    pub selected: usize,
    /// Where they go, as typed (`~` is home).
    pub folder: String,
    /// The folder line has the keys.
    pub editing: bool,
    /// The popup and its file rows as last drawn, and the first file shown (for the mouse).
    pub area: Rect,
    rows: Rect,
    first: usize,
    /// The row the pointer is over (not the selection), at the last `Moved`.
    pub hover: Option<usize>,
}

impl SaveDialog {
    /// Every file ticked, going to `folder` (shown with `~` for home).
    pub fn new(files: Vec<File>, folder: &std::path::Path) -> SaveDialog {
        let ticked = vec![true; files.len()];
        SaveDialog { files, ticked, selected: 0, folder: shown_folder(folder), editing: false, area: Rect::default(), rows: Rect::default(), first: 0, hover: None }
    }

    pub fn move_by(&mut self, delta: isize) {
        self.selected = (self.selected as isize + delta).clamp(0, self.files.len().saturating_sub(1) as isize) as usize;
    }

    pub fn tick(&mut self) {
        if let Some(t) = self.ticked.get_mut(self.selected) {
            *t = !*t;
        }
    }

    /// Tick them all, or none when all are.
    pub fn tick_all(&mut self) {
        let all = self.ticked.iter().all(|t| *t);
        self.ticked.iter_mut().for_each(|t| *t = !all);
    }

    pub fn chosen(&self) -> Vec<File> {
        self.files.iter().zip(&self.ticked).filter(|(_, t)| **t).map(|(f, _)| f.clone()).collect()
    }

    /// The folder typed, with `~` for home.
    pub fn folder_path(&self) -> PathBuf {
        expand_home(self.folder.trim())
    }

    /// The file row drawn at a terminal position.
    pub fn row_at(&self, column: u16, row: u16) -> Option<usize> {
        self.rows.contains((column, row).into()).then(|| self.first + (row - self.rows.y) as usize).filter(|n| *n < self.files.len())
    }

    /// The pointer moved to a position: hover the file there (none off the list).
    pub fn hover_at(&mut self, column: u16, row: u16) {
        self.hover = self.row_at(column, row);
    }

    /// Folders the folder line could go on to.
    fn suggestions(&self) -> Vec<String> {
        super::autocomplete::path_suggestions(&self.folder, false).into_iter().filter(|s| super::autocomplete::ends_open(&s.replacement)).map(|s| s.replacement.trim_start_matches('"').to_string()).collect()
    }

    /// Tab on the folder line: the one folder it can go on to, or as far as all of them agree.
    pub fn complete(&mut self) {
        let options = self.suggestions();
        let common = options.iter().skip(1).fold(options.first().cloned().unwrap_or_default(), |acc, o| acc.chars().zip(o.chars()).take_while(|(a, b)| a == b).map(|(a, _)| a).collect());
        if common.chars().count() > self.folder.chars().count() {
            self.folder = common;
        }
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect) {
        let p = super::theme::p();
        let shown = self.files.len().min(ROWS);
        let hints = if self.editing { self.suggestions() } else { Vec::new() };
        let height = (shown + 4 + hints.len().min(SUGGESTIONS)) as u16;
        let [popup] = Layout::horizontal([Constraint::Length(70.min(area.width))]).flex(Flex::Center).areas(area);
        let [popup] = Layout::vertical([Constraint::Length(height.min(area.height))]).flex(Flex::Center).areas(popup);
        self.area = popup;
        super::media::clear_modal(f, popup);
        let ticked = self.ticked.iter().filter(|t| **t).count();
        let title = format!(" Save {ticked} of {} ", self.files.len());
        let block = Block::default().borders(Borders::ALL).border_style(Style::default().fg(p.accent)).title(title);
        let inner = block.inner(popup);
        f.render_widget(block, popup);
        self.first = self.selected.saturating_sub(shown.saturating_sub(1));
        self.rows = Rect { height: shown as u16, ..inner };
        let width = inner.width as usize;
        let mut lines: Vec<Line> = self
            .files
            .iter()
            .zip(&self.ticked)
            .enumerate()
            .skip(self.first)
            .take(shown)
            .map(|(n, (file, ticked))| {
                let size = super::text::human_size(file.size);
                let name = file.name.clone();
                let room = width.saturating_sub(6 + size.chars().count() + 2);
                let name: String = if name.chars().count() > room { name.chars().take(room.saturating_sub(1)).chain(['…']).collect() } else { name };
                let pad = " ".repeat(room.saturating_sub(name.chars().count()));
                let line = Line::from(vec![Span::raw(format!(" [{}] ", if *ticked { "x" } else { " " })), Span::raw(name), Span::raw(pad), Span::styled(format!("  {size}"), Style::default().fg(p.dim))]);
                if n == self.selected && !self.editing { line.style(Style::default().bg(p.selection).add_modifier(Modifier::BOLD)) } else if self.hover == Some(n) { line.style(super::theme::hover_style()) } else { line }
            })
            .collect();
        lines.push(Line::raw(""));
        let folder_style = if self.editing { Style::default().bg(p.hover) } else { Style::default() };
        // A long folder shows its end, where typing happens.
        let room = width.saturating_sub(6);
        let folder = tail(&self.folder, room);
        lines.push(Line::from(vec![Span::styled(" to: ", Style::default().fg(p.dim)), Span::styled(folder.clone(), folder_style)]));
        for h in hints.iter().take(SUGGESTIONS) {
            lines.push(Line::styled(format!("     {}", tail(h, room)), Style::default().fg(p.dim)));
        }
        f.render_widget(Paragraph::new(lines), inner);
        if self.editing {
            let x = inner.x + 5 + unicode_width::UnicodeWidthStr::width(folder.as_str()) as u16;
            f.set_cursor_position((x.min(inner.right().saturating_sub(1)), inner.y + shown as u16 + 1));
        }
    }
}

/// The end of `text` that fits in `room` cells, after "…" when cut.
fn tail(text: &str, room: usize) -> String {
    if unicode_width::UnicodeWidthStr::width(text) <= room {
        return text.to_string();
    }
    let mut kept: Vec<char> = Vec::new();
    let mut width = 1;
    for c in text.chars().rev() {
        let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if width + w > room {
            break;
        }
        width += w;
        kept.push(c);
    }
    std::iter::once('…').chain(kept.into_iter().rev()).collect()
}

/// A folder as shown: home as `~`, ending in `/`.
pub fn shown_folder(path: &std::path::Path) -> String {
    let home = directories::UserDirs::new().map(|d| d.home_dir().to_path_buf());
    let text = match home.as_ref().and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    };
    if text.ends_with('/') { text } else { format!("{text}/") }
}

/// `~` and `~/…` as the home folder.
pub fn expand_home(text: &str) -> PathBuf {
    match text.strip_prefix('~').filter(|rest| rest.is_empty() || rest.starts_with('/')) {
        Some(rest) => directories::UserDirs::new().map(|d| d.home_dir().join(rest.trim_start_matches('/'))).unwrap_or_else(|| PathBuf::from(text)),
        None => PathBuf::from(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> Vec<File> {
        ["one.png", "two.png", "report.pdf"].iter().map(|n| File { id: n.to_string(), name: n.to_string(), size: 2048, ..Default::default() }).collect()
    }

    #[test]
    fn ticking_files() {
        let mut d = SaveDialog::new(files(), std::path::Path::new("/tmp/x"));
        assert_eq!(d.chosen().len(), 3, "all ticked to begin with");
        assert_eq!(d.folder, "/tmp/x/");
        d.move_by(2);
        d.tick();
        assert_eq!(d.chosen().iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["one.png", "two.png"]);
        d.tick_all();
        assert_eq!(d.chosen().len(), 3, "not all ticked: all");
        d.tick_all();
        assert!(d.chosen().is_empty(), "all ticked: none");
        d.move_by(10);
        assert_eq!(d.selected, 2);
    }

    #[test]
    fn folders_complete_and_home_is_tilde() {
        let base = std::env::temp_dir().join(format!("klack-save-{}", std::process::id()));
        for sub in ["Pictures", "Pictures old", "Projects", "notes.txt"] {
            if sub.contains('.') {
                std::fs::create_dir_all(&base).unwrap();
                std::fs::write(base.join(sub), "x").unwrap();
            } else {
                std::fs::create_dir_all(base.join(sub)).unwrap();
            }
        }
        let mut d = SaveDialog::new(files(), &base);
        d.folder = format!("{}/Pi", base.display());
        d.complete();
        assert_eq!(d.folder, format!("{}/Pictures", base.display()), "as far as Pictures/ and Pictures old/ agree");
        d.folder = format!("{}/Pro", base.display());
        d.complete();
        assert_eq!(d.folder, format!("{}/Projects/", base.display()), "the one folder (a file does not count)");
        assert_eq!(d.folder_path(), base.join("Projects"));
        std::fs::remove_dir_all(&base).unwrap();
        let home = directories::UserDirs::new().unwrap().home_dir().to_path_buf();
        assert_eq!(shown_folder(&home.join("Downloads")), "~/Downloads/");
        assert_eq!(expand_home("~/Downloads/"), home.join("Downloads/"));
        assert_eq!(expand_home("~other"), PathBuf::from("~other"));
        assert_eq!((tail("/tmp/abc/def/", 20), tail("/tmp/abc/def/", 8)), ("/tmp/abc/def/".to_string(), "…bc/def/".to_string()), "long folders show their end");
    }

    #[test]
    fn hover_follows_the_pointer_and_leaves_the_selection() {
        let mut d = SaveDialog::new(files(), std::path::Path::new("/tmp/x"));
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 30)).unwrap();
        terminal.draw(|f| d.draw(f, f.area())).unwrap();
        let y = d.rows.y;
        d.hover_at(d.rows.x + 2, y + 1);
        assert_eq!((d.hover, d.selected), (Some(1), 0));
        d.hover_at(d.rows.x + 2, y + 3);
        assert_eq!(d.hover, None, "below the last file");
    }
}

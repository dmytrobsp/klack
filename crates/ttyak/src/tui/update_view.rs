//! The update screen: the new version, and either "Update now" (a copy the install script put in a
//! folder ttyak can write) or what to run instead. Updating downloads, checks and puts the new
//! executable in place, then says to restart: ttyak never restarts by itself.

use std::path::Path;
use std::sync::Arc;

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::i18n::tr;
use super::keymap::{Action, Context, Keymap};
use crate::install::{Install, Method};
use crate::update::{Latest, Progress};

const OUT: &str = "ttyak {version} is out ({date}); you have {current}.";
const NOTES: &str = "Release notes: {url}";
const WHAT_HAPPENS: &str = "Update now downloads it, checks it and puts it in place of {exe}. ttyak does not restart by itself: restart it when you are ready.";
const MANAGED: &str = "ttyak was installed {how}. To update it, run:";
const UNWRITABLE: &str = "ttyak cannot write {dir}, so it cannot update itself. To update it, run (as someone who can write there):";
const UNWRITABLE_YOURS: &str = "ttyak cannot write {dir}, so it cannot update itself. Once you can write there (it is your folder), run:";
const PACKAGE: &str = "ttyak was installed {how}: download the new {package} from";
const MANUAL: &str = "ttyak cannot tell how it was installed: download it from";
const DOWNLOADING: &str = "Downloading… {got} of {total}";
const DOWNLOADING_UNSIZED: &str = "Downloading… {got}";
const INSTALLING: &str = "Checking it and putting it in place…";
const CANCELLED: &str = "The download was cancelled; nothing was changed.";
pub const FAILED: &str = "Could not update: {error}";
pub const DONE: &str = "ttyak {version} is installed — restart ttyak to use it";
const NOT_RESTARTED: &str = "This ttyak keeps running until you quit it.";
const TITLE: &str = "Update ttyak";
const UPDATE_NOW: &str = "Update now";
const TRY_AGAIN: &str = "Try again";
const COPY_COMMAND: &str = "Copy the command";
const COPY_LINK: &str = "Copy the link";
const RELEASE_NOTES: &str = "Release notes";
const DOWNLOAD_PAGE: &str = "Open the download page";
const CANCEL: &str = "Cancel";
const CLOSE: &str = "Close";
/// The update screen's words, for the locale template.
pub const WORDS: &[&str] = &[
    OUT,
    NOTES,
    WHAT_HAPPENS,
    MANAGED,
    UNWRITABLE,
    UNWRITABLE_YOURS,
    PACKAGE,
    MANUAL,
    DOWNLOADING,
    DOWNLOADING_UNSIZED,
    INSTALLING,
    CANCELLED,
    FAILED,
    DONE,
    NOT_RESTARTED,
    TITLE,
    UPDATE_NOW,
    TRY_AGAIN,
    COPY_COMMAND,
    COPY_LINK,
    RELEASE_NOTES,
    DOWNLOAD_PAGE,
    CANCEL,
    CLOSE,
    "with Homebrew",
    "with Scoop",
    "with winget",
    "with Chocolatey",
    "from the AUR",
    "from a .deb package",
    "from an .rpm package",
];

/// What the screen offers for this copy of ttyak.
#[derive(Debug, Clone, PartialEq)]
pub enum Offer {
    /// ttyak can update itself.
    UpdateNow,
    /// The command to run: the package manager's, or the install script's for a folder ttyak
    /// cannot write.
    Command(String),
    /// Download the new `.deb` / `.rpm` (None: the archive) from the release page `url`.
    Download { package: Option<&'static str>, url: String },
}

/// The newest release's page, for a download while no newer version is known.
pub const RELEASES_PAGE: &str = "https://github.com/dmytrobsp/ttyak/releases/latest";

/// What `install` can do about `latest`: update itself only when the install script put it in a
/// folder it can write.
pub fn offer(install: &Install, latest: &Latest) -> Offer {
    offer_with(install, &latest.notes)
}

/// [`offer`] for a release whose page is `page` (also where F6 → Updates sends a download, with
/// no newer version known yet).
pub fn offer_with(install: &Install, page: &str) -> Offer {
    let download = |package| Offer::Download { package, url: page.to_string() };
    match install.method {
        Method::Script if install.writable => Offer::UpdateNow,
        Method::Script => {
            let dir = install.exe.parent().unwrap_or(Path::new(""));
            Offer::Command(script_command(dir, !yours(dir)))
        }
        Method::Deb => download(Some(".deb")),
        Method::Rpm => download(Some(".rpm")),
        method => match method.update_command() {
            Some(command) => Offer::Command(command.into()),
            None => download(None),
        },
    }
}

/// Where the update has got to.
#[derive(Debug)]
pub enum Stage {
    /// Nothing done yet.
    Ready,
    Downloading(Arc<Progress>),
    Cancelled,
    /// Why it failed (the old ttyak stays).
    Failed(String),
    /// The new ttyak is in place.
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    UpdateNow,
    Copy,
    Notes,
    Cancel,
    Close,
}

/// What the app does after a key or a click.
#[derive(Debug, Clone, PartialEq)]
pub enum Do {
    /// Start the self-update ([`UpdateView::started`] once it runs).
    Start,
    /// Copy this to the clipboard.
    Copy(String),
    /// Open this link in the browser.
    Open(String),
    Close,
    Nothing,
}

/// What the run-wide tasks tell the app: they outlive a workspace switch.
#[derive(Debug)]
pub enum UpdateEvent {
    /// The update notification was clicked.
    Clicked,
    /// The self-update to `version` ended (the error says what failed).
    Finished { version: semver::Version, result: Result<(), String> },
}

pub struct UpdateView {
    pub latest: Latest,
    install: Install,
    pub offer: Offer,
    pub stage: Stage,
    /// The running self-update, aborted by a cancel.
    task: Option<tokio::task::AbortHandle>,
    /// The popup and its buttons as last drawn (for the mouse).
    pub area: Rect,
    buttons_at: Vec<(Button, Rect)>,
    /// The button the pointer is over, at the last `Moved`.
    pub hover: Option<Button>,
}

impl UpdateView {
    pub fn new(latest: Latest, install: Install) -> UpdateView {
        let offer = offer(&install, &latest);
        UpdateView { latest, install, offer, stage: Stage::Ready, task: None, area: Rect::default(), buttons_at: Vec::new(), hover: None }
    }

    /// Whether a download is under way that can still be cancelled.
    fn downloading(&self) -> bool {
        matches!(&self.stage, Stage::Downloading(p) if !p.installing())
    }

    /// The buttons there are now, the first one Enter's.
    pub fn buttons(&self) -> Vec<Button> {
        match &self.stage {
            Stage::Downloading(p) if p.installing() => Vec::new(),
            Stage::Downloading(_) => vec![Button::Cancel],
            Stage::Done => vec![Button::Close],
            Stage::Ready | Stage::Cancelled | Stage::Failed(_) => match self.offer {
                Offer::UpdateNow => vec![Button::UpdateNow, Button::Notes, Button::Close],
                Offer::Command(_) => vec![Button::Copy, Button::Notes, Button::Close],
                Offer::Download { .. } => vec![Button::Notes, Button::Copy, Button::Close],
            },
        }
    }

    /// The button's words and the action whose key it shows.
    fn label(&self, b: Button) -> (&'static str, Action) {
        match b {
            Button::UpdateNow if matches!(self.stage, Stage::Failed(_)) => (TRY_AGAIN, Action::UpdateNow),
            Button::UpdateNow => (UPDATE_NOW, Action::UpdateNow),
            Button::Copy if matches!(self.offer, Offer::Download { .. }) => (COPY_LINK, Action::Copy),
            Button::Copy => (COPY_COMMAND, Action::Copy),
            Button::Notes if matches!(self.offer, Offer::Download { .. }) => (DOWNLOAD_PAGE, Action::ReleaseNotes),
            Button::Notes => (RELEASE_NOTES, Action::ReleaseNotes),
            Button::Cancel => (CANCEL, Action::Close),
            Button::Close => (CLOSE, Action::Close),
        }
    }

    /// Press `b` (nothing when it is not shown now).
    pub fn press(&mut self, b: Button) -> Do {
        if !self.buttons().contains(&b) {
            return Do::Nothing;
        }
        match b {
            Button::UpdateNow => Do::Start,
            Button::Copy => match &self.offer {
                Offer::Command(command) => Do::Copy(command.clone()),
                Offer::Download { url, .. } => Do::Copy(url.clone()),
                Offer::UpdateNow => Do::Nothing,
            },
            Button::Notes => Do::Open(self.latest.notes.clone()),
            Button::Cancel => {
                self.cancel();
                Do::Nothing
            }
            Button::Close => Do::Close,
        }
    }

    /// The update screen's `action` (a key).
    pub fn key(&mut self, action: Action) -> Do {
        match action {
            Action::Open => match self.buttons().first() {
                Some(b) => self.press(*b),
                None => Do::Nothing,
            },
            Action::UpdateNow => self.press(Button::UpdateNow),
            Action::Copy => self.press(Button::Copy),
            Action::ReleaseNotes => self.press(Button::Notes),
            Action::Close if self.close() => Do::Close,
            _ => Do::Nothing,
        }
    }

    /// The close key or a click outside: cancels a download (the screen stays, saying so), else
    /// true (close it; a self-update past its download goes on and says how it ended).
    pub fn close(&mut self) -> bool {
        if self.downloading() {
            self.cancel();
            return false;
        }
        true
    }

    fn cancel(&mut self) {
        if let Stage::Downloading(p) = &self.stage {
            if p.cancel() {
                if let Some(task) = self.task.take() {
                    task.abort();
                }
                self.stage = Stage::Cancelled;
            }
        }
    }

    /// The self-update runs as `task`, telling how far it is in `progress`.
    pub fn started(&mut self, progress: Arc<Progress>, task: tokio::task::AbortHandle) {
        self.stage = Stage::Downloading(progress);
        self.task = Some(task);
    }

    /// The self-update ended with `result`.
    pub fn finished(&mut self, result: Result<(), String>) {
        // (A cancel already said so.)
        if matches!(self.stage, Stage::Cancelled) {
            return;
        }
        self.task = None;
        self.stage = match result {
            Ok(()) => Stage::Done,
            Err(e) => Stage::Failed(e),
        };
    }

    /// What the screen says, a paragraph a line (`` ` `` before a command or a link: shown
    /// apart, in the accent colour).
    pub fn lines(&self) -> Vec<String> {
        let version = self.latest.version.to_string();
        if matches!(self.stage, Stage::Done) {
            return vec![tr(DONE).replace("{version}", &version), tr(NOT_RESTARTED).into_owned()];
        }
        let mut out = vec![tr(OUT).replace("{version}", &version).replace("{date}", &self.latest.date).replace("{current}", env!("CARGO_PKG_VERSION"))];
        let how = tr(self.install.method.how());
        match &self.offer {
            Offer::UpdateNow => {
                out.push(tr(NOTES).replace("{url}", &self.latest.notes));
                out.push(tr(WHAT_HAPPENS).replace("{exe}", &shown(&self.install.exe)));
            }
            Offer::Command(command) => {
                out.push(tr(NOTES).replace("{url}", &self.latest.notes));
                let dir = self.install.exe.parent().map(shown).unwrap_or_default();
                let script = if self.install.exe.parent().is_some_and(yours) { UNWRITABLE_YOURS } else { UNWRITABLE };
                out.push(if self.install.method == Method::Script { tr(script).replace("{dir}", &dir) } else { tr(MANAGED).replace("{how}", &how) });
                out.push(format!("`{command}"));
            }
            Offer::Download { package, url } => {
                out.push(match package {
                    Some(package) => tr(PACKAGE).replace("{how}", &how).replace("{package}", package),
                    None => tr(MANUAL).into_owned(),
                });
                out.push(format!("`{url}"));
            }
        }
        match &self.stage {
            Stage::Downloading(p) if p.installing() => out.push(tr(INSTALLING).into_owned()),
            Stage::Downloading(p) => out.push(match p.bytes() {
                (got, Some(total)) => tr(DOWNLOADING).replace("{got}", &super::text::human_size(got)).replace("{total}", &super::text::human_size(total)),
                (got, None) => tr(DOWNLOADING_UNSIZED).replace("{got}", &super::text::human_size(got)),
            }),
            Stage::Cancelled => out.push(tr(CANCELLED).into_owned()),
            Stage::Failed(e) => out.push(tr(FAILED).replace("{error}", e)),
            Stage::Ready | Stage::Done => {}
        }
        out
    }

    /// The button drawn at a terminal position.
    pub fn button_at(&self, column: u16, row: u16) -> Option<Button> {
        self.buttons_at.iter().find(|(_, r)| r.contains((column, row).into())).map(|(b, _)| *b)
    }

    /// The pointer moved to a position: hover the button there.
    pub fn hover_at(&mut self, column: u16, row: u16) {
        self.hover = self.button_at(column, row);
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, keys: &Keymap) {
        let p = super::theme::p();
        let width = 76.min(area.width);
        let room = width.saturating_sub(4) as usize;
        let mut body: Vec<Line<'static>> = Vec::new();
        for (n, text) in self.lines().into_iter().enumerate() {
            if n > 0 && !text.starts_with('`') {
                body.push(Line::raw(""));
            }
            // A command or a link: indented, its wrapped rows more so.
            let (style, text, indent) = match text.strip_prefix('`') {
                Some(apart) => (Style::default().fg(p.accent).add_modifier(Modifier::BOLD), apart.to_string(), ("  ", "    ")),
                None => (Style::default(), text, ("", "")),
            };
            body.extend(super::text::wrap(vec![Span::styled(text, style)], room, vec![Span::raw(indent.0)], vec![Span::raw(indent.1)]));
        }
        if let Stage::Downloading(progress) = &self.stage {
            if let (got, Some(total)) = progress.bytes() {
                let filled = (room as u64 * got.min(total) / total.max(1)) as usize;
                body.push(Line::styled(format!("{}{}", "█".repeat(filled), "░".repeat(room - filled)), Style::default().fg(p.good)));
            }
        }
        let buttons = self.buttons();
        let height = (body.len() + if buttons.is_empty() { 2 } else { 4 }) as u16;
        let [popup] = Layout::horizontal([Constraint::Length(width)]).flex(Flex::Center).areas(area);
        let [popup] = Layout::vertical([Constraint::Length(height.min(area.height))]).flex(Flex::Center).areas(popup);
        self.area = popup;
        super::media::clear_modal(f, popup);
        let block = Block::default().borders(Borders::ALL).border_style(Style::default().fg(p.accent)).title(format!(" {} ", tr(TITLE)));
        let inner = block.inner(popup);
        f.render_widget(block, popup);
        let inner = Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner };
        f.render_widget(Paragraph::new(body.clone()), inner);
        // The buttons on the last row, each with its key.
        self.buttons_at.clear();
        let y = inner.y + body.len() as u16 + 1;
        if buttons.is_empty() || y >= inner.bottom() {
            return;
        }
        let mut x = inner.x;
        for b in buttons {
            let (words, action) = self.label(b);
            let key = keys.first_key(Context::Update, action).map(|k| format!(" ({k})")).unwrap_or_default();
            let text = format!("[ {}{key} ]", tr(words));
            let w = (unicode_width::UnicodeWidthStr::width(text.as_str()) as u16).min(inner.right().saturating_sub(x));
            if w == 0 {
                break;
            }
            let rect = Rect { x, y, width: w, height: 1 };
            let style = if self.hover == Some(b) { super::theme::hover_style().add_modifier(Modifier::BOLD) } else { Style::default().add_modifier(Modifier::BOLD) };
            f.render_widget(Paragraph::new(Span::styled(text, style)), rect);
            self.buttons_at.push((b, rect));
            x = x.saturating_add(w + 2);
        }
    }
}

/// `path` as people write it (without Windows' `\\?\`).
fn shown(path: &Path) -> String {
    crate::uninstall::plain(path).display().to_string()
}

/// Whether `dir` belongs to whoever runs ttyak (Unix; elsewhere: taken as not).
fn yours(dir: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: getuid has no preconditions and cannot fail.
        std::fs::metadata(dir).is_ok_and(|m| m.uid() == unsafe { libc::getuid() })
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        false
    }
}

/// The install script's command that puts the new version in `dir`; through sudo when `as_root`
/// (Unix: a folder someone else owns).
fn script_command(dir: &Path, as_root: bool) -> String {
    let dir = shown(dir);
    if cfg!(windows) {
        format!("& ([scriptblock]::Create((irm https://ttyak.sh/install.ps1))) -Dir '{}'", dir.replace('\'', "''"))
    } else {
        let safe = !dir.is_empty() && dir.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+~".contains(c));
        let quoted = if safe { dir } else { format!("'{}'", dir.replace('\'', r"'\''")) };
        let sudo = if as_root { "sudo " } else { "" };
        format!("curl -fsSL https://ttyak.sh/install.sh | {sudo}sh -s -- --dir {quoted}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn latest() -> Latest {
        Latest {
            version: semver::Version::new(9, 9, 9),
            date: "2026-11-01".into(),
            notes: "https://github.com/dmytrobsp/ttyak/releases/tag/v9.9.9".into(),
            asset: crate::update::Asset { url: "https://github.com/dmytrobsp/ttyak/releases/download/v9.9.9/ttyak-9.9.9-x86_64-linux.zip".into(), sha256: "ab".repeat(32) },
        }
    }

    fn install(method: Method, writable: bool) -> Install {
        Install { method, exe: PathBuf::from("/home/u/.local/bin/ttyak"), writable }
    }

    fn view(method: Method, writable: bool) -> UpdateView {
        UpdateView::new(latest(), install(method, writable))
    }

    /// The screen as drawn, a row a line.
    fn screen(v: &mut UpdateView) -> Vec<String> {
        let keys = Keymap::load("").0;
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| v.draw(f, f.area(), &keys)).unwrap();
        let buf = terminal.backend().buffer();
        (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>().trim_end().to_string()).collect()
    }

    #[test]
    fn update_now_only_when_self_updatable() {
        let mut ours = view(Method::Script, true);
        assert_eq!(ours.offer, Offer::UpdateNow);
        assert_eq!(ours.buttons(), [Button::UpdateNow, Button::Notes, Button::Close]);
        let text = screen(&mut ours).join("\n");
        assert!(text.contains("[ Update now (u) ]") && text.contains("/home/u/.local/bin/ttyak") && text.contains("ttyak 9.9.9 is out (2026-11-01)"), "{text}");
        assert_eq!(ours.key(Action::Open), Do::Start, "Enter is the first button");
        for (method, writable) in [(Method::Script, false), (Method::Homebrew, false), (Method::Scoop, false), (Method::Winget, false), (Method::Chocolatey, false), (Method::Aur, false), (Method::Deb, false), (Method::Rpm, false), (Method::Unknown, false)] {
            let mut v = view(method, writable);
            assert_ne!(v.offer, Offer::UpdateNow, "{method:?}");
            assert!(!v.buttons().contains(&Button::UpdateNow), "{method:?}");
            assert_eq!(v.key(Action::UpdateNow), Do::Nothing, "{method:?}: the key does nothing either");
            assert_ne!(v.key(Action::Open), Do::Start, "{method:?}");
            assert!(!screen(&mut v).join("\n").contains("Update now"), "{method:?}");
        }
    }

    #[test]
    fn unwritable_folder_offers_the_command() {
        let mut v = view(Method::Script, false);
        let Offer::Command(command) = v.offer.clone() else { panic!("{:?}", v.offer) };
        if cfg!(windows) {
            assert_eq!(command, "& ([scriptblock]::Create((irm https://ttyak.sh/install.ps1))) -Dir '/home/u/.local/bin'");
        } else {
            assert_eq!(command, "curl -fsSL https://ttyak.sh/install.sh | sudo sh -s -- --dir /home/u/.local/bin");
        }
        assert_eq!(v.buttons(), [Button::Copy, Button::Notes, Button::Close]);
        assert_eq!(v.key(Action::Copy), Do::Copy(command.clone()));
        let text = screen(&mut v).join("\n");
        // (Wrapped on the screen; the button copies it whole.)
        assert!(text.contains("ttyak cannot write /home/u/.local/bin") && text.contains(&command[..30]) && text.contains("[ Copy the command (y) ]"), "{text}");
        assert_eq!(script_command(Path::new("/opt/it's mine"), true), if cfg!(windows) { "& ([scriptblock]::Create((irm https://ttyak.sh/install.ps1))) -Dir '/opt/it''s mine'".to_string() } else { r"curl -fsSL https://ttyak.sh/install.sh | sudo sh -s -- --dir '/opt/it'\''s mine'".to_string() });
    }

    #[cfg(unix)]
    #[test]
    fn sudo_only_for_someone_else_s_folder() {
        let mine = std::env::temp_dir().join(format!("ttyak-test-yours-{}", std::process::id()));
        std::fs::create_dir_all(&mine).unwrap();
        assert!(yours(&mine) && !yours(&mine.join("not there")));
        // SAFETY: getuid has no preconditions and cannot fail.
        if unsafe { libc::getuid() } != 0 {
            assert!(!yours(Path::new("/")), "root's folder");
        }
        let mut v = UpdateView::new(latest(), Install { method: Method::Script, exe: mine.join("ttyak"), writable: false });
        assert_eq!(v.offer, Offer::Command(format!("curl -fsSL https://ttyak.sh/install.sh | sh -s -- --dir {}", mine.display())), "your own folder: no sudo");
        let text = screen(&mut v).join("\n");
        assert!(text.contains("Once you can write there (it is your folder), run:") && !text.contains("as someone"), "{text}");
        assert_eq!(script_command(Path::new("/usr/local/bin"), true), "curl -fsSL https://ttyak.sh/install.sh | sudo sh -s -- --dir /usr/local/bin");
        std::fs::remove_dir(&mine).unwrap();
    }

    #[test]
    fn package_install_shows_its_command() {
        for (method, command, how) in [
            (Method::Homebrew, "brew upgrade ttyak", "with Homebrew"),
            (Method::Scoop, "scoop update ttyak", "with Scoop"),
            (Method::Winget, "winget upgrade dmytrobsp.ttyak", "with winget"),
            (Method::Chocolatey, "choco upgrade ttyak", "with Chocolatey"),
            (Method::Aur, "yay -Syu ttyak-bin", "from the AUR"),
        ] {
            let mut v = view(method, false);
            assert_eq!(v.offer, Offer::Command(command.into()), "{method:?}");
            assert_eq!(v.key(Action::Open), Do::Copy(command.into()), "{method:?}: Enter copies it");
            let text = screen(&mut v).join("\n");
            assert!(text.contains(&format!("ttyak was installed {how}. To update it, run:")) && text.contains(&format!("  {command}")), "{method:?}: {text}");
        }
    }

    #[test]
    fn deb_rpm_show_the_download_link() {
        let notes = latest().notes;
        for (method, package, says) in [(Method::Deb, Some(".deb"), "ttyak was installed from a .deb package: download the new .deb from"), (Method::Rpm, Some(".rpm"), "ttyak was installed from an .rpm package: download the new .rpm from"), (Method::Unknown, None, "ttyak cannot tell how it was installed: download it from")] {
            let mut v = view(method, false);
            assert_eq!(v.offer, Offer::Download { package, url: notes.clone() }, "{method:?}");
            assert_eq!(v.key(Action::Open), Do::Open(notes.clone()), "{method:?}: Enter opens the page");
            assert_eq!(v.key(Action::Copy), Do::Copy(notes.clone()), "{method:?}");
            let text = screen(&mut v).join("\n");
            assert!(text.contains(says) && text.contains(&notes) && text.contains("[ Open the download page (o) ]"), "{method:?}: {text}");
        }
    }

    #[test]
    fn says_restart_never_restarts() {
        let mut v = view(Method::Script, true);
        assert_eq!(v.key(Action::Open), Do::Start);
        let progress = Arc::new(Progress::default());
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let task = rt.block_on(async { tokio::spawn(std::future::pending::<()>()).abort_handle() });
        v.started(progress, task);
        assert_eq!(v.buttons(), [Button::Cancel]);
        assert!(screen(&mut v).join("\n").contains("Downloading…"));
        v.finished(Ok(()));
        let text = screen(&mut v).join("\n");
        assert!(text.contains("ttyak 9.9.9 is installed — restart ttyak to use it") && text.contains("keeps running until you quit it"), "{text}");
        assert_eq!(v.buttons(), [Button::Close], "nothing that restarts");
        for action in [Action::Open, Action::UpdateNow, Action::Copy] {
            assert!(matches!(v.key(action), Do::Close | Do::Nothing), "{action:?}");
        }
        assert!(!text.contains("Update now") && !text.to_lowercase().contains("restart now"), "{text}");
    }

    #[test]
    fn a_cancel_while_downloading_keeps_the_screen() {
        let mut v = view(Method::Script, true);
        let progress = Arc::new(Progress::default());
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let task = rt.block_on(async { tokio::spawn(std::future::pending::<()>()) });
        v.started(progress.clone(), task.abort_handle());
        assert_eq!(v.key(Action::Close), Do::Nothing, "Esc cancels the download, the screen stays");
        assert!(matches!(v.stage, Stage::Cancelled) && !progress.installing());
        assert!(rt.block_on(task).unwrap_err().is_cancelled(), "the download task is stopped");
        assert!(screen(&mut v).join("\n").contains("cancelled; nothing was changed"));
        v.finished(Err("cancelled".into()));
        assert!(matches!(v.stage, Stage::Cancelled), "a late end does not undo the cancel");
        assert_eq!(v.buttons(), [Button::UpdateNow, Button::Notes, Button::Close]);
        assert_eq!(v.key(Action::Close), Do::Close);
    }

    #[test]
    fn a_failure_says_what_and_offers_again() {
        let mut v = view(Method::Script, true);
        v.finished(Err("the download does not match the SHA256 in latest.json".into()));
        let text = screen(&mut v).join("\n");
        assert!(text.contains("Could not update: the download does not match the SHA256 in latest.json") && text.contains("[ Try again (u) ]"), "{text}");
    }

    #[test]
    fn buttons_take_clicks_and_hover() {
        let mut v = view(Method::Homebrew, false);
        screen(&mut v);
        let (b, r) = v.buttons_at[1];
        assert_eq!(b, Button::Notes);
        v.hover_at(r.x + 1, r.y);
        assert_eq!((v.button_at(r.x, r.y), v.hover), (Some(Button::Notes), Some(Button::Notes)));
        assert_eq!(v.press(Button::Notes), Do::Open(latest().notes));
        v.hover_at(r.x, r.y + 1);
        assert_eq!(v.hover, None);
    }
}

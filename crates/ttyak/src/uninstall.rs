//! `ttyak uninstall`: removes a copy the install scripts put here (the program, its `install.json`, the line or user
//! PATH entry that put its folder on PATH, and unless asked to keep it everything `ttyak purge` deletes); for a copy
//! a package manager installed it shows that manager's command instead.

use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use slack_core::profiles::Dirs;

use crate::install::{self, Install, Method, Record};

/// The end of every line the install script writes into a shell's startup file (and of every line it prints for one).
pub const MARKER: &str = "# added by the ttyak installer; \"ttyak uninstall\" removes this line";

/// A change that takes the install folder off PATH again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathEdit {
    /// The installer's marked lines in this shell startup file.
    ProfileLine(PathBuf),
    /// The folder's entry in the Windows user PATH (`HKCU\Environment`).
    WindowsUserPath(PathBuf),
}

/// What uninstalling removes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Files, then a folder that goes when nothing else is left in it.
    pub remove: Vec<PathBuf>,
    pub path_edits: Vec<PathEdit>,
    /// Startup files with the installer's lines that ttyak leaves alone (a link, in a linked folder, or not
    /// writable), with those lines: the user removes them.
    pub by_hand: Vec<(PathBuf, Vec<String>)>,
    /// Whether everything ttyak wrote goes too (as `ttyak purge`).
    pub purge: bool,
    /// Said after the steps.
    pub notes: Vec<String>,
}

/// `ttyak uninstall`.
pub fn command(dirs: &Dirs, yes: bool, keep_data: bool) -> Result<()> {
    use std::io::IsTerminal;
    let install = install::current();
    let home = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf()).ok_or_else(|| anyhow!("no home folder"))?;
    let plan = plan(&install, keep_data, &home);
    let data = if plan.purge { crate::purge_targets(dirs) } else { Vec::new() };
    let mut confirm = |question: &str| crate::auth::confirm(question, false);
    let terminal = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    uninstall(&install, plan, dirs, &data, yes, if terminal { Some(&mut confirm) } else { None })
}

/// What uninstalling `install` removes; nothing for a copy the install scripts did not put here.
pub fn plan(install: &Install, keep_data: bool, home: &Path) -> Plan {
    let folder = |name: &str| std::env::var_os(name).map(PathBuf::from).filter(|p| p.is_absolute());
    let (zdotdir, config) = (folder("ZDOTDIR"), folder("XDG_CONFIG_HOME"));
    plan_in(install, keep_data, home, zdotdir.as_deref(), config.as_deref(), cfg!(windows))
}

/// `plan` with the folders `ZDOTDIR` and `XDG_CONFIG_HOME` name, for Windows or not.
fn plan_in(install: &Install, keep_data: bool, home: &Path, zdotdir: Option<&Path>, config: Option<&Path>, windows: bool) -> Plan {
    if install.method != Method::Script {
        return Plan::default();
    }
    let exe = plain(&install.exe);
    let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut remove = vec![exe.clone(), dir.join(install::RECORD_FILE)];
    let mut path_edits = Vec::new();
    let mut by_hand = Vec::new();
    let mut notes = Vec::new();
    if windows {
        // What install.ps1 can leave: old copies that were running during an update, files of a run cut short.
        let mut names: Vec<String> = std::fs::read_dir(&dir).into_iter().flatten().flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| is_old_copy(n)).collect();
        names.sort();
        remove.extend(names.iter().map(|n| dir.join(n)));
        remove.extend([".ttyak.exe.new", ".install.json.new"].map(|n| dir.join(n)));
        remove.extend(update_leftovers(&dir));
        remove.retain(|p| p.symlink_metadata().is_ok());
        // The script's own folder (the default %LOCALAPPDATA%\Programs\ttyak); never a shared one.
        if dir.file_name().is_some_and(|n| n.eq_ignore_ascii_case("ttyak")) {
            remove.push(dir.clone());
        }
        // install.json's spelling of the folder, as install.ps1 put it on PATH; only when it is this folder.
        let key = |p: &Path| p.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase();
        let on_path = Record::read(&dir, &install::System).map(|r| plain(&r.dir)).filter(|d| key(d) == key(&dir)).unwrap_or(dir);
        path_edits.push(PathEdit::WindowsUserPath(on_path));
    } else {
        // The folder stays: ~/.local/bin is shared.
        remove.extend([".ttyak.new", ".install.json.new"].map(|n| dir.join(n)));
        remove.extend(update_leftovers(&dir));
        remove.retain(|p| p.symlink_metadata().is_ok());
        for file in startup_files(home, zdotdir, config) {
            let Ok(text) = std::fs::read(&file) else { continue };
            let lines = marked_lines(&text);
            if lines.is_empty() {
                continue;
            }
            let linked = file.is_symlink() || linked_on_the_way(&file, home);
            if linked || std::fs::OpenOptions::new().append(true).open(&file).is_err() {
                by_hand.push((file, lines));
            } else {
                path_edits.push(PathEdit::ProfileLine(file));
            }
        }
        // zsh's own folder, when only its startup files set it, is not known here.
        let zshenv = home.join(".zshenv");
        if zdotdir.is_none() && std::fs::read(&zshenv).is_ok_and(|t| t.windows(7).any(|w| w == b"ZDOTDIR")) {
            notes.push(format!(
                "{} sets ZDOTDIR, which is not set here, so for zsh only {} was looked at: a ttyak PATH line may remain in the .zshrc in your zsh folder (the line ending with {MARKER})",
                zshenv.display(),
                home.join(".zshrc").display()
            ));
        }
    }
    Plan { remove, path_edits, by_hand, purge: !keep_data, notes }
}

/// What a self-update cut short (the power gone, the process killed) can leave in `dir`: the new
/// executable and the download, `.ttyak-update-<pid>.zip`.
fn update_leftovers(dir: &Path) -> Vec<PathBuf> {
    let download = |n: &str| n.strip_prefix(".ttyak-update-").and_then(|r| r.strip_suffix(".zip")).is_some_and(|pid| !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()));
    let mut names: Vec<String> = std::fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| download(n)).collect();
    names.sort();
    std::iter::once(dir.join(".ttyak-new")).chain(names.iter().map(|n| dir.join(n))).collect()
}

/// Whether `name` is an old copy install.ps1 renamed aside during an update: `ttyak.old.exe`,
/// `ttyak.old-<8 hex>.exe`.
pub(crate) fn is_old_copy(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("ttyak.old").and_then(|r| r.strip_suffix(".exe")) else { return false };
    rest.is_empty() || rest.strip_prefix('-').is_some_and(|h| h.len() == 8 && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The shell startup files the install script may have put its line in, or where people paste the lines it printed;
/// each once: `home`'s, `zdotdir`'s `.zshrc`, and the fish, nushell and elvish files under `config` and `~/.config`.
fn startup_files(home: &Path, zdotdir: Option<&Path>, config: Option<&Path>) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = [".profile", ".bashrc", ".bash_profile", ".bash_login", ".zshrc", ".cshrc", ".tcshrc", ".xonshrc"].iter().map(|n| home.join(n)).collect();
    files.extend(zdotdir.map(|z| z.join(".zshrc")));
    for folder in config.into_iter().map(Path::to_path_buf).chain([home.join(".config")]) {
        files.extend(["fish/conf.d/ttyak.fish", "nushell/env.nu", "elvish/rc.elv"].map(|n| folder.join(n)));
    }
    // nushell's on macOS.
    files.push(home.join("Library/Application Support/nushell/env.nu"));
    let mut seen = std::collections::HashSet::new();
    files.retain(|f| seen.insert(f.clone()));
    files
}

/// Whether a folder between `home` (or `/`, for a file outside it) and `file` is a link: dotfiles kept elsewhere,
/// which the install script leaves alone too.
fn linked_on_the_way(file: &Path, home: &Path) -> bool {
    file.ancestors().skip(1).take_while(|p| *p != home && p.parent().is_some()).any(Path::is_symlink)
}

/// The fish file the install script makes: ours, deleted when nothing but its lines is in it.
const FISH_FILE: &str = "ttyak.fish";

/// Carries out `plan`'s removals and PATH edits (not the purge), every one even when another fails. Returns what
/// happened to each, said for the user: on an error the words that go before it.
pub fn run(plan: &Plan) -> Vec<(String, io::Result<()>)> {
    let mut done = Vec::new();
    // Files the running ttyak holds (Windows): deleted once it has quit, with the folders they are in.
    let mut later: Vec<PathBuf> = Vec::new();
    let mut later_folders: Vec<PathBuf> = Vec::new();
    // While the program stays, its folder stays on PATH: ttyak can still be run (and uninstalled again).
    let mut program_left = false;
    for path in &plan.remove {
        let program = path.file_name().is_some_and(|n| n == "ttyak" || n == "ttyak.exe");
        let shown = path.display();
        if path.symlink_metadata().is_ok_and(|m| m.is_dir()) {
            if later.iter().any(|f| f.starts_with(path)) {
                later_folders.push(path.clone());
            } else if std::fs::read_dir(path).is_ok_and(|mut d| d.next().is_some()) {
                done.push((format!("kept {shown}: other files are in it"), Ok(())));
            } else {
                done.push(outcome(format!("deleted {shown}"), format!("could not delete {shown}"), std::fs::remove_dir(path)));
            }
            continue;
        }
        match std::fs::remove_file(path) {
            Ok(()) => done.push((format!("deleted {shown}"), Ok(()))),
            Err(e) if cfg!(windows) && path.file_name().is_some_and(|n| n == "ttyak.exe") => {
                // The running program: Windows lets it be renamed, not deleted.
                let aside = path.with_file_name(format!("ttyak.old-{:08x}.exe", std::process::id() ^ std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0)));
                match std::fs::rename(path, &aside) {
                    Ok(()) => later.push(aside),
                    Err(_) => {
                        program_left = true;
                        done.push((format!("could not delete {shown}"), Err(e)));
                    }
                }
            }
            Err(e) => {
                program_left |= program;
                done.push((format!("could not delete {shown}"), Err(e)));
            }
        }
    }
    if !later.is_empty() {
        let shown = later.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ");
        let result = cleanup_line(&later, &later_folders).ok_or_else(|| io::Error::other("its name has a \" or %: delete it yourself")).and_then(|line| spawn_cleanup(&line));
        done.push(outcome(
            format!("{shown} goes once ttyak has quit (within 2 minutes; if a ttyak is still running then, delete {shown} yourself)"),
            format!("could not delete {shown}"),
            result,
        ));
    }
    for edit in &plan.path_edits {
        done.push(match edit {
            PathEdit::ProfileLine(file) if program_left => (format!("left the ttyak PATH line in {}, as ttyak itself is still there", file.display()), Ok(())),
            PathEdit::WindowsUserPath(dir) if program_left => (format!("left {} on your user PATH, as ttyak itself is still there", dir.display()), Ok(())),
            PathEdit::ProfileLine(file) => edit_profile(file),
            PathEdit::WindowsUserPath(dir) => user_path::remove(dir),
        });
    }
    done
}

/// `result` with the words for success or for an error.
fn outcome(ok: String, failed: String, result: io::Result<()>) -> (String, io::Result<()>) {
    match result {
        Ok(()) => (ok, Ok(())),
        Err(e) => (failed, Err(e)),
    }
}

/// Writes `new` over `file` with `write`, in place (its mode, owner and links stay); when that fails, writes `old`
/// back.
fn rewrite(file: &Path, new: &[u8], old: &[u8], write: &dyn Fn(&Path, &[u8]) -> io::Result<()>) -> io::Result<()> {
    let Err(e) = write(file, new) else { return Ok(()) };
    match write(file, old) {
        Ok(()) => Err(io::Error::new(e.kind(), format!("{e} (the file is as it was)"))),
        Err(again) => Err(io::Error::new(e.kind(), format!("{e}, and its old text could not be written back ({again}): restore {} from a backup", file.display()))),
    }
}

/// Takes the installer's lines out of the startup file `file`, written in place (its mode, owner and links stay);
/// the fish file the installer made goes when nothing else is in it.
fn edit_profile(file: &Path) -> (String, io::Result<()>) {
    let shown = file.display();
    let edit = || -> io::Result<bool> {
        let old = std::fs::read(file)?;
        let Some(kept) = without_marked_lines(&old) else { return Ok(false) };
        if kept.is_empty() && file.file_name().is_some_and(|n| n == FISH_FILE) {
            std::fs::remove_file(file)?;
            return Ok(true);
        }
        rewrite(file, &kept, &old, &|p, bytes| std::fs::write(p, bytes))?;
        Ok(false)
    };
    match edit() {
        Ok(true) => (format!("deleted {shown} (it only put ttyak on PATH)"), Ok(())),
        Ok(false) => (format!("removed the ttyak PATH line from {shown}"), Ok(())),
        Err(e) => (format!("could not remove the ttyak PATH line from {shown}"), Err(e)),
    }
}

/// Asks a y/N question; None without a terminal.
type Ask<'a> = Option<&'a mut dyn FnMut(&str) -> Result<bool>>;

/// The steps of `command`: `data` is what `ttyak purge` would delete.
fn uninstall(install: &Install, mut plan: Plan, dirs: &Dirs, data: &[PathBuf], yes: bool, ask: Ask) -> Result<()> {
    if let Some(note) = package_note(install) {
        println!("{note}");
        return Ok(());
    }
    let exe = plain(&install.exe);
    let dir = exe.parent().unwrap_or(&exe);
    if !install.writable {
        bail!("ttyak cannot write in {}, so it cannot remove itself: delete {} and the install.json beside it yourself", dir.display(), exe.display());
    }
    let ask = match (yes, ask) {
        (true, _) => None,
        (false, Some(ask)) => Some(ask),
        (false, None) => bail!("no terminal to ask whether to keep your data: pass --yes to remove ttyak and its data, or --yes --keep-data to keep the data"),
    };
    let version = Record::read(dir, &install::System).map(|r| format!(" {}", r.version)).unwrap_or_default();
    eprintln!("This removes ttyak{version} (put here by the install script):");
    for line in describe(&plan) {
        eprintln!("  {line}");
    }
    for (file, lines) in &plan.by_hand {
        eprintln!("{} is a link, lies in a linked folder or cannot be written, so it is left alone: remove these lines from it (or from where it comes from) yourself:", file.display());
        for line in lines {
            eprintln!("  {line}");
        }
    }
    let data_note = || {
        eprintln!("and everything ttyak wrote (files you saved from messages stay):");
        crate::list_purge_targets(data);
    };
    if let Some(ask) = ask {
        if plan.purge && !data.is_empty() {
            plan.purge = !ask("Keep your settings, sign-ins and drafts?")?;
        }
        if plan.purge && !data.is_empty() {
            data_note();
            crate::refuse_while_running(dirs)?;
        }
        let question = match (plan.purge, data.is_empty()) {
            (false, _) => "Remove ttyak? Your settings, sign-ins and drafts stay.",
            (true, true) => "Remove ttyak?",
            (true, false) => "Remove ttyak and all of this?",
        };
        if !ask(question)? {
            println!("nothing removed");
            return Ok(());
        }
    } else if plan.purge && !data.is_empty() {
        data_note();
        crate::refuse_while_running(dirs)?;
    }
    // The data first: when some of it cannot go, ttyak is still here to run again.
    if plan.purge {
        let failed = crate::delete_purge_targets(data);
        if failed > 0 {
            bail!("{failed} of {} could not be deleted (see above), so ttyak itself was left: quit what uses them and run ttyak uninstall again", data.len());
        }
    }
    let done = run(&plan);
    let mut failed = 0;
    for (what, result) in &done {
        match result {
            Ok(()) => println!("{what}"),
            Err(e) => {
                failed += 1;
                eprintln!("{what}: {e}");
            }
        }
    }
    if failed > 0 {
        bail!("{failed} of {} steps did not work (see above)", done.len());
    }
    if !plan.by_hand.is_empty() {
        println!("ttyak is uninstalled, but for the lines named above");
    } else {
        println!("ttyak is uninstalled");
    }
    for note in &plan.notes {
        println!("{note}");
    }
    if !plan.purge {
        let kept: Vec<String> = [&dirs.config, &dirs.data, &dirs.cache].into_iter().filter(|d| d.exists()).map(|d| d.display().to_string()).collect();
        if !kept.is_empty() {
            println!("Your settings, sign-ins and drafts stay in {}", kept.join(", "));
        }
    }
    Ok(())
}

/// `plan`'s steps, one a line, for the list shown before anything goes.
fn describe(plan: &Plan) -> Vec<String> {
    let mut lines: Vec<String> = plan
        .remove
        .iter()
        .map(|p| if p.symlink_metadata().is_ok_and(|m| m.is_dir()) { format!("{} (if nothing else is in it)", p.display()) } else { p.display().to_string() })
        .collect();
    lines.extend(plan.path_edits.iter().map(|edit| match edit {
        PathEdit::ProfileLine(file) => format!("the line in {} that puts ttyak on PATH", file.display()),
        PathEdit::WindowsUserPath(dir) => format!("{} from your user PATH", dir.display()),
    }));
    lines
}

/// What to do instead for a copy the install scripts did not put here: the package manager's command and the purge
/// step, or where the file is.
fn package_note(install: &Install) -> Option<String> {
    if install.method == Method::Script {
        return None;
    }
    let how = install.method.how();
    let purge = "To delete your settings, sign-ins and drafts too, first run: ttyak purge";
    let Some(command) = install.method.uninstall_command() else {
        return Some(format!("This ttyak was not put here by the install script or a package manager: delete {} yourself.\n{purge}", plain(&install.exe).display()));
    };
    // Their uninstall scripts purge (Scoop's manifest, Chocolatey's chocolateyUninstall.ps1).
    let data = if matches!(install.method, Method::Scoop | Method::Chocolatey) { "It deletes your settings, sign-ins and drafts too, as ttyak purge does." } else { purge };
    Some(format!("ttyak was installed {how}. To remove it: {command}\n{data}"))
}

/// `path` without the `\\?\` that Windows puts before a resolved path, as people write paths.
pub(crate) fn plain(path: &Path) -> PathBuf {
    let Some(text) = path.to_str() else { return path.to_path_buf() };
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path.to_path_buf()
    }
}

/// `text`'s lines (without their line ending), split at `\n`.
fn lines(text: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    text.split_inclusive(|b| *b == b'\n').map(|line| {
        let bare = line.strip_suffix(b"\n").unwrap_or(line);
        (line, bare.strip_suffix(b"\r").unwrap_or(bare))
    })
}

/// Whether `line` ends with [`MARKER`] (spaces and tabs after it allowed).
fn is_marked(line: &[u8]) -> bool {
    let end = line.iter().rposition(|b| !matches!(b, b' ' | b'\t')).map_or(0, |i| i + 1);
    line[..end].ends_with(MARKER.as_bytes())
}

/// The lines of `text` that end with [`MARKER`].
fn marked_lines(text: &[u8]) -> Vec<String> {
    lines(text).filter(|(_, bare)| is_marked(bare)).map(|(_, bare)| String::from_utf8_lossy(bare).into_owned()).collect()
}

/// `text` without its lines that end with [`MARKER`], every other byte kept; None when no line does.
fn without_marked_lines(text: &[u8]) -> Option<Vec<u8>> {
    let mut kept = Vec::with_capacity(text.len());
    let mut found = false;
    for (line, bare) in lines(text) {
        if is_marked(bare) {
            found = true;
        } else {
            kept.extend_from_slice(line);
        }
    }
    found.then_some(kept)
}

/// The user PATH `value` without its entries naming `dir` (in any case, with or without a trailing `\`); None when
/// none does.
#[cfg_attr(not(windows), allow(dead_code))] // only Windows has a user PATH to edit; tested everywhere
fn without_entry(value: &str, dir: &str) -> Option<String> {
    let key = |entry: &str| entry.trim_end_matches('\\').to_lowercase();
    let want = key(dir);
    if want.is_empty() {
        return None;
    }
    let entries: Vec<&str> = value.split(';').collect();
    let kept: Vec<&str> = entries.iter().copied().filter(|e| key(e) != want).collect();
    (kept.len() < entries.len()).then(|| kept.join(";"))
}

/// The `cmd /c` line that deletes `files` once they can go (once this ttyak has quit: tried every second for two
/// minutes), then removes `folders` if empty; None when a path has a `"` or `%` (cmd would read them).
fn cleanup_line(files: &[PathBuf], folders: &[PathBuf]) -> Option<String> {
    let quoted = |p: &PathBuf| p.to_str().filter(|t| !t.contains(['"', '%'])).map(|t| format!("\"{t}\""));
    let files: Vec<String> = files.iter().map(quoted).collect::<Option<_>>()?;
    let folders: Vec<String> = folders.iter().map(quoted).collect::<Option<_>>()?;
    if files.is_empty() {
        return None;
    }
    let mut body: Vec<String> = files.iter().map(|f| format!("del /f /q {f} 2>nul")).collect();
    let waits: Vec<String> = files.iter().map(|f| format!("if exist {f} (ping -n 2 127.0.0.1 >nul)")).collect();
    let mut last: Vec<String> = folders.iter().map(|d| format!("rmdir {d} 2>nul")).collect();
    last.push("exit".into());
    body.push(format!("{} else ({})", waits.join(" else "), last.join(" & ")));
    Some(format!("for /l %i in (1,1,120) do @({})", body.join(" & ")))
}

/// Starts `line` in a hidden `cmd` that outlives this process (and the console or ssh session it runs in, where
/// Windows allows).
#[cfg(windows)]
fn spawn_cleanup(line: &str) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let cmd = std::env::var_os("SystemRoot").map(|r| PathBuf::from(r).join(r"System32\cmd.exe")).unwrap_or_else(|| PathBuf::from("cmd.exe"));
    let start = |flags: u32| {
        Command::new(&cmd)
            // No AutoRun commands, no delayed expansion (a `!` in a path stays a `!`).
            .raw_arg(format!("/d /v:off /s /c \"{line}\""))
            .current_dir(std::env::temp_dir())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(flags)
            .spawn()
    };
    // A job that does not allow leaving it refuses the breakaway: then inside it.
    start(CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB).or_else(|_| start(CREATE_NO_WINDOW)).map(drop)
}

#[cfg(not(windows))]
fn spawn_cleanup(_line: &str) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "only needed on Windows"))
}

/// The user PATH in the registry (`HKCU\Environment`, value `Path`).
mod user_path {
    use std::io;
    use std::path::Path;

    /// Takes `dir` out of the user PATH, the value's kind kept (REG_SZ or REG_EXPAND_SZ, as install.ps1 does), and
    /// tells running programs.
    #[cfg(windows)]
    pub fn remove(dir: &Path) -> (String, io::Result<()>) {
        let shown = dir.display();
        match edit(dir) {
            Ok(true) => (format!("removed {shown} from your user PATH (open a new terminal for it to count)"), Ok(())),
            Ok(false) => (format!("{shown} was not on your user PATH"), Ok(())),
            Err(e) => (format!("could not remove {shown} from your user PATH"), Err(e)),
        }
    }

    #[cfg(not(windows))]
    pub fn remove(dir: &Path) -> (String, io::Result<()>) {
        (format!("could not remove {} from the user PATH", dir.display()), Err(io::Error::new(io::ErrorKind::Unsupported, "only Windows has one")))
    }

    #[cfg(windows)]
    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain([0]).collect()
    }

    /// Whether the PATH listed `dir` (and now does not).
    #[cfg(windows)]
    fn edit(dir: &Path) -> io::Result<bool> {
        use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
        use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_EXPAND_SZ, REG_SZ, RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW};
        let check = |code: u32| if code == ERROR_SUCCESS { Ok(()) } else { Err(io::Error::from_raw_os_error(code as i32)) };
        let name = wide("Path");
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: the strings are NUL-terminated and outlive the call; `key` is written by it.
        check(unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, wide("Environment").as_ptr(), 0, KEY_QUERY_VALUE | KEY_SET_VALUE, &mut key) })?;
        /// Closes the key on every way out.
        struct Key(HKEY);
        impl Drop for Key {
            fn drop(&mut self) {
                // SAFETY: the key was opened above and is closed once.
                unsafe { RegCloseKey(self.0) };
            }
        }
        let key = Key(key);
        // Read raw: RegQueryValueExW expands nothing.
        let (mut kind, mut size) = (0u32, 0u32);
        // SAFETY: a null buffer asks only for the kind and the size.
        let code = unsafe { RegQueryValueExW(key.0, name.as_ptr(), std::ptr::null(), &mut kind, std::ptr::null_mut(), &mut size) };
        if code == ERROR_FILE_NOT_FOUND {
            return Ok(false);
        }
        check(code)?;
        if kind != REG_SZ && kind != REG_EXPAND_SZ {
            return Err(io::Error::other("the user PATH is not text; left as it is"));
        }
        let mut buffer = vec![0u16; (size as usize).div_ceil(2) + 1];
        let mut bytes = (buffer.len() * 2) as u32;
        // SAFETY: `buffer` holds `bytes` bytes.
        check(unsafe { RegQueryValueExW(key.0, name.as_ptr(), std::ptr::null(), &mut kind, buffer.as_mut_ptr().cast(), &mut bytes) })?;
        buffer.truncate((bytes as usize) / 2);
        while buffer.last() == Some(&0) {
            buffer.pop();
        }
        let value = String::from_utf16(&buffer).map_err(|_| io::Error::other("the user PATH is not valid text; left as it is"))?;
        let Some(kept) = super::without_entry(&value, &dir.to_string_lossy()) else { return Ok(false) };
        if kept.is_empty() {
            // install.ps1 made the value with this one entry.
            // SAFETY: the name is NUL-terminated.
            check(unsafe { RegDeleteValueW(key.0, name.as_ptr()) })?;
        } else {
            let data = wide(&kept);
            // SAFETY: `data` is NUL-terminated and its length is given in bytes.
            check(unsafe { RegSetValueExW(key.0, name.as_ptr(), 0, kind, data.as_ptr().cast(), (data.len() * 2) as u32) })?;
        }
        broadcast();
        Ok(true)
    }

    /// Tells running programs (Explorer above all) that the environment changed, as setting it in Windows does; one
    /// that does not answer in 5 seconds is not waited for.
    #[cfg(windows)]
    fn broadcast() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE};
        let what = wide("Environment");
        let mut result = 0usize;
        // SAFETY: `what` is NUL-terminated and outlives the call.
        unsafe { SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, 0, what.as_ptr() as isize, SMTO_ABORTIFHUNG, 5000, &mut result) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temporary folder with a home, an install folder `bin`, ttyak's three folders, removed when dropped.
    struct Temp(PathBuf);

    impl Temp {
        fn new(test: &str) -> Temp {
            let base = std::env::temp_dir().join(format!("ttyak-uninstall-{test}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(base.join("home")).unwrap();
            // Without links on the way (macOS's /var is one), which the link checks would see; as people write
            // it (no `\\?\` on Windows), as the plans do.
            Temp(plain(&base.canonicalize().unwrap()))
        }

        fn home(&self) -> PathBuf {
            self.0.join("home")
        }

        fn dirs(&self) -> Dirs {
            Dirs { config: self.0.join("config"), cache: self.0.join("cache"), data: self.0.join("data") }
        }

        /// ttyak's folders with a file in each; returned as `purge` would list them.
        fn data(&self) -> Vec<PathBuf> {
            let d = self.dirs();
            write(&d.config.join("default/session.json"), "{}");
            write(&d.cache.join("default/ttyak.log"), "log");
            write(&d.data.join("drafts.json"), "{}");
            vec![d.config, d.cache, d.data]
        }

        /// A script install in `<base>/<folder>`: the program and its install.json.
        fn script(&self, folder: &str, exe: &str) -> Install {
            let dir = self.0.join(folder);
            write(&dir.join(exe), "program");
            let record = serde_json::json!({"method": "script", "version": "0.11.0", "dir": dir});
            write(&dir.join("install.json"), &format!("{record}\n"));
            Install { method: Method::Script, exe: dir.join(exe), writable: true }
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    fn marked(line: &str) -> String {
        format!("{line} {MARKER}")
    }

    /// No questions: what `uninstall` does with `--yes`.
    fn no_terminal() -> Ask<'static> {
        None
    }

    #[cfg(unix)]
    #[test]
    fn script_install_removes_binary_record_and_path_line() {
        let t = Temp::new("script");
        let install = t.script("bin", "ttyak");
        let bin = t.0.join("bin");
        write(&bin.join(".ttyak.new"), "staged");
        write(&bin.join("other-tool"), "not ours");
        let home = t.home();
        let posix = marked(r#"case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) export PATH="$HOME/.local/bin:$PATH" ;; esac"#);
        let bashrc = format!("alias a=b\n{posix}\nexport X=1\n");
        write(&home.join(".bashrc"), &bashrc);
        // No newline at the end, before and after the marked line.
        write(&home.join(".profile"), &format!("a\n{posix}"));
        write(&home.join(".bash_login"), &format!("x\n{posix}\ny"));
        // Two marked lines (two install folders), CRLF, a line that only mentions the marker.
        let mention = format!("echo '{MARKER} and more'");
        write(&home.join(".zshrc"), &format!("{posix}\r\nkeep\r\n{}\r\n{mention}\r\n", marked("export PATH='/opt/t':\"$PATH\"")));
        // The fish file holds only the installer's line: it goes; under XDG_CONFIG_HOME one with more stays.
        let fish = marked("contains -- ~/.local/bin $PATH; or set -gx PATH ~/.local/bin $PATH");
        write(&home.join(".config/fish/conf.d/ttyak.fish"), &format!("{fish}\n"));
        // (Inside the home: a folder outside it is checked for links up to `/`, and macOS's /var is one.)
        let xdg = home.join("xdg");
        write(&xdg.join("fish/conf.d/ttyak.fish"), &format!("set -gx A 1\n{fish}\n"));
        // A linked startup file and one in a linked folder: left alone, their lines listed.
        let dotfiles = t.0.join("dotfiles");
        write(&dotfiles.join("bash_profile"), &format!("{posix}\n"));
        std::os::unix::fs::symlink(dotfiles.join("bash_profile"), home.join(".bash_profile")).unwrap();
        write(&dotfiles.join("elvish/rc.elv"), &format!("{}\n", marked("set paths = [~/.local/bin $@paths]")));
        std::os::unix::fs::symlink(dotfiles.join("elvish"), home.join(".config/elvish")).unwrap();
        // A startup file with no marked line is not touched.
        write(&home.join(".xonshrc"), "$PATH.insert(0, '~/bin')\n");

        let plan = plan_in(&install, false, &home, None, Some(&xdg), false);
        assert_eq!(plan.remove, [bin.join("ttyak"), bin.join("install.json"), bin.join(".ttyak.new")], "the folder stays: it can be a shared bin");
        let edited = |name: &str| PathEdit::ProfileLine(home.join(name));
        for want in [edited(".profile"), edited(".bashrc"), edited(".bash_login"), edited(".zshrc"), edited(".config/fish/conf.d/ttyak.fish"), PathEdit::ProfileLine(xdg.join("fish/conf.d/ttyak.fish"))] {
            assert!(plan.path_edits.contains(&want), "{want:?} in {:?}", plan.path_edits);
        }
        assert_eq!(plan.path_edits.len(), 6, "{:?}", plan.path_edits);
        let left: Vec<&PathBuf> = plan.by_hand.iter().map(|(f, _)| f).collect();
        assert_eq!(left, [&home.join(".bash_profile"), &home.join(".config/elvish/rc.elv")]);
        assert_eq!(plan.by_hand[0].1, std::slice::from_ref(&posix));
        assert!(plan.purge);

        let done = run(&plan);
        assert!(done.iter().all(|(_, r)| r.is_ok()), "{done:?}");
        assert_eq!(done.len(), 9);
        assert!(!bin.join("ttyak").exists() && !bin.join("install.json").exists() && !bin.join(".ttyak.new").exists());
        assert_eq!(read(&bin.join("other-tool")), "not ours");
        assert_eq!(read(&home.join(".bashrc")), "alias a=b\nexport X=1\n");
        assert_eq!(read(&home.join(".profile")), "a\n");
        assert_eq!(read(&home.join(".bash_login")), "x\ny");
        assert_eq!(read(&home.join(".zshrc")), format!("keep\r\n{mention}\r\n"));
        assert!(!home.join(".config/fish/conf.d/ttyak.fish").exists());
        assert!(home.join(".config/fish/conf.d").is_dir(), "only the file goes");
        assert_eq!(read(&xdg.join("fish/conf.d/ttyak.fish")), "set -gx A 1\n");
        assert_eq!(read(&dotfiles.join("bash_profile")), format!("{posix}\n"));
        assert!(home.join(".bash_profile").is_symlink());
        assert_eq!(read(&dotfiles.join("elvish/rc.elv")), format!("{}\n", marked("set paths = [~/.local/bin $@paths]")));
        assert_eq!(read(&home.join(".xonshrc")), "$PATH.insert(0, '~/bin')\n");
    }

    #[test]
    fn a_self_update_cut_short_is_cleaned_up() {
        let t = Temp::new("cut-short");
        for windows in [false, true] {
            let install = t.script("bin", if windows { "ttyak.exe" } else { "ttyak" });
            let bin = t.0.join("bin");
            for name in [".ttyak-new", ".ttyak-update-4242.zip", ".ttyak-update-.zip", ".ttyak-update-12a.zip", "ttyak-update-7.zip"] {
                write(&bin.join(name), "x");
            }
            let plan = plan_in(&install, true, &t.home(), None, None, windows);
            let names: Vec<String> = plan.remove.iter().filter_map(|p| p.strip_prefix(&bin).ok()).map(|p| p.display().to_string()).collect();
            assert!(names.contains(&".ttyak-new".to_string()) && names.contains(&".ttyak-update-4242.zip".to_string()), "{windows}: {names:?}");
            assert!(!names.iter().any(|n| n.ends_with(".zip") && n != ".ttyak-update-4242.zip"), "{windows}: only its own downloads: {names:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_startup_file_is_left_alone() {
        use std::os::unix::fs::PermissionsExt;
        let t = Temp::new("read-only");
        let install = t.script("bin", "ttyak");
        let rc = t.home().join(".zshrc");
        write(&rc, &format!("{}\n", marked("x")));
        std::fs::set_permissions(&rc, std::fs::Permissions::from_mode(0o400)).unwrap();
        let plan = plan_in(&install, true, &t.home(), None, None, false);
        assert_eq!(plan.by_hand, [(rc.clone(), vec![marked("x")])]);
        assert!(plan.path_edits.is_empty() && !plan.purge);
    }

    #[test]
    fn zdotdir_is_looked_at_too() {
        let t = Temp::new("zdotdir");
        let install = t.script("bin", "ttyak");
        let z = t.0.join("zdot");
        write(&z.join(".zshrc"), &format!("{}\n", marked("x")));
        let plan = plan_in(&install, false, &t.home(), Some(&z), None, false);
        assert_eq!(plan.path_edits, [PathEdit::ProfileLine(z.join(".zshrc"))]);
        assert!(plan.notes.is_empty(), "{:?}", plan.notes);
        // ZDOTDIR set in ~/.zshenv but not here: the .zshrc there is not known, and the note says so.
        assert!(plan_in(&install, false, &t.home(), None, None, false).notes.is_empty(), "no .zshenv");
        write(&t.home().join(".zshenv"), "export ZDOTDIR=$HOME/.config/zsh\n");
        let plan = plan_in(&install, false, &t.home(), None, None, false);
        assert_eq!(plan.notes.len(), 1, "{:?}", plan.notes);
        let note = &plan.notes[0];
        assert!(note.contains(".zshenv") && note.contains(&t.home().join(".zshrc").display().to_string()) && note.contains("may remain"), "{note}");
        assert!(plan_in(&install, false, &t.home(), Some(&z), None, false).notes.is_empty(), "ZDOTDIR known");
    }

    #[cfg(unix)]
    #[test]
    fn the_path_lines_stay_while_ttyak_does() {
        use std::os::unix::fs::PermissionsExt;
        let t = Temp::new("binary-stays");
        let install = t.script("bin", "ttyak");
        let rc = t.home().join(".bashrc");
        let text = format!("a\n{}\n", marked("x"));
        write(&rc, &text);
        let plan = plan_in(&install, true, &t.home(), None, None, false);
        let bin = t.0.join("bin");
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o500)).unwrap();
        let done = run(&plan);
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(install.exe.exists());
        assert_eq!(read(&rc), text, "ttyak is still there: still on PATH");
        let line = done.iter().find(|(what, _)| what.contains(&rc.display().to_string())).unwrap();
        assert!(line.0.contains("left") && line.1.is_ok(), "{done:?}");
    }

    #[test]
    fn a_failed_rewrite_puts_the_old_bytes_back() {
        let file = Path::new("/x/.bashrc");
        let calls = std::cell::RefCell::new(Vec::new());
        // The new text fails half-way; the old one goes back.
        let write = |p: &Path, bytes: &[u8]| {
            calls.borrow_mut().push((p.to_path_buf(), bytes.to_vec()));
            if bytes == b"new" { Err(io::Error::other("disk full")) } else { Ok(()) }
        };
        let e = rewrite(file, b"new", b"old", &write).unwrap_err().to_string();
        assert!(e.contains("disk full") && e.contains("as it was"), "{e}");
        assert_eq!(calls.borrow().iter().map(|(_, b)| b.clone()).collect::<Vec<_>>(), [b"new".to_vec(), b"old".to_vec()]);
        // Both fail: said.
        let e = rewrite(file, b"new", b"old", &|_: &Path, _: &[u8]| Err(io::Error::other("disk full"))).unwrap_err().to_string();
        assert!(e.contains("could not be written back"), "{e}");
        assert!(rewrite(file, b"new", b"old", &|_: &Path, _: &[u8]| Ok(())).is_ok());
    }

    #[test]
    fn marked_lines_go_and_every_other_byte_stays() {
        let m = marked("l");
        for (text, want) in [
            (format!("{m}\n"), Some(String::new())),
            (format!("a\n{m}\nb"), Some("a\nb".into())),
            (format!("a\n{m}"), Some("a\n".into())),
            (format!("{m}\n{m}\n\n"), Some("\n".into())),
            ("a\nb\n".into(), None),
            (format!("{m} trailing\n"), None),
            (format!("a\r\n{m}\r\n"), Some("a\r\n".into())),
            (format!("a\n{m} \t\nb\n"), Some("a\nb\n".into())),
        ] {
            assert_eq!(without_marked_lines(text.as_bytes()).map(|b| String::from_utf8(b).unwrap()), want, "{text:?}");
        }
        // Not text: kept byte for byte.
        let bytes = [b"\xff\xfe\n".as_slice(), m.as_bytes(), b"\n\x80"].concat();
        assert_eq!(without_marked_lines(&bytes), Some(b"\xff\xfe\n\x80".to_vec()));
    }

    #[test]
    fn keep_data_leaves_the_folders() {
        let t = Temp::new("keep");
        let install = t.script("bin", "ttyak");
        let data = t.data();
        let plan = plan_in(&install, true, &t.home(), None, None, false);
        assert!(!plan.purge);
        uninstall(&install, plan, &t.dirs(), &data, true, no_terminal()).unwrap();
        assert!(!install.exe.exists());
        for d in &data {
            assert!(d.is_dir(), "{}", d.display());
        }
        assert_eq!(read(&t.dirs().data.join("drafts.json")), "{}");
    }

    #[test]
    fn yes_removes_the_data_too() {
        let t = Temp::new("yes");
        let install = t.script("bin", "ttyak");
        let data = t.data();
        let plan = plan_in(&install, false, &t.home(), None, None, false);
        uninstall(&install, plan, &t.dirs(), &data, true, no_terminal()).unwrap();
        assert!(!install.exe.exists());
        for d in &data {
            assert!(!d.exists(), "{}", d.display());
        }
    }

    #[test]
    fn the_questions_decide() {
        // (answers, program gone, data gone)
        for (answers, gone, data_gone) in [([false, true], true, true), ([true, true], true, false), ([false, false], false, false), ([true, false], false, false)] {
            let t = Temp::new("ask");
            let install = t.script("bin", "ttyak");
            let data = t.data();
            let plan = plan_in(&install, false, &t.home(), None, None, false);
            let mut asked = Vec::new();
            let mut replies = answers.into_iter();
            let mut ask = |q: &str| {
                asked.push(q.to_string());
                Ok(replies.next().unwrap())
            };
            uninstall(&install, plan, &t.dirs(), &data, false, Some(&mut ask)).unwrap();
            assert_eq!(asked.len(), 2, "{asked:?}");
            assert_eq!(asked[0], "Keep your settings, sign-ins and drafts?");
            assert_eq!((!install.exe.exists(), !data[0].exists()), (gone, data_gone), "{answers:?}");
        }
        // --keep-data: only the one question.
        let t = Temp::new("ask-keep");
        let install = t.script("bin", "ttyak");
        let data = t.data();
        let mut asked = Vec::new();
        let mut ask = |q: &str| {
            asked.push(q.to_string());
            Ok(true)
        };
        // (`command` lists no data then.)
        uninstall(&install, plan_in(&install, true, &t.home(), None, None, false), &t.dirs(), &[], false, Some(&mut ask)).unwrap();
        assert_eq!(asked, ["Remove ttyak? Your settings, sign-ins and drafts stay."]);
        assert!(!install.exe.exists() && data[0].exists());
    }

    #[test]
    fn package_install_prints_the_command() {
        for method in [Method::Homebrew, Method::Scoop, Method::Winget, Method::Chocolatey, Method::Aur, Method::Deb, Method::Rpm] {
            let t = Temp::new("package");
            let exe = t.0.join("pkg/ttyak");
            write(&exe, "program");
            let install = Install { method, exe: exe.clone(), writable: false };
            let note = package_note(&install).unwrap();
            let command = method.uninstall_command().unwrap();
            assert!(note.contains(command) && note.contains("ttyak purge"), "{method:?}: {note}");
            let data = t.data();
            let plan = plan_in(&install, false, &t.home(), None, None, false);
            assert_eq!(plan, Plan::default(), "{method:?}");
            uninstall(&install, plan, &t.dirs(), &data, true, no_terminal()).unwrap();
            assert!(exe.exists() && data.iter().all(|d| d.exists()), "{method:?}: nothing is deleted");
        }
        let scoop = package_note(&Install { method: Method::Scoop, exe: PathBuf::from(r"C:\Users\u\scoop\apps\ttyak\current\ttyak.exe"), writable: false }).unwrap();
        assert!(scoop.contains("deletes your settings, sign-ins and drafts too"), "{scoop}");
        let brew = package_note(&Install { method: Method::Homebrew, exe: PathBuf::from("/opt/homebrew/Cellar/ttyak/0.11.0/bin/ttyak"), writable: false }).unwrap();
        assert_eq!(brew, "ttyak was installed with Homebrew. To remove it: brew uninstall ttyak\nTo delete your settings, sign-ins and drafts too, first run: ttyak purge");
        assert_eq!(package_note(&Install { method: Method::Script, exe: PathBuf::from("/x/ttyak"), writable: true }), None);
    }

    #[test]
    fn unknown_install_names_the_file() {
        let t = Temp::new("unknown");
        let exe = t.0.join("Downloads/ttyak");
        write(&exe, "program");
        let install = Install { method: Method::Unknown, exe: exe.clone(), writable: false };
        let note = package_note(&install).unwrap();
        assert!(note.contains(&format!("delete {} yourself", exe.display())) && note.contains("ttyak purge"), "{note}");
        let data = t.data();
        uninstall(&install, plan_in(&install, false, &t.home(), None, None, false), &t.dirs(), &data, true, no_terminal()).unwrap();
        assert!(exe.exists() && data.iter().all(|d| d.exists()));
        let windows = package_note(&Install { method: Method::Unknown, exe: PathBuf::from(r"\\?\C:\Users\u\Downloads\ttyak.exe"), writable: false }).unwrap();
        assert!(windows.contains(r"delete C:\Users\u\Downloads\ttyak.exe yourself"), "{windows}");
    }

    #[test]
    fn no_terminal_without_yes_refuses() {
        let t = Temp::new("no-terminal");
        let install = t.script("bin", "ttyak");
        let data = t.data();
        let plan = plan_in(&install, false, &t.home(), None, None, false);
        let e = uninstall(&install, plan.clone(), &t.dirs(), &data, false, no_terminal()).unwrap_err().to_string();
        assert!(e.contains("--yes") && e.contains("--keep-data"), "{e}");
        assert!(install.exe.exists() && data.iter().all(|d| d.exists()));
        // A folder ttyak cannot write: refused before anything goes, the file named.
        let stuck = Install { writable: false, ..install.clone() };
        let e = uninstall(&stuck, plan, &t.dirs(), &data, true, no_terminal()).unwrap_err().to_string();
        assert!(e.contains(&install.exe.display().to_string()), "{e}");
        assert!(install.exe.exists() && data.iter().all(|d| d.exists()));
    }

    #[test]
    fn running_profile_refuses() {
        let t = Temp::new("running");
        let install = t.script("bin", "ttyak");
        let data = t.data();
        let held = slack_core::profiles::lock(&t.dirs(), "default").unwrap().unwrap();
        let e = uninstall(&install, plan_in(&install, false, &t.home(), None, None, false), &t.dirs(), &data, true, no_terminal()).unwrap_err().to_string();
        assert!(e.contains("running"), "{e}");
        assert!(install.exe.exists() && data.iter().all(|d| d.exists()), "nothing removed");
        // Asked, the same once the data is to go: before the last question.
        let mut asked = 0;
        let mut ask = |_: &str| {
            asked += 1;
            Ok(false)
        };
        let e = uninstall(&install, plan_in(&install, false, &t.home(), None, None, false), &t.dirs(), &data, false, Some(&mut ask)).unwrap_err().to_string();
        assert!(e.contains("running") && asked == 1, "{e}");
        assert!(install.exe.exists() && data.iter().all(|d| d.exists()), "nothing removed");
        // Keeping the data, a running ttyak is no obstacle.
        uninstall(&install, plan_in(&install, true, &t.home(), None, None, false), &t.dirs(), &data, true, no_terminal()).unwrap();
        assert!(!install.exe.exists() && data.iter().all(|d| d.exists()));
        drop(held);
    }

    #[test]
    fn windows_plan() {
        let t = Temp::new("windows");
        let install = t.script("Programs/ttyak", "ttyak.exe");
        let dir = t.0.join("Programs/ttyak");
        for name in ["ttyak.old.exe", "ttyak.old-0123abcd.exe", ".ttyak.exe.new", ".install.json.new", "ttyak.old-xyz.exe", "notes.txt"] {
            write(&dir.join(name), "x");
        }
        write(&t.home().join(".bashrc"), &format!("{}\n", marked("x")));
        let plan = plan_in(&install, false, &t.home(), None, None, true);
        let names: Vec<String> = plan.remove.iter().map(|p| p.strip_prefix(&t.0).unwrap().display().to_string().replace('\\', "/")).collect();
        assert_eq!(names, ["Programs/ttyak/ttyak.exe", "Programs/ttyak/install.json", "Programs/ttyak/ttyak.old-0123abcd.exe", "Programs/ttyak/ttyak.old.exe", "Programs/ttyak/.ttyak.exe.new", "Programs/ttyak/.install.json.new", "Programs/ttyak"]);
        assert_eq!(plan.path_edits, [PathEdit::WindowsUserPath(dir.clone())], "no shell files on Windows");
        // install.json's own spelling of the folder, but only when it is the exe's folder.
        let spelled = dir.to_string_lossy().to_uppercase();
        write(&dir.join("install.json"), &serde_json::json!({"method": "script", "version": "0.11.0", "dir": spelled}).to_string());
        assert_eq!(plan_in(&install, false, &t.home(), None, None, true).path_edits, [PathEdit::WindowsUserPath(PathBuf::from(&spelled))]);
        write(&dir.join("install.json"), r#"{"method":"script","version":"0.11.0","dir":"C:\\Windows\\System32"}"#);
        assert_eq!(plan_in(&install, false, &t.home(), None, None, true).path_edits, [PathEdit::WindowsUserPath(dir.clone())], "another folder is not taken off PATH");
        // Another folder than one named ttyak stays.
        let other = t.script("tools", "ttyak.exe");
        let plan = plan_in(&other, false, &t.home(), None, None, true);
        assert!(!plan.remove.contains(&t.0.join("tools")));
        // The folder from a resolved path, without `\\?\`.
        assert_eq!(plain(Path::new(r"\\?\C:\Users\u\AppData\Local\Programs\ttyak\ttyak.exe")), PathBuf::from(r"C:\Users\u\AppData\Local\Programs\ttyak\ttyak.exe"));
        assert_eq!(plain(Path::new(r"\\?\UNC\server\share\ttyak.exe")), PathBuf::from(r"\\server\share\ttyak.exe"));
        assert_eq!(plain(Path::new("/usr/bin/ttyak")), PathBuf::from("/usr/bin/ttyak"));
    }

    #[test]
    fn the_user_path_loses_only_that_entry() {
        let dir = r"C:\Users\u\AppData\Local\Programs\ttyak";
        for (value, want) in [
            (format!(r"{dir};C:\a;D:\b"), Some(r"C:\a;D:\b".to_string())),
            (r"C:\a;c:\users\U\appdata\local\programs\TTYAK\;D:\b".into(), Some(r"C:\a;D:\b".into())),
            (dir.into(), Some(String::new())),
            (format!(r"C:\a;;{dir};"), Some(r"C:\a;;".into())),
            (format!(r"{dir};C:\a;{dir}"), Some(r"C:\a".into())),
            (format!(r"{dir}\sub;C:\a"), None),
            (r"C:\a;%LOCALAPPDATA%\Programs\ttyak".into(), None),
            (String::new(), None),
        ] {
            assert_eq!(without_entry(&value, dir), want, "{value}");
        }
        assert_eq!(without_entry(r"C:\a;C:\Users\Дмитро\ttyak", r"c:\users\дмитро\TTYAK\"), Some(r"C:\a".into()), "any case beyond ASCII");
    }

    #[test]
    fn the_cleanup_waits_for_the_files() {
        let line = cleanup_line(&[PathBuf::from(r"C:\p\ttyak\ttyak.old-1a2b3c4d.exe")], &[PathBuf::from(r"C:\p\ttyak")]).unwrap();
        assert_eq!(
            line,
            r#"for /l %i in (1,1,120) do @(del /f /q "C:\p\ttyak\ttyak.old-1a2b3c4d.exe" 2>nul & if exist "C:\p\ttyak\ttyak.old-1a2b3c4d.exe" (ping -n 2 127.0.0.1 >nul) else (rmdir "C:\p\ttyak" 2>nul & exit))"#
        );
        let two = cleanup_line(&[PathBuf::from(r"C:\a.exe"), PathBuf::from(r"C:\b.exe")], &[]).unwrap();
        assert_eq!(two, r#"for /l %i in (1,1,120) do @(del /f /q "C:\a.exe" 2>nul & del /f /q "C:\b.exe" 2>nul & if exist "C:\a.exe" (ping -n 2 127.0.0.1 >nul) else if exist "C:\b.exe" (ping -n 2 127.0.0.1 >nul) else (exit))"#);
        assert_eq!(cleanup_line(&[PathBuf::from(r"C:\100%\ttyak.exe")], &[]), None);
        assert_eq!(cleanup_line(&[], &[]), None);
    }
}

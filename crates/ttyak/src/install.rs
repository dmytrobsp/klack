//! How this copy of ttyak was installed: by the install script (`install.json` next to the executable), by a package
//! manager (told by the executable's path or by the package databases' file lists), or by hand; and which release
//! platform this build is.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// `WINGET_ID` as a literal, for `concat!`.
macro_rules! winget_id {
    () => {
        "dmytrobsp.ttyak"
    };
}

/// The id the winget manifest uses (`winget_id` in packaging/identity.json; packaging/test_render.py compares).
#[allow(dead_code)] // the commands use `winget_id!`; only the tests read the constant
pub const WINGET_ID: &str = winget_id!();

/// The file the install scripts write next to the executable.
pub const RECORD_FILE: &str = "install.json";

/// How a copy of ttyak was installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Script,
    Homebrew,
    Scoop,
    Winget,
    Chocolatey,
    Aur,
    Deb,
    Rpm,
    /// A manual download (or nothing tells).
    Unknown,
}

impl Method {
    const ALL: [Method; 9] = [Method::Script, Method::Homebrew, Method::Scoop, Method::Winget, Method::Chocolatey, Method::Aur, Method::Deb, Method::Rpm, Method::Unknown];

    /// The method as `install.json` names it.
    pub fn name(self) -> &'static str {
        match self {
            Method::Script => "script",
            Method::Homebrew => "homebrew",
            Method::Scoop => "scoop",
            Method::Winget => "winget",
            Method::Chocolatey => "chocolatey",
            Method::Aur => "aur",
            Method::Deb => "deb",
            Method::Rpm => "rpm",
            Method::Unknown => "unknown",
        }
    }

    /// The method `name` names.
    pub fn from_name(name: &str) -> Option<Method> {
        Method::ALL.into_iter().find(|m| m.name() == name)
    }

    /// How it was installed, as in "ttyak was installed with Homebrew" (empty for the install script and unknown).
    pub fn how(self) -> &'static str {
        match self {
            Method::Homebrew => "with Homebrew",
            Method::Scoop => "with Scoop",
            Method::Winget => "with winget",
            Method::Chocolatey => "with Chocolatey",
            Method::Aur => "as the Arch package ttyak-bin",
            Method::Deb => "from a .deb package",
            Method::Rpm => "from an .rpm package",
            Method::Script | Method::Unknown => "",
        }
    }

    /// The package manager's command that updates ttyak; None where ttyak updates itself or nothing does it in one
    /// command (a .deb, an .rpm, the Arch package: rebuilt from the release's PKGBUILD, ttyak-bin is not in the AUR yet).
    pub fn update_command(self) -> Option<&'static str> {
        match self {
            Method::Homebrew => Some("brew upgrade ttyak"),
            Method::Scoop => Some("scoop update ttyak"),
            Method::Winget => Some(concat!("winget upgrade ", winget_id!())),
            Method::Chocolatey => Some("choco upgrade ttyak"),
            Method::Script | Method::Aur | Method::Deb | Method::Rpm | Method::Unknown => None,
        }
    }

    /// The package manager's command that removes ttyak; None where ttyak removes itself.
    pub fn uninstall_command(self) -> Option<&'static str> {
        match self {
            Method::Homebrew => Some("brew uninstall ttyak"),
            Method::Scoop => Some("scoop uninstall ttyak"),
            Method::Winget => Some(concat!("winget uninstall ", winget_id!())),
            Method::Chocolatey => Some("choco uninstall ttyak"),
            Method::Aur => Some("sudo pacman -Rns ttyak-bin"),
            Method::Deb => Some("sudo apt remove ttyak"),
            Method::Rpm => Some("sudo dnf remove ttyak"),
            Method::Script | Method::Unknown => None,
        }
    }
}

/// `install.json`, as the install scripts write it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    pub method: String,
    pub version: String,
    pub dir: PathBuf,
}

impl Record {
    /// The `install.json` in `dir`, None when there is none or it cannot be read.
    pub fn read(dir: &Path, fs: &impl Probe) -> Option<Record> {
        fs.read(&dir.join(RECORD_FILE)).and_then(|text| serde_json::from_str(&text).ok())
    }
}

/// This copy of ttyak: how it was installed, where it is, whether ttyak can write its folder.
#[derive(Clone, Debug)]
pub struct Install {
    pub method: Method,
    /// The executable, symlinks resolved.
    pub exe: PathBuf,
    /// Whether ttyak can write the executable's folder; checked for `Method::Script` only (a package manager's
    /// folder is never ours to write), false for the others.
    pub writable: bool,
}

/// The file system as `detect` looks at it, so tests can fake it.
pub trait Probe {
    /// The file's text, None when it cannot be read.
    fn read(&self, p: &Path) -> Option<String>;
    fn exists(&self, p: &Path) -> bool;
    /// Whether a file can be made in `dir`.
    fn writable(&self, dir: &Path) -> bool;
    /// What `dir` holds (empty when it cannot be read).
    fn list(&self, dir: &Path) -> Vec<PathBuf>;
}

/// The real file system.
pub struct System;

impl Probe for System {
    fn read(&self, p: &Path) -> Option<String> {
        std::fs::read_to_string(p).ok()
    }

    fn exists(&self, p: &Path) -> bool {
        p.exists()
    }

    /// Makes and removes a file of a name nothing else uses; a file that cannot be removed again counts as not
    /// writable.
    fn writable(&self, dir: &Path) -> bool {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let probe = dir.join(format!("ttyak-write-{}-{nanos}.tmp", std::process::id()));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&probe) {
            Ok(file) => {
                drop(file);
                match std::fs::remove_file(&probe) {
                    Ok(()) => true,
                    Err(e) => {
                        tracing::warn!("could not remove {}: {e}", probe.display());
                        false
                    }
                }
            }
            Err(_) => false,
        }
    }

    fn list(&self, dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect()
    }
}

/// Where the packages put ttyak on Linux.
const USR_BIN: &str = "/usr/bin/ttyak";

/// A path's folder names, compared in any case on a Windows-style path (a `\` or a drive letter), exactly elsewhere.
struct Parts<'a> {
    names: Vec<&'a str>,
    any_case: bool,
}

impl<'a> Parts<'a> {
    fn new(path: &'a str) -> Parts<'a> {
        let drive = path.as_bytes().get(..2).is_some_and(|b| b[0].is_ascii_alphabetic() && b[1] == b':');
        Parts { names: path.split(['/', '\\']).collect(), any_case: drive || path.contains('\\') }
    }

    fn same(&self, part: &str, name: &str) -> bool {
        if self.any_case { part.eq_ignore_ascii_case(name) } else { part == name }
    }

    /// Whether a folder is named one of `names`.
    fn has(&self, names: &[&str]) -> bool {
        self.names.iter().any(|p| names.iter().any(|n| self.same(p, n)))
    }

    /// Whether a folder `outer` directly holds a folder `inner`.
    fn pair(&self, outer: &str, inner: &str) -> bool {
        self.names.windows(2).any(|w| self.same(w[0], outer) && self.same(w[1], inner))
    }
}

/// Whether `text` has the line `line`.
fn lists(text: Option<String>, line: &str) -> bool {
    text.is_some_and(|t| t.lines().any(|l| l == line))
}

/// Whether pacman's file list of `ttyak-bin` (`/var/lib/pacman/local/ttyak-bin-<version>/files`) has
/// `usr/bin/ttyak`.
fn pacman_owns(fs: &impl Probe) -> bool {
    for entry in fs.list(Path::new("/var/lib/pacman/local")) {
        let name = entry.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let is_ttyak_bin = name.strip_prefix("ttyak-bin-").is_some_and(|v| v.starts_with(|c: char| c.is_ascii_digit()));
        if is_ttyak_bin && lists(fs.read(&entry.join("files")), "usr/bin/ttyak") {
            return true;
        }
    }
    false
}

/// The package manager whose own files hold `exe`, which outweighs `install.json`: Homebrew's `Cellar`, Scoop's
/// `scoop\apps`, winget's `WinGet\Packages`, Chocolatey's `chocolatey\lib`; for `/usr/bin/ttyak` pacman's file list
/// of `ttyak-bin` (the Arch package) or dpkg's of `ttyak`.
fn owner(exe: &Path, fs: &impl Probe) -> Option<Method> {
    let path = exe.to_string_lossy();
    let parts = Parts::new(&path);
    let usr_bin = exe == Path::new(USR_BIN);
    if parts.has(&["Cellar"]) {
        Some(Method::Homebrew)
    } else if parts.pair("scoop", "apps") {
        Some(Method::Scoop)
    } else if parts.pair("WinGet", "Packages") {
        Some(Method::Winget)
    } else if parts.pair("chocolatey", "lib") {
        Some(Method::Chocolatey)
    } else if usr_bin && pacman_owns(fs) {
        Some(Method::Aur)
    } else if usr_bin && lists(fs.read(Path::new("/var/lib/dpkg/info/ttyak.list")), USR_BIN) {
        Some(Method::Deb)
    } else {
        None
    }
}

/// The package manager `exe`'s path only hints at, when there is no `install.json`: a `homebrew` or `linuxbrew`
/// folder, winget's `WinGet\Links`, a `chocolatey` folder; for `/usr/bin/ttyak` an rpm database.
fn hint(exe: &Path, fs: &impl Probe) -> Option<Method> {
    let path = exe.to_string_lossy();
    let parts = Parts::new(&path);
    let rpm = || ["/var/lib/rpm", "/usr/lib/sysimage/rpm"].iter().any(|d| fs.exists(Path::new(d)));
    if parts.has(&["homebrew", "linuxbrew", ".linuxbrew"]) {
        Some(Method::Homebrew)
    } else if parts.pair("WinGet", "Links") {
        Some(Method::Winget)
    } else if parts.has(&["chocolatey"]) {
        Some(Method::Chocolatey)
    } else if exe == Path::new(USR_BIN) && rpm() {
        Some(Method::Rpm)
    } else {
        None
    }
}

/// How the executable `exe` (symlinks resolved) was installed: a package manager whose files hold it, else
/// `install.json` next to it, else what its path hints at, else unknown.
pub fn detect(exe: &Path, fs: &impl Probe) -> Install {
    let dir = exe.parent().filter(|d| !d.as_os_str().is_empty());
    let recorded = || dir.and_then(|d| Record::read(d, fs)).and_then(|r| Method::from_name(&r.method));
    let method = owner(exe, fs).or_else(recorded).or_else(|| hint(exe, fs)).unwrap_or(Method::Unknown);
    let writable = method == Method::Script && dir.is_some_and(|d| fs.writable(d));
    Install { method, exe: exe.to_path_buf(), writable }
}

/// This running copy of ttyak.
pub fn current() -> Install {
    let exe = std::env::current_exe().map(|p| p.canonicalize().unwrap_or(p)).unwrap_or_default();
    detect(&exe, &System)
}

/// The platforms ttyak is released for, as the release archives name them.
const PLATFORMS: [&str; 5] = ["x86_64-linux", "aarch64-linux", "x86_64-macos", "aarch64-macos", "x86_64-windows"];

/// This build's release platform (`x86_64-linux`, …), None for a target ttyak is not released for.
pub fn platform() -> Option<&'static str> {
    PLATFORMS.into_iter().find(|p| p.split_once('-') == Some((std::env::consts::ARCH, std::env::consts::OS)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Files with their text; a folder exists when it is listed in `dirs` or holds a file.
    #[derive(Default)]
    struct Fake {
        files: HashMap<PathBuf, String>,
        dirs: Vec<PathBuf>,
        writable: bool,
    }

    impl Fake {
        fn file(mut self, p: &str, text: &str) -> Fake {
            self.files.insert(PathBuf::from(p), text.into());
            self
        }

        fn dir(mut self, p: &str) -> Fake {
            self.dirs.push(PathBuf::from(p));
            self
        }
    }

    impl Probe for Fake {
        fn read(&self, p: &Path) -> Option<String> {
            self.files.get(p).cloned()
        }

        fn exists(&self, p: &Path) -> bool {
            self.files.contains_key(p) || self.dirs.iter().any(|d| d == p) || self.files.keys().any(|f| f.starts_with(p))
        }

        fn writable(&self, _dir: &Path) -> bool {
            self.writable
        }

        fn list(&self, dir: &Path) -> Vec<PathBuf> {
            let mut out: Vec<PathBuf> = self.files.keys().filter_map(|f| f.strip_prefix(dir).ok()?.components().next().map(|c| dir.join(c))).collect();
            out.sort();
            out.dedup();
            out
        }
    }

    fn method(exe: &str, fs: &Fake) -> Method {
        detect(Path::new(exe), fs).method
    }

    const SCRIPT: &str = r#"{"method":"script","version":"0.11.0","dir":"/opt/x"}"#;

    #[test]
    fn install_json_wins() {
        let fs = Fake { writable: true, ..Fake::default() }.file("/opt/x/install.json", SCRIPT);
        let found = detect(Path::new("/opt/x/ttyak"), &fs);
        assert_eq!((found.method, found.exe, found.writable), (Method::Script, PathBuf::from("/opt/x/ttyak"), true));
        let brew = Fake::default().file("/opt/homebrew/bin/install.json", SCRIPT);
        assert_eq!(method("/opt/homebrew/bin/ttyak", &brew), Method::Script, "a path that only hints at Homebrew");
        let rpm = Fake::default().file("/usr/bin/install.json", SCRIPT).dir("/var/lib/rpm");
        assert_eq!(method("/usr/bin/ttyak", &rpm), Method::Script, "an rpm database alone");
    }

    #[test]
    fn a_manager_owning_the_file_outweighs_install_json() {
        for (exe, want, more) in [
            ("/opt/homebrew/Cellar/ttyak/0.11.0/bin/ttyak", Method::Homebrew, None),
            ("C:/Users/u/scoop/apps/ttyak/0.11.0/ttyak.exe", Method::Scoop, None),
            ("C:/Users/u/AppData/Local/Microsoft/WinGet/Packages/dmytrobsp.ttyak_x/ttyak.exe", Method::Winget, None),
            ("C:/ProgramData/chocolatey/lib/ttyak/tools/ttyak.exe", Method::Chocolatey, None),
            ("/usr/bin/ttyak", Method::Aur, Some(("/var/lib/pacman/local/ttyak-bin-0.11.0-1/files", "usr/bin/ttyak\n"))),
            ("/usr/bin/ttyak", Method::Deb, Some(("/var/lib/dpkg/info/ttyak.list", "/usr/bin/ttyak\n"))),
        ] {
            let record = Path::new(exe).with_file_name("install.json");
            let mut fs = Fake { writable: true, ..Fake::default() }.file(record.to_str().unwrap(), SCRIPT);
            if let Some((list, text)) = more {
                fs = fs.file(list, text);
            }
            let found = detect(Path::new(exe), &fs);
            assert_eq!((found.method, found.writable), (want, false), "{exe}: a stale install.json, a manager's folder never ours to write");
        }
    }

    #[test]
    fn paths_tell_the_manager() {
        let fs = Fake::default();
        for (exe, want) in [
            ("/opt/homebrew/Cellar/ttyak/0.11.0/bin/ttyak", Method::Homebrew),
            ("/usr/local/Cellar/ttyak/0.11.0/bin/ttyak", Method::Homebrew),
            ("/home/linuxbrew/.linuxbrew/Cellar/ttyak/0.11.0/bin/ttyak", Method::Homebrew),
            ("/home/u/.linuxbrew/bin/ttyak", Method::Homebrew),
            ("/opt/homebrew/bin/ttyak", Method::Homebrew),
            (r"C:\Users\u\scoop\apps\ttyak\0.11.0\ttyak.exe", Method::Scoop),
            (r"\\?\C:\Users\u\Scoop\Apps\ttyak\current\ttyak.exe", Method::Scoop),
            (r"C:\Users\u\AppData\Local\Microsoft\WinGet\Packages\dmytrobsp.ttyak_Microsoft.Winget.Source_8wekyb3d8bbwe\ttyak.exe", Method::Winget),
            (r"C:\Users\u\AppData\Local\Microsoft\WINGET\links\ttyak.exe", Method::Winget),
            (r"C:\ProgramData\chocolatey\lib\ttyak\tools\ttyak.exe", Method::Chocolatey),
            (r"C:\ProgramData\Chocolatey\bin\ttyak.exe", Method::Chocolatey),
        ] {
            assert_eq!(method(exe, &fs), want, "{exe}");
        }
        for exe in [r"C:\Users\u\scoop\shims-old\apps\ttyak.exe", r"C:\Users\u\WinGet\ttyak.exe", "/opt/cellarman/ttyak", "/home/u/cellar/ttyak", "/opt/Homebrew/bin/ttyak"] {
            assert_eq!(method(exe, &fs), Method::Unknown, "{exe}: a name inside a folder name does not count, nor another case outside Windows");
        }
    }

    #[test]
    fn usr_bin_by_package_database() {
        let aur = Fake::default().file("/var/lib/pacman/local/ttyak-bin-0.10.0-1/files", "%FILES%\nusr/\nusr/bin/\nusr/bin/ttyak\n").dir("/var/lib/rpm");
        assert_eq!(method("/usr/bin/ttyak", &aur), Method::Aur);
        let other = Fake::default().file("/var/lib/pacman/local/ttyak-bin-0.10.0-1/files", "%FILES%\nusr/bin/ttyak-old\n").file("/var/lib/pacman/local/other-1.0-1/files", "usr/bin/ttyak\n");
        assert_eq!(method("/usr/bin/ttyak", &other), Method::Unknown, "only ttyak-bin's own list, the very file");
        let deb = Fake::default().file("/var/lib/dpkg/info/ttyak.list", "/.\n/usr\n/usr/bin\n/usr/bin/ttyak\n").dir("/var/lib/rpm");
        assert_eq!(method("/usr/bin/ttyak", &deb), Method::Deb);
        assert_eq!(method("/usr/bin/ttyak", &Fake::default().dir("/var/lib/rpm")), Method::Rpm);
        assert_eq!(method("/usr/bin/ttyak", &Fake::default().dir("/usr/lib/sysimage/rpm")), Method::Rpm);
        assert_eq!(method("/usr/local/bin/ttyak", &deb), Method::Unknown, "only /usr/bin/ttyak");
    }

    #[test]
    fn nothing_known_is_unknown() {
        let fs = Fake { writable: true, ..Fake::default() };
        let found = detect(Path::new("/home/u/Downloads/ttyak-0.11.0-x86_64-linux/ttyak"), &fs);
        assert_eq!((found.method, found.writable), (Method::Unknown, false), "only a script install is ours to write");
        assert_eq!(method("/usr/bin/ttyak", &Fake::default()), Method::Unknown, "no package database");
        assert_eq!(method(r"C:\Users\u\Downloads\ttyak.exe", &fs), Method::Unknown);
    }

    #[test]
    fn a_broken_install_json_is_ignored() {
        for text in ["{not json", r#"{"method":"script"}"#, r#"{"method":"teleport","version":"0.11.0","dir":"/x"}"#] {
            let brew = Fake::default().file("/opt/homebrew/bin/install.json", text);
            assert_eq!(method("/opt/homebrew/bin/ttyak", &brew), Method::Homebrew, "{text}");
            let plain = Fake::default().file("/opt/x/install.json", text);
            assert_eq!(method("/opt/x/ttyak", &plain), Method::Unknown, "{text}");
        }
    }

    #[test]
    fn names_and_commands() {
        for m in Method::ALL {
            assert_eq!(Method::from_name(m.name()), Some(m));
        }
        assert_eq!(Method::Winget.update_command(), Some(format!("winget upgrade {WINGET_ID}").as_str()));
        assert_eq!(Method::Winget.uninstall_command(), Some(format!("winget uninstall {WINGET_ID}").as_str()));
        assert_eq!((Method::Aur.update_command(), Method::Deb.update_command()), (None, None), "the Arch package is rebuilt from the release's PKGBUILD");
        assert_eq!((Method::Script.uninstall_command(), Method::Rpm.uninstall_command()), (None, Some("sudo dnf remove ttyak")));
    }

    #[test]
    fn platform_of_this_build_is_named() {
        let name = format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS);
        let released = ["x86_64-linux", "aarch64-linux", "x86_64-macos", "aarch64-macos", "x86_64-windows"];
        assert_eq!(platform(), released.into_iter().find(|p| *p == name));
    }

    #[test]
    fn the_real_folder_check_leaves_nothing_behind() {
        /// A temporary folder, removed with what it holds even when an assertion fails.
        struct Temp(PathBuf);

        impl Drop for Temp {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        let temp = Temp(std::env::temp_dir().join(format!("ttyak-install-{}", std::process::id())));
        std::fs::create_dir_all(&temp.0).unwrap();
        assert!(System.writable(&temp.0));
        assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 0);
        std::fs::remove_dir(&temp.0).unwrap();
        assert!(!System.writable(&temp.0), "a folder that is not there");
    }
}

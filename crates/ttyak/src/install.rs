//! How this copy of ttyak was installed: by the install script (`install.json` next to the executable), by a package
//! manager (told by the executable's path or by the package databases' file lists), or by hand; and which release
//! platform this build is.

// The update screen and `ttyak uninstall` use this; until then nothing outside the tests does.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// `WINGET_ID` as a literal, for `concat!`.
macro_rules! winget_id {
    () => {
        "ttyak.ttyak"
    };
}

/// The id the winget manifest uses.
pub const WINGET_ID: &str = winget_id!();

/// The file the install scripts write next to the executable.
const RECORD_FILE: &str = "install.json";

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

    /// The package manager's command that updates ttyak; None where ttyak updates itself or nothing does it in one
    /// command.
    pub fn update_command(self) -> Option<&'static str> {
        match self {
            Method::Homebrew => Some("brew upgrade ttyak"),
            Method::Scoop => Some("scoop update ttyak"),
            Method::Winget => Some(concat!("winget upgrade ", winget_id!())),
            Method::Chocolatey => Some("choco upgrade ttyak"),
            Method::Aur => Some("yay -Syu ttyak-bin"),
            Method::Script | Method::Deb | Method::Rpm | Method::Unknown => None,
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

/// This copy of ttyak: how it was installed, where it is, whether ttyak can write its folder.
#[derive(Clone, Debug)]
pub struct Install {
    pub method: Method,
    /// The executable, symlinks resolved.
    pub exe: PathBuf,
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

    /// Makes and removes a file of a name nothing else uses.
    fn writable(&self, dir: &Path) -> bool {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let probe = dir.join(format!("ttyak-write-{}-{nanos}.tmp", std::process::id()));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&probe) {
            Ok(file) => {
                drop(file);
                let _ = std::fs::remove_file(&probe);
                true
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

/// The package manager a folder of `exe`'s path names (whole folder names, any case): Homebrew's `Cellar`,
/// `homebrew` or `linuxbrew`, Scoop's `scoop\apps`, winget's `WinGet\Packages` or `WinGet\Links`, `chocolatey`.
fn by_path(exe: &str) -> Option<Method> {
    let parts: Vec<&str> = exe.split(['/', '\\']).collect();
    let any = |names: &[&str]| parts.iter().any(|p| names.iter().any(|n| p.eq_ignore_ascii_case(n)));
    let pair = |a: &str, b: &str| parts.windows(2).any(|w| w[0].eq_ignore_ascii_case(a) && w[1].eq_ignore_ascii_case(b));
    if any(&["Cellar", "homebrew", "linuxbrew", ".linuxbrew"]) {
        Some(Method::Homebrew)
    } else if pair("scoop", "apps") {
        Some(Method::Scoop)
    } else if pair("WinGet", "Packages") || pair("WinGet", "Links") {
        Some(Method::Winget)
    } else if any(&["chocolatey"]) {
        Some(Method::Chocolatey)
    } else {
        None
    }
}

/// The package that put `/usr/bin/ttyak` there, by the package databases' file lists: pacman's `ttyak-bin`
/// (from the AUR), dpkg's `ttyak`, else rpm's when there is an rpm database.
fn by_package_database(fs: &impl Probe) -> Option<Method> {
    let lists = |text: Option<String>, line: &str| text.is_some_and(|t| t.lines().any(|l| l == line));
    let ttyak_bin = |d: &PathBuf| d.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_prefix("ttyak-bin-")).is_some_and(|v| v.starts_with(|c: char| c.is_ascii_digit()));
    let pacman = fs.list(Path::new("/var/lib/pacman/local")).iter().filter(|d| ttyak_bin(d)).any(|d| lists(fs.read(&d.join("files")), "usr/bin/ttyak"));
    if pacman {
        Some(Method::Aur)
    } else if lists(fs.read(Path::new("/var/lib/dpkg/info/ttyak.list")), USR_BIN) {
        Some(Method::Deb)
    } else if ["/var/lib/rpm", "/usr/lib/sysimage/rpm"].iter().any(|d| fs.exists(Path::new(d))) {
        Some(Method::Rpm)
    } else {
        None
    }
}

/// How the executable `exe` (symlinks resolved) was installed: `install.json` next to it, else its path, else (for
/// `/usr/bin/ttyak`) the package databases; and whether ttyak can write its folder.
pub fn detect(exe: &Path, fs: &impl Probe) -> Install {
    let dir = exe.parent().filter(|d| !d.as_os_str().is_empty());
    let recorded = dir
        .and_then(|d| fs.read(&d.join(RECORD_FILE)))
        .and_then(|text| serde_json::from_str::<Record>(&text).ok())
        .and_then(|r| Method::from_name(&r.method));
    let method = recorded
        .or_else(|| by_path(&exe.to_string_lossy()))
        .or_else(|| if exe == Path::new(USR_BIN) { by_package_database(fs) } else { None })
        .unwrap_or(Method::Unknown);
    Install { method, exe: exe.to_path_buf(), writable: dir.is_some_and(|d| fs.writable(d)) }
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
        let brew = Fake::default().file("/opt/homebrew/Cellar/ttyak/0.11.0/bin/install.json", SCRIPT);
        assert_eq!(method("/opt/homebrew/Cellar/ttyak/0.11.0/bin/ttyak", &brew), Method::Script, "even under a Homebrew path");
        let usr = Fake::default().file("/usr/bin/install.json", SCRIPT).file("/var/lib/dpkg/info/ttyak.list", "/usr/bin/ttyak\n");
        assert_eq!(method("/usr/bin/ttyak", &usr), Method::Script, "even where dpkg claims the file");
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
            (r"C:\Users\u\AppData\Local\Microsoft\WinGet\Packages\ttyak.ttyak_Microsoft.Winget.Source_8wekyb3d8bbwe\ttyak.exe", Method::Winget),
            (r"C:\Users\u\AppData\Local\Microsoft\WINGET\links\ttyak.exe", Method::Winget),
            (r"C:\ProgramData\chocolatey\lib\ttyak\tools\ttyak.exe", Method::Chocolatey),
            (r"C:\ProgramData\Chocolatey\bin\ttyak.exe", Method::Chocolatey),
        ] {
            assert_eq!(method(exe, &fs), want, "{exe}");
        }
        for exe in [r"C:\Users\u\scoop\shims-old\apps\ttyak.exe", r"C:\Users\u\WinGet\ttyak.exe", "/opt/cellarman/ttyak"] {
            assert_eq!(method(exe, &fs), Method::Unknown, "{exe}: a name inside a folder name does not count");
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
        assert_eq!((found.method, found.writable), (Method::Unknown, true));
        assert_eq!(method("/usr/bin/ttyak", &Fake::default()), Method::Unknown, "no package database");
        assert_eq!(method(r"C:\Users\u\Downloads\ttyak.exe", &fs), Method::Unknown);
    }

    #[test]
    fn a_broken_install_json_is_ignored() {
        for text in ["{not json", r#"{"method":"script"}"#, r#"{"method":"teleport","version":"0.11.0","dir":"/x"}"#] {
            let brew = Fake::default().file("/opt/homebrew/Cellar/ttyak/0.11.0/bin/install.json", text);
            assert_eq!(method("/opt/homebrew/Cellar/ttyak/0.11.0/bin/ttyak", &brew), Method::Homebrew, "{text}");
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
        assert_eq!((Method::Aur.update_command(), Method::Deb.update_command()), (Some("yay -Syu ttyak-bin"), None));
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
        let dir = std::env::temp_dir().join(format!("ttyak-install-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(System.writable(&dir));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir(&dir).unwrap();
        assert!(!System.writable(&dir), "a folder that is not there");
    }
}

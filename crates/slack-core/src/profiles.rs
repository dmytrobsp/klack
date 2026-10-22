//! Profiles: the folders `<config>/klack/<name>/` (sign-ins) and `<cache>/klack/<name>/` (log,
//! caches) that keep accounts apart, which of them plain `klack` uses (`<config>/klack/default-profile`),
//! and the lock a running klack holds on its profile so it is not removed under it.
//! Every function takes the folders as `Dirs`, so tests work in a temporary folder.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};

/// The file in the config folder naming the default profile.
const DEFAULT_FILE: &str = "default-profile";
/// The file in a profile's cache folder a running klack holds locked.
const LOCK_FILE: &str = "running.lock";

/// klack's three folders: `config` holds a folder per profile and `default-profile`, `cache` a
/// folder per profile, `data` the drafts and sidebar layout (kept per workspace + user).
#[derive(Clone, Debug)]
pub struct Dirs {
    pub config: PathBuf,
    pub cache: PathBuf,
    pub data: PathBuf,
}

impl Dirs {
    /// The folders klack uses on this system (`session::project_dirs`).
    pub fn system() -> Result<Dirs> {
        let dirs = crate::session::project_dirs().ok_or_else(|| anyhow!("no home directory"))?;
        Ok(Dirs { config: dirs.config_dir().to_path_buf(), cache: dirs.cache_dir().to_path_buf(), data: dirs.data_local_dir().to_path_buf() })
    }

    /// The roaming data folder when it is not `data`: on Windows `%APPDATA%\klack\data`; elsewhere it is `data`
    /// itself, so None.
    pub fn roaming_data() -> Option<PathBuf> {
        let dirs = crate::session::project_dirs()?;
        (dirs.data_dir() != dirs.data_local_dir()).then(|| dirs.data_dir().to_path_buf())
    }
}

/// Names next to the profiles that are klack's own: the default-profile file, `settings.toml` and `keys.toml` (and the
/// `.tmp` files they are written through), `themes/` and `locales/` in the config folder, `login-browser/` in the cache
/// folder.
const RESERVED: &[&str] = &[DEFAULT_FILE, "settings.toml", "settings.tmp", "keys.toml", "keys.tmp", "themes", "locales", "login-browser"];

/// Windows' device names: a file named so, with any extension after it (`con.txt`), is the device there.
const DEVICES: &[&str] = &["con", "nul", "aux", "prn", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9"];

/// What klack writes into a profile's cache folder.
const CACHE_FILES: &[&str] = &["klack.log", LOCK_FILE];

/// Why `name` cannot be a profile name, or None when it can. A profile name is a folder name: letters, digits, `-`
/// and `_`, and `.` or `@` inside it (`john.doe`, `me@work`; never first or last, never two dots together, so it can
/// never point outside klack's folders); none of klack's own names next to the profiles nor a Windows device name, in
/// any case (file systems may ignore case).
fn name_problem(name: &str) -> Option<&'static str> {
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@');
    if name.is_empty() || !name.chars().all(allowed) {
        return Some("letters, digits, - and _ only, and . or @ inside the name");
    }
    if name.starts_with(['.', '@']) || name.ends_with(['.', '@']) || name.contains("..") {
        return Some(". and @ only inside the name, never two dots together");
    }
    let lower = name.to_ascii_lowercase();
    if RESERVED.contains(&lower.as_str()) {
        return Some("klack's own name for a file next to the profiles");
    }
    if lower.split('.').next().is_some_and(|stem| DEVICES.contains(&stem)) {
        return Some("a device name on Windows");
    }
    None
}

/// Refuses a name that cannot be a profile's (see `name_problem`).
pub fn valid_name(name: &str) -> Result<()> {
    match name_problem(name) {
        Some(why) => bail!("'{name}' cannot be a profile name: {why}"),
        None => Ok(()),
    }
}

/// Every profile with sign-ins (a folder of `d.config` holding `session.json`), sorted.
pub fn list(d: &Dirs) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(&d.config)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join("session.json").is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| valid_name(n).is_ok())
        .collect();
    names.sort();
    names
}

/// The folders of `d.config` holding `session.json` that `list` leaves out because their names cannot be a profile's,
/// each with why (sorted): sign-ins of an earlier klack under a name it no longer takes.
pub fn skipped(d: &Dirs) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = std::fs::read_dir(&d.config)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join("session.json").is_file())
        .filter_map(|e| match e.file_name().into_string() {
            Ok(name) => name_problem(&name).map(|why| (name, why.to_string())),
            Err(raw) => Some((raw.to_string_lossy().into_owned(), "not a name klack can read".to_string())),
        })
        .collect();
    out.sort();
    out
}

/// The profile plain `klack` uses: the first line of `default-profile`, else `default`.
pub fn default_name(d: &Dirs) -> String {
    std::fs::read_to_string(d.config.join(DEFAULT_FILE))
        .ok()
        .and_then(|text| text.lines().next().map(|l| l.trim().to_string()))
        .filter(|name| valid_name(name).is_ok())
        .unwrap_or_else(|| "default".into())
}

/// Make `name` the default profile; refuses a name that is not a profile.
pub fn set_default(d: &Dirs, name: &str) -> Result<()> {
    valid_name(name)?;
    let known = list(d);
    if !known.iter().any(|n| n == name) {
        let have = if known.is_empty() { "none yet".to_string() } else { known.join(", ") };
        bail!("no profile '{name}' (profiles: {have})");
    }
    std::fs::write(d.config.join(DEFAULT_FILE), format!("{name}\n"))?;
    Ok(())
}

/// The folders of profile `name` that exist: its config and its cache folder.
pub fn paths(d: &Dirs, name: &str) -> Vec<PathBuf> {
    if valid_name(name).is_err() {
        return vec![];
    }
    [d.config.join(name), d.cache.join(name)].into_iter().filter(|p| p.symlink_metadata().is_ok()).collect()
}

/// The folders `remove` would delete, or why it refuses. Only a profile is removed: its config
/// folder holds `session.json`, or it has no config folder and its cache folder holds nothing but
/// what klack writes there (the log, the lock) — left behind by a profile that never signed in.
/// Refused while a klack runs on it.
pub fn removable(d: &Dirs, name: &str) -> Result<Vec<PathBuf>> {
    valid_name(name)?;
    let gone = paths(d, name);
    if gone.is_empty() {
        bail!("no profile '{name}'");
    }
    let (config, cache) = (d.config.join(name), d.cache.join(name));
    let profile = if config.symlink_metadata().is_ok() {
        config.join("session.json").is_file()
    } else {
        cache.symlink_metadata().is_ok_and(|m| m.is_dir())
            && std::fs::read_dir(&cache).is_ok_and(|entries| {
                entries.flatten().all(|e| e.file_type().is_ok_and(|t| t.is_file()) && e.file_name().to_str().is_some_and(|n| CACHE_FILES.contains(&n)))
            })
    };
    if !profile {
        bail!("'{name}' is not a profile (no sign-ins in {})", config.display());
    }
    if is_running(d, name) {
        bail!("profile '{name}' is in use by a running klack — quit it first");
    }
    Ok(gone)
}

/// Delete profile `name`'s folders and return them (see `removable` for what is refused). Drafts
/// and the sidebar layout stay. When it was the default, `default` is the default again.
pub fn remove(d: &Dirs, name: &str) -> Result<Vec<PathBuf>> {
    let gone = removable(d, name)?;
    for path in &gone {
        // A symbolic link is removed itself, never what it points to.
        if path.symlink_metadata()?.is_dir() {
            std::fs::remove_dir_all(path)?;
        } else {
            std::fs::remove_file(path)?;
        }
    }
    let file = d.config.join(DEFAULT_FILE);
    if default_name(d) == name && file.is_file() {
        std::fs::remove_file(file)?;
    }
    Ok(gone)
}

/// Take the lock a running klack holds on profile `name` (`<cache>/<name>/running.lock`): None
/// when another klack holds it.
pub fn lock(d: &Dirs, name: &str) -> Result<Option<std::fs::File>> {
    valid_name(name)?;
    let dir = d.cache.join(name);
    std::fs::create_dir_all(&dir)?;
    let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join(LOCK_FILE))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

/// Whether a klack holds profile `name`'s lock.
pub fn is_running(d: &Dirs, name: &str) -> bool {
    valid_name(name).is_ok() && d.cache.join(name).join(LOCK_FILE).is_file() && matches!(lock(d, name), Ok(None))
}

/// Every folder of the cache that a running klack holds locked: a profile that never signed in
/// has no config folder, so `list` would miss it. Sorted.
pub fn running_profiles(d: &Dirs) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(&d.cache)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| is_running(d, n))
        .collect();
    names.sort();
    names
}

/// Whether `name` is a file klack leaves in the temporary folder: `klack-cookies-…` (a copy of a
/// browser's cookie database) or `klack-<pid>-…` (a video made for the player).
fn is_temp_leftover(name: &str) -> bool {
    if name.starts_with("klack-cookies-") {
        return true;
    }
    let Some(rest) = name.strip_prefix("klack-") else { return false };
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    digits > 0 && rest[digits..].strip_prefix('-').is_some_and(|tail| !tail.is_empty())
}

/// What `purge` deletes, existing paths only: klack's config, cache and data folders, the `more` folders (the
/// roaming data folder, those of the former name), and in `temp` only the entries klack makes (`is_temp_leftover`) —
/// on Unix only those of the user `owner` (a shared /tmp holds other users' too; None: any).
pub fn purge_targets(d: &Dirs, more: &[PathBuf], temp: &Path, owner: Option<u32>) -> Vec<PathBuf> {
    let mut targets: Vec<PathBuf> = [&d.config, &d.cache, &d.data].into_iter().chain(more).cloned().collect();
    let mut leftovers: Vec<PathBuf> = std::fs::read_dir(temp)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_str().is_some_and(is_temp_leftover) && owned_by(e, owner))
        .map(|e| e.path())
        .collect();
    leftovers.sort();
    targets.extend(leftovers);
    // The same folder can come twice (macOS keeps config and data in one): listed and deleted once.
    let mut seen = std::collections::HashSet::new();
    targets.retain(|p| seen.insert(p.clone()));
    targets.retain(|p| p.symlink_metadata().is_ok());
    targets
}

/// Whether the entry `e` (itself, not what a link points to) belongs to the user `owner` (None: anyone's). Other
/// systems than Unix have no such owner here: every entry.
fn owned_by(e: &std::fs::DirEntry, owner: Option<u32>) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        owner.is_none_or(|uid| e.metadata().is_ok_and(|m| m.uid() == uid))
    }
    #[cfg(not(unix))]
    {
        let _ = (e, owner);
        true
    }
}

/// Where `path` points when it is a symbolic link (None when it is not one).
pub fn link_target(path: &Path) -> Option<PathBuf> {
    path.symlink_metadata().ok().filter(|m| m.file_type().is_symlink()).and_then(|_| std::fs::read_link(path).ok())
}

/// Delete `targets`, every one even when another fails; a symbolic link is removed itself, never
/// what it points to. Returns each target with how deleting it went: `Some(where it pointed)` for a link.
pub fn purge(targets: &[PathBuf]) -> Vec<(PathBuf, std::io::Result<Option<PathBuf>>)> {
    targets
        .iter()
        .map(|path| {
            let link = link_target(path);
            let result = match path.symlink_metadata() {
                Ok(m) if m.is_dir() => std::fs::remove_dir_all(path),
                Ok(_) => std::fs::remove_file(path),
                Err(_) => Ok(()),
            };
            (path.clone(), result.map(|()| link))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh set of folders under the temporary folder, removed when dropped.
    struct Temp(PathBuf, Dirs);

    impl Temp {
        fn new(test: &str) -> Temp {
            let base = std::env::temp_dir().join(format!("klack-profiles-{test}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            let d = Dirs { config: base.join("config"), cache: base.join("cache"), data: base.join("data") };
            Temp(base, d)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "{}").unwrap();
    }

    /// Every path under `root`, sorted, to see that nothing changed.
    fn tree(root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut todo = vec![root.to_path_buf()];
        while let Some(dir) = todo.pop() {
            for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                if e.path().is_dir() {
                    todo.push(e.path());
                }
                out.push(e.path());
            }
        }
        out.sort();
        out
    }

    /// Profiles `a` and `b` with sign-ins and caches, `c` without sign-ins, a draft.
    fn three(t: &Temp) {
        touch(&t.1.config.join("a/session.json"));
        touch(&t.1.config.join("b/session.json"));
        std::fs::create_dir_all(t.1.config.join("c")).unwrap();
        touch(&t.1.cache.join("a/klack.log"));
        touch(&t.1.cache.join("b/klack.log"));
        touch(&t.1.data.join("drafts/x.json"));
    }

    #[test]
    fn bad_names_are_refused() {
        let t = Temp::new("bad");
        three(&t);
        // A file next to the base that a bad name could reach.
        touch(&t.0.join("x/session.json"));
        // klack's own folders and files next to the profiles, also in other cases (case-insensitive
        // file systems), and Windows' device names.
        touch(&t.1.config.join("themes/sunset.toml"));
        touch(&t.1.config.join("locales/de.toml"));
        touch(&t.1.cache.join("login-browser/klack-login-1/Cookies"));
        touch(&t.1.config.join("settings.toml"));
        let before = tree(&t.0);
        for name in [
            "", ".", "..", "../x", "a/b", "a\\b", "default-profile", "Default-Profile", "a b", "../config/a", "themes", "Themes", "locales", "login-browser", "con", "NUL", "aux", "prn", "com1", "COM9", "lpt1", "lpt9",
            // A dot or @ only inside a name, never two dots; klack's files next to the profiles; a device name with an
            // extension is still the device on Windows.
            ".a", "a.", "..a", "a..b", ".hidden", "@a", "a@", "settings.toml", "Keys.toml", "settings.tmp", "con.txt", "NUL.json", "com1.x", "lpt9.tar.gz", "Aux.a.b",
        ] {
            assert!(valid_name(name).is_err(), "{name:?}");
            assert!(remove(&t.1, name).is_err(), "{name:?}");
            assert!(set_default(&t.1, name).is_err(), "{name:?}");
            assert!(paths(&t.1, name).is_empty(), "{name:?}");
        }
        assert_eq!(tree(&t.0), before);
        for name in ["default", "a", "Work_2", "b-c", "com", "com10", "console"] {
            assert!(valid_name(name).is_ok(), "{name:?}");
        }
    }

    #[test]
    fn names_of_earlier_versions_still_work() {
        // klack 0.6 took any folder name: a dot or @ inside one is still a profile.
        for name in ["john.doe", "work.acme", "me@work", "a.b.c", "first.last@work.example", "com10.x", "console.app", "themes.old", "x.con"] {
            assert!(valid_name(name).is_ok(), "{name:?}");
        }
        let t = Temp::new("dotted");
        three(&t);
        touch(&t.1.config.join("john.doe/session.json"));
        touch(&t.1.config.join("me@work/session.json"));
        assert_eq!(list(&t.1), ["a", "b", "john.doe", "me@work"]);
        set_default(&t.1, "john.doe").unwrap();
        assert_eq!(default_name(&t.1), "john.doe");
        assert_eq!(remove(&t.1, "me@work").unwrap(), [t.1.config.join("me@work")]);
        assert_eq!(list(&t.1), ["a", "b", "john.doe"]);
    }

    #[test]
    fn folders_list_cannot_use_are_named() {
        let t = Temp::new("skipped");
        three(&t);
        // Sign-ins in folders whose names are not profile names: named with why; a folder without sign-ins, or one
        // that is a profile, is not.
        touch(&t.1.config.join("a b/session.json"));
        touch(&t.1.config.join(".hidden/session.json"));
        touch(&t.1.config.join("con.txt/session.json"));
        std::fs::create_dir_all(t.1.config.join("no sign-ins")).unwrap();
        touch(&t.1.config.join("themes/sunset.toml"));
        let left_out = skipped(&t.1);
        let names: Vec<&str> = left_out.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, [".hidden", "a b", "con.txt"]);
        assert!(left_out.iter().all(|(_, why)| !why.is_empty()));
        assert!(left_out[2].1.contains("device"), "{:?}", left_out[2]);
        assert_eq!(list(&t.1), ["a", "b"]);
        assert!(skipped(&Temp::new("skipped-none").1).is_empty());
    }

    #[test]
    fn profiles_are_listed_and_the_default_marked() {
        let t = Temp::new("list");
        assert!(list(&t.1).is_empty());
        assert_eq!(default_name(&t.1), "default");
        three(&t);
        assert_eq!(list(&t.1), ["a", "b"]);
        assert_eq!(default_name(&t.1), "default");
        set_default(&t.1, "b").unwrap();
        assert_eq!(default_name(&t.1), "b");
        let e = set_default(&t.1, "zz").unwrap_err().to_string();
        assert!(e.contains("zz") && e.contains("a, b"), "{e}");
        assert!(set_default(&t.1, "c").is_err());
        assert_eq!(default_name(&t.1), "b");
        // A broken file names no profile.
        std::fs::write(t.1.config.join("default-profile"), "../x\n").unwrap();
        assert_eq!(default_name(&t.1), "default");
    }

    #[test]
    fn removing_takes_config_and_cache_only() {
        let t = Temp::new("remove");
        three(&t);
        let want = vec![t.1.config.join("a"), t.1.cache.join("a")];
        assert_eq!(paths(&t.1, "a"), want);
        assert_eq!(remove(&t.1, "a").unwrap(), want);
        assert!(!t.1.config.join("a").exists() && !t.1.cache.join("a").exists());
        assert!(t.1.data.join("drafts/x.json").is_file());
        assert!(t.1.config.join("b/session.json").is_file() && t.1.cache.join("b/klack.log").is_file());
        assert_eq!(list(&t.1), ["b"]);
        // Nothing left of it: refused, and no folder is made for it.
        assert!(remove(&t.1, "a").is_err());
        assert!(remove(&t.1, "nope").is_err());
        assert!(!t.1.cache.join("a").exists() && !t.1.cache.join("nope").exists());
    }

    #[test]
    fn only_profiles_are_removed() {
        let t = Temp::new("notprofile");
        three(&t);
        // A config folder without sign-ins, and a cache folder holding something klack's profiles
        // never write, are not profiles.
        touch(&t.1.cache.join("stuff/picture.png"));
        touch(&t.1.cache.join("c/klack.log"));
        let before = tree(&t.0);
        for name in ["c", "stuff"] {
            let e = remove(&t.1, name).unwrap_err().to_string();
            assert!(e.contains("not a profile"), "{name}: {e}");
        }
        assert_eq!(tree(&t.0), before);
        // What a profile without sign-ins left behind in the cache (its log, its lock) goes.
        touch(&t.1.cache.join("old/klack.log"));
        touch(&t.1.cache.join("old/running.lock"));
        assert_eq!(remove(&t.1, "old").unwrap(), [t.1.cache.join("old")]);
        assert!(!t.1.cache.join("old").exists());
    }

    #[test]
    fn removing_the_default_resets_it() {
        let t = Temp::new("default");
        three(&t);
        set_default(&t.1, "a").unwrap();
        remove(&t.1, "b").unwrap();
        assert_eq!(default_name(&t.1), "a");
        remove(&t.1, "a").unwrap();
        assert_eq!(default_name(&t.1), "default");
        assert!(!t.1.config.join("default-profile").exists());
        // The default profile when no file names one.
        touch(&t.1.config.join("default/session.json"));
        remove(&t.1, "default").unwrap();
    }

    #[test]
    fn a_running_profile_is_not_removed() {
        let t = Temp::new("running");
        three(&t);
        assert!(!is_running(&t.1, "a"));
        let held = lock(&t.1, "a").unwrap();
        assert!(held.is_some());
        assert!(is_running(&t.1, "a"));
        // A second klack does not get it.
        assert!(lock(&t.1, "a").unwrap().is_none());
        let e = remove(&t.1, "a").unwrap_err().to_string();
        assert!(e.contains("running"), "{e}");
        assert!(t.1.config.join("a/session.json").is_file());
        drop(held);
        assert!(!is_running(&t.1, "a"));
        remove(&t.1, "a").unwrap();
        assert!(!t.1.cache.join("a").exists());
        // Checking does not make a folder.
        assert!(!is_running(&t.1, "zz") && !t.1.cache.join("zz").exists());
    }

    #[test]
    fn purge_lists_every_klack_folder() {
        let t = Temp::new("purge-list");
        three(&t);
        touch(&t.1.config.join("settings.toml"));
        let roaming = t.0.join("roaming");
        touch(&roaming.join("default/session.json"));
        let gone = t.0.join("roaming-missing");
        let temp = t.0.join("tmp");
        std::fs::create_dir_all(&temp).unwrap();
        let got = purge_targets(&t.1, &[roaming.clone(), gone], &temp, None);
        assert_eq!(got, [t.1.config.clone(), t.1.cache.clone(), t.1.data.clone(), roaming]);
        // A folder that does not exist is not listed.
        std::fs::remove_dir_all(&t.1.data).unwrap();
        assert!(!purge_targets(&t.1, &[], &temp, None).contains(&t.1.data));
        // Deleting takes the whole folders and nothing next to them.
        touch(&t.0.join("other/keep.txt"));
        let targets = purge_targets(&t.1, &[], &temp, None);
        assert!(purge(&targets).iter().all(|(_, r)| r.is_ok()));
        assert!(!t.1.config.exists() && !t.1.cache.exists());
        assert!(t.0.join("other/keep.txt").is_file());
        assert!(purge(&targets).iter().all(|(_, r)| r.is_ok()));
    }

    #[test]
    fn the_roaming_data_folder_is_listed_only_where_it_is_another() {
        // Windows keeps `%APPDATA%\klack\data` apart from `%LOCALAPPDATA%\klack\data`; elsewhere they are one folder.
        let Some(roaming) = Dirs::roaming_data() else {
            assert!(!cfg!(windows));
            return;
        };
        assert!(cfg!(windows));
        assert_ne!(Some(roaming), Dirs::system().ok().map(|d| d.data));
    }

    #[test]
    fn purge_lists_a_repeated_folder_once() {
        let mut t = Temp::new("purge-dup");
        three(&t);
        // As on macOS: config and data are one folder, and an extra folder repeats a listed one.
        t.1.data = t.1.config.clone();
        let temp = t.0.join("tmp");
        let got = purge_targets(&t.1, &[t.1.cache.clone(), t.1.config.clone()], &temp, None);
        assert_eq!(got, [t.1.config.clone(), t.1.cache.clone()]);
    }

    #[cfg(unix)]
    #[test]
    fn purge_goes_on_after_a_failure() {
        use std::os::unix::fs::PermissionsExt;
        let t = Temp::new("purge-fail");
        touch(&t.0.join("locked/stuck/f"));
        touch(&t.0.join("free/f"));
        let (stuck, free) = (t.0.join("locked/stuck"), t.0.join("free"));
        // A folder whose parent cannot be written cannot be removed.
        std::fs::set_permissions(t.0.join("locked"), std::fs::Permissions::from_mode(0o555)).unwrap();
        let results = purge(&[stuck.clone(), free.clone()]);
        std::fs::set_permissions(t.0.join("locked"), std::fs::Permissions::from_mode(0o755)).unwrap();
        // (Running as root nothing is refused: then there is nothing to check.)
        if results[0].1.is_err() {
            assert!(stuck.exists());
        }
        assert_eq!(results[0].0, stuck);
        assert_eq!(results[1].0, free);
        assert!(results[1].1.is_ok() && !free.exists());
    }

    #[test]
    fn purge_keeps_unrelated_temp_files() {
        let t = Temp::new("purge-temp");
        let temp = t.0.join("tmp");
        for name in ["klack-cookies-12-34", "klack-777-F1.mp4", "klackery", "klack-notes.txt", "other", "klack-12", "klack--x", "klack-1a-x", "xklack-1-x"] {
            touch(&temp.join(name));
        }
        std::fs::create_dir_all(temp.join("klack-cookies-5-6")).unwrap();
        let got = purge_targets(&t.1, &[], &temp, None);
        assert_eq!(got, [temp.join("klack-777-F1.mp4"), temp.join("klack-cookies-12-34"), temp.join("klack-cookies-5-6")]);
        assert!(purge(&got).iter().all(|(_, r)| r.is_ok()));
        assert_eq!(tree(&temp).len(), 7);
        assert!(temp.join("klack-notes.txt").is_file() && temp.join("other").is_file() && !temp.join("klack-cookies-5-6").exists());
    }

    #[cfg(unix)]
    #[test]
    fn purge_takes_only_this_user_s_temp_files() {
        use std::os::unix::fs::MetadataExt;
        let t = Temp::new("purge-owner");
        let temp = t.0.join("tmp");
        touch(&temp.join("klack-777-F1.mp4"));
        touch(&temp.join("klack-cookies-1-2"));
        let me = std::fs::metadata(temp.join("klack-777-F1.mp4")).unwrap().uid();
        assert_eq!(purge_targets(&t.1, &[], &temp, Some(me)), [temp.join("klack-777-F1.mp4"), temp.join("klack-cookies-1-2")]);
        // Another user's (a shared /tmp): not listed, so not deleted.
        assert!(purge_targets(&t.1, &[], &temp, Some(me + 1)).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn purge_removes_a_link_and_says_where_it_pointed() {
        let t = Temp::new("purge-link");
        let real = t.0.join("real-config");
        touch(&real.join("a/session.json"));
        std::os::unix::fs::symlink(&real, &t.1.config).unwrap();
        assert_eq!(link_target(&t.1.config), Some(real.clone()));
        assert_eq!(link_target(&real), None);
        let targets = purge_targets(&t.1, &[], &t.0.join("tmp"), None);
        assert_eq!(targets, [t.1.config.clone()]);
        let results = purge(&targets);
        assert_eq!(results[0].1.as_ref().ok(), Some(&Some(real.clone())));
        // The link is gone, what it pointed to stays.
        assert!(t.1.config.symlink_metadata().is_err());
        assert!(real.join("a/session.json").is_file());
        // A folder that is not a link says so too.
        touch(&t.1.cache.join("a/klack.log"));
        assert_eq!(purge(&[t.1.cache.clone()])[0].1.as_ref().ok(), Some(&None));
    }

    #[test]
    fn purge_refuses_while_running() {
        let t = Temp::new("purge-running");
        three(&t);
        assert!(running_profiles(&t.1).is_empty());
        // A profile that never signed in has only a cache folder: still seen.
        let held = lock(&t.1, "fresh").unwrap();
        let other = lock(&t.1, "a").unwrap();
        assert_eq!(running_profiles(&t.1), ["a", "fresh"]);
        drop(held);
        drop(other);
        assert!(running_profiles(&t.1).is_empty());
        // Checking made no folder.
        assert!(!t.1.config.join("fresh").exists());
    }
}

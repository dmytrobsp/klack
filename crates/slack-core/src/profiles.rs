//! Profiles: the folders `<config>/klack/<name>/` (sign-ins) and `<cache>/klack/<name>/` (log,
//! caches) that keep accounts apart, which of them plain `klack` uses (`<config>/klack/default-profile`),
//! and the lock a running klack holds on its profile so it is not removed under it.
//! Every function takes the folders as `Dirs`, so tests work in a temporary folder.

use std::path::PathBuf;

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
}

/// A profile name is a folder name: letters, digits, `-` and `_` only (no dots or slashes, so it
/// can never point outside klack's folders), and not the name of the default-profile file.
pub fn valid_name(name: &str) -> Result<()> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        bail!("'{name}' is not a profile name: letters, digits, - and _ only");
    }
    if name == DEFAULT_FILE {
        bail!("'{name}' cannot be a profile name");
    }
    Ok(())
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

/// Delete profile `name`'s folders and return them; refused while a klack runs on it. Drafts and
/// the sidebar layout stay. When it was the default, `default` is the default again.
pub fn remove(d: &Dirs, name: &str) -> Result<Vec<PathBuf>> {
    valid_name(name)?;
    let gone = paths(d, name);
    if gone.is_empty() {
        bail!("no profile '{name}'");
    }
    if is_running(d, name) {
        bail!("profile '{name}' is in use by a running klack — quit it first");
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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
        let before = tree(&t.0);
        for name in ["", ".", "..", "../x", "a/b", "a\\b", "default-profile", "a b", "../config/a"] {
            assert!(valid_name(name).is_err(), "{name:?}");
            assert!(remove(&t.1, name).is_err(), "{name:?}");
            assert!(set_default(&t.1, name).is_err(), "{name:?}");
            assert!(paths(&t.1, name).is_empty(), "{name:?}");
        }
        assert_eq!(tree(&t.0), before);
        for name in ["default", "a", "Work_2", "b-c"] {
            assert!(valid_name(name).is_ok(), "{name:?}");
        }
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
}

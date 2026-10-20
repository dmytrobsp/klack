//! `klack auth login --from-browser firefox|chrome`: take the `d` cookie from a browser you are already
//! signed in to Slack with. Its cookie database is copied to a temporary folder and read there
//! (nothing of the browser is changed); Chrome's cookies are decrypted with its key from the
//! system keyring (Secret Service on Linux, the Keychain on macOS). Chrome on Windows encrypts
//! them so that only Chrome can read them, so it is not offered there.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Source {
    Firefox,
    Chrome,
}

/// A `d` cookie found in a browser profile.
#[derive(Debug)]
pub struct Found {
    pub profile: PathBuf,
    pub cookie: String,
    /// Last used (microseconds since 1970), to try the freshest first.
    pub last_used: i64,
}

/// Every Slack session cookie in `source`'s profiles (or just in `profile`), freshest first.
pub async fn find(source: Source, profile: Option<&Path>) -> Result<Vec<Found>> {
    let profiles: Vec<PathBuf> = match profile {
        // One profile, or a browser's folder of profiles.
        Some(p) if is_profile(p) => vec![p.to_path_buf()],
        Some(p) => profiles_with(&[p.to_path_buf()], &["cookies.sqlite", "Network/Cookies", "Cookies"]),
        None => match source {
            Source::Firefox => firefox_profiles(),
            Source::Chrome => chrome_profiles(),
        },
    };
    if profiles.is_empty() {
        bail!("no {} profile found (--browser-profile names one)", label(source));
    }
    let mut found = Vec::new();
    let mut key = None;
    for p in profiles {
        let result = match source {
            Source::Firefox => firefox_cookies(&p),
            Source::Chrome => {
                if key.is_none() {
                    key = Some(chrome_key().await?);
                }
                chrome_cookies(&p, key.as_deref().unwrap_or_default())
            }
        };
        match result {
            Ok(list) => found.extend(list),
            Err(e) => tracing::debug!("{}: {e:#}", p.display()),
        }
    }
    found.sort_by(|a, b| b.last_used.cmp(&a.last_used));
    Ok(found)
}

fn is_profile(p: &Path) -> bool {
    ["cookies.sqlite", "Network/Cookies", "Cookies"].iter().any(|d| p.join(d).is_file())
}

/// How the browser names `profile`: Chrome's display name from `Local State` ("Work"), else the
/// folder's name.
pub fn profile_name(profile: &Path) -> String {
    let folder = profile.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let shown = profile
        .parent()
        .and_then(|root| std::fs::read_to_string(root.join("Local State")).ok())
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|v| v["profile"]["info_cache"][&folder]["name"].as_str().map(String::from));
    match shown {
        Some(name) if name != folder => format!("{name} ({folder})"),
        _ => folder,
    }
}

pub fn label(source: Source) -> &'static str {
    match source {
        Source::Firefox => "Firefox",
        Source::Chrome => "Chrome",
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default()
}

/// Profile folders under each of `roots` that hold a file `db` (relative to the profile).
fn profiles_with(roots: &[PathBuf], db: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if db.iter().any(|d| p.join(d).is_file()) {
                out.push(p);
            }
        }
    }
    out
}

fn firefox_profiles() -> Vec<PathBuf> {
    let h = home();
    let roots: Vec<PathBuf> = if cfg!(target_os = "macos") {
        vec![h.join("Library/Application Support/Firefox/Profiles")]
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(|a| vec![PathBuf::from(a).join(r"Mozilla\Firefox\Profiles")]).unwrap_or_default()
    } else {
        vec![h.join(".mozilla/firefox"), h.join("snap/firefox/common/.mozilla/firefox"), h.join(".var/app/org.mozilla.firefox/.mozilla/firefox")]
    };
    profiles_with(&roots, &["cookies.sqlite"])
}

fn chrome_profiles() -> Vec<PathBuf> {
    let h = home();
    let roots: Vec<PathBuf> = if cfg!(target_os = "macos") {
        ["Google/Chrome", "Chromium", "Microsoft Edge", "BraveSoftware/Brave-Browser"].iter().map(|d| h.join("Library/Application Support").join(d)).collect()
    } else if cfg!(windows) {
        return Vec::new();
    } else {
        let mut r: Vec<PathBuf> = ["google-chrome", "chromium", "microsoft-edge", "BraveSoftware/Brave-Browser"].iter().map(|d| h.join(".config").join(d)).collect();
        r.push(h.join(".var/app/com.google.Chrome/config/google-chrome"));
        r.push(h.join(".var/app/org.chromium.Chromium/config/chromium"));
        r
    };
    profiles_with(&roots, &["Network/Cookies", "Cookies"])
}

/// A copy of `db` (and its journal / WAL) in a fresh temporary folder, so a running browser's
/// lock does not matter and nothing of it is written.
struct Copy {
    dir: PathBuf,
    db: PathBuf,
}

impl Copy {
    fn of(db: &Path) -> Result<Copy> {
        let dir = std::env::temp_dir().join(format!("klack-cookies-{}-{}", std::process::id(), rand_suffix()));
        std::fs::create_dir_all(&dir)?;
        let name = db.file_name().ok_or_else(|| anyhow!("no file name"))?;
        let copy = Copy { dir: dir.clone(), db: dir.join(name) };
        std::fs::copy(db, &copy.db).with_context(|| format!("copying {}", db.display()))?;
        for extra in ["-wal", "-journal"] {
            let side = PathBuf::from(format!("{}{extra}", db.display()));
            if side.is_file() {
                let _ = std::fs::copy(&side, dir.join(format!("{}{extra}", name.to_string_lossy())));
            }
        }
        Ok(copy)
    }
}

impl Drop for Copy {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn rand_suffix() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos())
}

fn firefox_cookies(profile: &Path) -> Result<Vec<Found>> {
    let copy = Copy::of(&profile.join("cookies.sqlite"))?;
    let con = rusqlite::Connection::open(&copy.db)?;
    let mut stmt = con.prepare("SELECT value, lastAccessed FROM moz_cookies WHERE name = 'd' AND host IN ('.slack.com', 'slack.com')")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    Ok(rows.flatten().filter(|(v, _)| v.starts_with("xoxd-")).map(|(cookie, last_used)| Found { profile: profile.to_path_buf(), cookie, last_used }).collect())
}

fn chrome_cookies(profile: &Path, key_password: &[u8]) -> Result<Vec<Found>> {
    let db = [profile.join("Network/Cookies"), profile.join("Cookies")].into_iter().find(|p| p.is_file()).ok_or_else(|| anyhow!("no cookie database"))?;
    let copy = Copy::of(&db)?;
    let con = rusqlite::Connection::open(&copy.db)?;
    let version: i64 = con.query_row("SELECT value FROM meta WHERE key = 'version'", [], |r| r.get::<_, String>(0)).ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut stmt = con.prepare("SELECT value, encrypted_value, last_access_utc FROM cookies WHERE name = 'd' AND host_key IN ('.slack.com', 'slack.com')")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?, r.get::<_, i64>(2)?)))?;
    let mut out = Vec::new();
    for (plain, encrypted, last_access) in rows.flatten() {
        let value = if !plain.is_empty() { plain } else { decrypt_chrome(&encrypted, key_password, version)? };
        if value.starts_with("xoxd-") {
            // Chrome counts microseconds from 1601.
            out.push(Found { profile: profile.to_path_buf(), cookie: value, last_used: last_access - 11_644_473_600_000_000 });
        }
    }
    Ok(out)
}

/// PBKDF2 rounds for Chrome's cookie key: 1 on Linux, 1003 on macOS.
const PBKDF2_ROUNDS: u32 = if cfg!(target_os = "macos") { 1003 } else { 1 };

/// Decrypt a Chrome cookie (`v10` / `v11`: AES-128-CBC, IV of 16 spaces, key = PBKDF2-HMAC-SHA1 of
/// the keyring password with salt "saltysalt"); from database version 24 the plaintext starts with
/// a SHA-256 of the host, which is dropped.
fn decrypt_chrome(encrypted: &[u8], password: &[u8], db_version: i64) -> Result<String> {
    use aes::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
    let (prefix, body) = encrypted.split_at(3.min(encrypted.len()));
    // v10 on Linux is the fixed "peanuts" password (no keyring); v11 the keyring's.
    let password: &[u8] = match prefix {
        b"v10" if cfg!(target_os = "linux") => b"peanuts",
        b"v10" | b"v11" => password,
        _ => bail!("unknown cookie encryption ({})", String::from_utf8_lossy(prefix)),
    };
    let key = pbkdf2_sha1(password, b"saltysalt", PBKDF2_ROUNDS);
    let mut buf = body.to_vec();
    let plain = cbc::Decryptor::<aes::Aes128>::new(&key.into(), &[b' '; 16].into()).decrypt_padded_mut::<Pkcs7>(&mut buf).map_err(|_| anyhow!("the cookie does not decrypt (another key?)"))?;
    let plain = if db_version >= 24 && plain.len() >= 32 { &plain[32..] } else { plain };
    Ok(String::from_utf8(plain.to_vec())?)
}

/// One 16-byte block of PBKDF2-HMAC-SHA1 (all a 128-bit key needs).
fn pbkdf2_sha1(password: &[u8], salt: &[u8], rounds: u32) -> [u8; 16] {
    use hmac::{Hmac, Mac};
    let prf = |data: &[u8]| -> [u8; 20] {
        let mut mac = Hmac::<sha1::Sha1>::new_from_slice(password).expect("HMAC takes any key length");
        mac.update(data);
        mac.finalize().into_bytes().into()
    };
    let mut u = prf(&[salt, &1u32.to_be_bytes()].concat());
    let mut t = u;
    for _ in 1..rounds {
        u = prf(&u);
        for (a, b) in t.iter_mut().zip(u) {
            *a ^= b;
        }
    }
    let mut key = [0u8; 16];
    key.copy_from_slice(&t[..16]);
    key
}

/// Chrome's cookie-key password from the system keyring.
async fn chrome_key() -> Result<Vec<u8>> {
    #[cfg(target_os = "linux")]
    {
        secret_service_password(&["chrome", "chromium", "brave", "microsoft-edge"]).await
    }
    #[cfg(target_os = "macos")]
    {
        for service in ["Chrome Safe Storage", "Chromium Safe Storage", "Microsoft Edge Safe Storage", "Brave Safe Storage"] {
            let out = std::process::Command::new("security").args(["find-generic-password", "-w", "-s", service]).output()?;
            if out.status.success() {
                return Ok(String::from_utf8_lossy(&out.stdout).trim().as_bytes().to_vec());
            }
        }
        bail!("no Chrome Safe Storage entry in the Keychain")
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        bail!("Chrome's cookies cannot be read on this system (app-bound encryption): use `klack auth login` (browser window) or Firefox")
    }
}

/// The secret of the first Secret Service item whose `application` is one of `apps` (only the
/// browsers' "Safe Storage" items; nothing else in the keyring is looked at).
#[cfg(target_os = "linux")]
async fn secret_service_password(apps: &[&str]) -> Result<Vec<u8>> {
    use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
    let conn = zbus::Connection::session().await.context("no D-Bus session")?;
    let service = zbus::Proxy::new(&conn, "org.freedesktop.secrets", "/org/freedesktop/secrets", "org.freedesktop.Secret.Service").await?;
    let (_, session): (OwnedValue, OwnedObjectPath) = service.call("OpenSession", &("plain", Value::from(""))).await.context("the keyring (Secret Service) is not available")?;
    for app in apps {
        let attrs = std::collections::HashMap::from([("application", *app)]);
        let (unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = service.call("SearchItems", &(attrs,)).await?;
        if !locked.is_empty() && unlocked.is_empty() {
            let paths: Vec<ObjectPath> = locked.iter().map(|p| p.as_ref()).collect();
            let _: (Vec<OwnedObjectPath>, OwnedObjectPath) = service.call("Unlock", &(paths,)).await?;
        }
        let Some(item) = unlocked.first().or(locked.first()) else { continue };
        let item = zbus::Proxy::new(&conn, "org.freedesktop.secrets", item.as_ref(), "org.freedesktop.Secret.Item").await?;
        let (_, _, secret, _): (OwnedObjectPath, Vec<u8>, Vec<u8>, String) = item.call("GetSecret", &(session.as_ref(),)).await.context("reading the keyring item")?;
        return Ok(secret);
    }
    bail!("no Chrome / Chromium Safe Storage item in the keyring")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encrypt like Chrome does, to check the decryption against.
    fn chrome_encrypt(plain: &[u8], password: &[u8], prefix: &[u8]) -> Vec<u8> {
        use aes::cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
        let key = pbkdf2_sha1(password, b"saltysalt", PBKDF2_ROUNDS);
        let mut out = prefix.to_vec();
        let mut buf = vec![0u8; plain.len() + 16];
        buf[..plain.len()].copy_from_slice(plain);
        let sealed = cbc::Encryptor::<aes::Aes128>::new(&key.into(), &[b' '; 16].into()).encrypt_padded_mut::<Pkcs7>(&mut buf, plain.len()).unwrap();
        out.extend_from_slice(sealed);
        out
    }

    #[test]
    fn pbkdf2_matches_the_rfc_6070_vector() {
        // RFC 6070: P = "password", S = "salt", c = 1 → 0c60c80f961f0e71f3a9b524af6012062fe037a6.
        assert_eq!(pbkdf2_sha1(b"password", b"salt", 1), [0x0c, 0x60, 0xc8, 0x0f, 0x96, 0x1f, 0x0e, 0x71, 0xf3, 0xa9, 0xb5, 0x24, 0xaf, 0x60, 0x12, 0x06]);
        assert_eq!(pbkdf2_sha1(b"password", b"salt", 2)[..4], [0xea, 0x6c, 0x01, 0x4d]);
    }

    #[test]
    fn chrome_cookies_decrypt() {
        // Database version 24: a 32-byte host hash before the value.
        let mut plain = vec![7u8; 32];
        plain.extend(b"xoxd-abc%2Fdef");
        let enc = chrome_encrypt(&plain, b"keyring-password", b"v11");
        assert_eq!(decrypt_chrome(&enc, b"keyring-password", 24).unwrap(), "xoxd-abc%2Fdef");
        // Older databases have no hash.
        assert_eq!(decrypt_chrome(&chrome_encrypt(b"xoxd-old", b"pw", b"v11"), b"pw", 23).unwrap(), "xoxd-old");
        assert!(decrypt_chrome(&enc, b"wrong", 24).is_err(), "another key does not decrypt");
        assert!(decrypt_chrome(b"v99abc", b"pw", 24).is_err());
    }

    #[test]
    fn firefox_cookies_are_read_from_a_copy() {
        let profile = std::env::temp_dir().join(format!("klack-test-ff-{}", rand_suffix()));
        std::fs::create_dir_all(&profile).unwrap();
        {
            // The columns Firefox's moz_cookies has that matter here.
            let con = rusqlite::Connection::open(profile.join("cookies.sqlite")).unwrap();
            con.execute_batch("CREATE TABLE moz_cookies (id INTEGER PRIMARY KEY, name TEXT, value TEXT, host TEXT, lastAccessed INTEGER);
                INSERT INTO moz_cookies (name, value, host, lastAccessed) VALUES
                ('d', 'xoxd-older', '.slack.com', 100), ('d', 'xoxd-newer', '.slack.com', 200),
                ('d', 'xoxd-elsewhere', '.example.com', 300), ('b', 'x', '.slack.com', 400), ('d', 'not-a-session', '.slack.com', 500);").unwrap();
        }
        let mut found = firefox_cookies(&profile).unwrap();
        found.sort_by(|a, b| b.last_used.cmp(&a.last_used));
        assert_eq!(found.iter().map(|f| f.cookie.as_str()).collect::<Vec<_>>(), ["xoxd-newer", "xoxd-older"]);
        std::fs::remove_dir_all(&profile).unwrap();
    }

    #[test]
    fn chrome_profiles_go_by_their_display_names() {
        let root = std::env::temp_dir().join(format!("klack-test-chrome-{}", rand_suffix()));
        std::fs::create_dir_all(root.join("Profile 2")).unwrap();
        std::fs::create_dir_all(root.join("Default")).unwrap();
        std::fs::write(root.join("Local State"), r#"{"profile": {"info_cache": {"Profile 2": {"name": "Work"}, "Default": {"name": "Default"}}}}"#).unwrap();
        assert_eq!(profile_name(&root.join("Profile 2")), "Work (Profile 2)");
        assert_eq!(profile_name(&root.join("Default")), "Default", "no repetition");
        std::fs::remove_file(root.join("Local State")).unwrap();
        assert_eq!(profile_name(&root.join("Profile 2")), "Profile 2", "no Local State: the folder");
        std::fs::remove_dir_all(&root).unwrap();
    }
}

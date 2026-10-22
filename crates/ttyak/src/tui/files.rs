//! Saving attachments and uploading local files.

use std::path::{Path, PathBuf};

use slack_core::types::File;

/// Where saved files go first: `$TTYAK_DOWNLOAD_DIR`, else `setting` (F6, `~` is home) when
/// set, else the user's downloads folder, else home.
pub fn download_dir(setting: &str) -> PathBuf {
    if let Some(dir) = std::env::var_os("TTYAK_DOWNLOAD_DIR") {
        return PathBuf::from(dir);
    }
    if !setting.trim().is_empty() {
        return super::save::expand_home(setting.trim());
    }
    let dirs = directories::UserDirs::new();
    dirs.as_ref().and_then(|d| d.download_dir().map(Path::to_path_buf)).or_else(|| dirs.as_ref().map(|d| d.home_dir().to_path_buf())).unwrap_or_else(|| PathBuf::from("."))
}

/// `dir/name`, or `dir/stem (n).ext` for the first n that does not exist yet. The name is
/// reduced to its last path component so a remote name cannot escape `dir`.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let name = Path::new(name).file_name().and_then(|n| n.to_str()).filter(|n| !n.is_empty() && *n != "..").unwrap_or("file");
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    (1..).map(|n| dir.join(format!("{stem} ({n}){ext}"))).find(|p| !p.exists()).unwrap_or(first)
}

/// Download one attachment into `dir`; returns where it was written.
pub async fn save(client: slack_core::Client, file: File, dir: PathBuf) -> anyhow::Result<PathBuf> {
    let url = file.url_private_download.as_deref().or(file.url_private.as_deref()).ok_or_else(|| anyhow::anyhow!("{} has no download URL", file.name))?;
    let bytes = client.download(url).await?;
    tokio::fs::create_dir_all(&dir).await?;
    let path = unique_path(&dir, &file.name);
    tokio::fs::write(&path, bytes).await?;
    Ok(path)
}

/// A parsed `/upload <path> [message]` command.
#[derive(Debug, PartialEq, Eq)]
pub struct UploadCommand {
    pub path: PathBuf,
    pub message: String,
}

/// Parse `/upload <path> [message]`; the path may be quoted and may start with `~`.
pub fn parse_upload(text: &str) -> Option<Result<UploadCommand, String>> {
    let rest = text.strip_prefix("/upload")?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    let (raw, message) = if let Some(quoted) = rest.strip_prefix('"') {
        match quoted.split_once('"') {
            Some((p, m)) => (p, m),
            None => return Some(Err("missing closing quote".into())),
        }
    } else {
        rest.split_once(char::is_whitespace).unwrap_or((rest, ""))
    };
    if raw.is_empty() {
        return Some(Err("usage: /upload <path> [message]".into()));
    }
    let path = match raw.strip_prefix('~') {
        Some(tail) => directories::UserDirs::new().map(|d| d.home_dir().join(tail.trim_start_matches('/'))).unwrap_or_else(|| PathBuf::from(raw)),
        None => PathBuf::from(raw),
    };
    Some(Ok(UploadCommand { path, message: message.trim().to_string() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_names() {
        let dir = std::env::temp_dir().join(format!("ttyak-files-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(unique_path(&dir, "a.png"), dir.join("a.png"));
        std::fs::write(dir.join("a.png"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "a.png"), dir.join("a (1).png"));
        std::fs::write(dir.join("a (1).png"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "a.png"), dir.join("a (2).png"));
        assert_eq!(unique_path(&dir, "../../etc/passwd"), dir.join("passwd"));
        assert_eq!(unique_path(&dir, ".."), dir.join("file"));
        std::fs::remove_file(dir.join("a.png")).unwrap();
        std::fs::remove_file(dir.join("a (1).png")).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn upload_commands() {
        assert_eq!(parse_upload("hello"), None);
        assert_eq!(parse_upload("/uploads x"), None);
        assert_eq!(parse_upload("/upload /tmp/a.png look at this").unwrap().unwrap(), UploadCommand { path: "/tmp/a.png".into(), message: "look at this".into() });
        assert_eq!(parse_upload("/upload \"/tmp/my file.png\"").unwrap().unwrap().path, PathBuf::from("/tmp/my file.png"));
        assert!(parse_upload("/upload").unwrap().is_err());
        assert!(parse_upload("/upload \"/tmp/x").unwrap().is_err());
        let home = parse_upload("/upload ~/pic.png").unwrap().unwrap().path;
        assert!(home.is_absolute() && home.ends_with("pic.png"));
    }
}

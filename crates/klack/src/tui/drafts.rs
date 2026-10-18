//! Drafts kept across runs and workspace switches: one file per workspace account in the local
//! data folder (`drafts/<team>-<user>.json`, readable by you only), keyed like the drafts in the
//! app (a conversation id, or `<channel>/<thread ts>` for a thread).

use std::collections::HashMap;
use std::path::PathBuf;

use super::composer::Draft;

pub type Drafts = HashMap<String, Draft>;

fn path(team: &str, user: &str) -> Option<PathBuf> {
    slack_core::session::project_dirs().map(|d| d.data_local_dir().join("drafts").join(format!("{team}-{user}.json")))
}

/// The saved drafts (none when there is no file; a broken file is reported and ignored).
pub fn load(team: &str, user: &str) -> Drafts {
    let Some(path) = path(team, user) else { return Drafts::new() };
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text).unwrap_or_else(|e| {
            tracing::warn!("drafts in {}: {e}", path.display());
            Drafts::new()
        }),
        Err(_) => Drafts::new(),
    }
}

fn parse(text: &str) -> serde_json::Result<Drafts> {
    serde_json::from_str(text)
}

/// Save the drafts; with none left the file is removed.
pub fn save(team: &str, user: &str, drafts: &Drafts) {
    let Some(path) = path(team, user) else { return };
    let result = if drafts.is_empty() {
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    } else {
        path.parent().map_or(Ok(()), std::fs::create_dir_all).map_err(anyhow::Error::from).and_then(|_| slack_core::session::write_private(&path, serde_json::to_string(drafts)?.as_bytes()))
    };
    if let Err(e) = result {
        tracing::warn!("saving drafts to {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drafts_round_trip() {
        let mut c = super::super::composer::Composer::default();
        c.set("héllo @bob 👋");
        let mut drafts = Drafts::new();
        drafts.insert("D0CA2V8G8CE".into(), c.take_draft().unwrap());
        c.set("in a thread");
        drafts.insert("C0CA2V1L12A/1792360258.578039".into(), c.take_draft().unwrap());
        let text = serde_json::to_string(&drafts).unwrap();
        assert!(text.contains("\"text\":\"héllo @bob 👋\""), "the text is kept as a string: {text}");
        assert_eq!(parse(&text).unwrap(), drafts);
        // A cursor past the end (an edited file) is put at the end.
        let d: Draft = serde_json::from_str(r#"{"text":"ab","cursor":9,"mentions":[],"editing":null}"#).unwrap();
        c.restore(d);
        assert_eq!(c.cursor(), 2);
        assert!(parse("not json").is_err());
    }
}

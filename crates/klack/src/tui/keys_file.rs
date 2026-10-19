//! Edits to `keys.toml` that leave the rest of the file as the user wrote it (comments,
//! order, spacing), and the commented template `klack ctl keys-template` prints.

use toml_edit::{DocumentMut, Item, Table, Value};

use super::keymap::{Context, Key, Keymap, ACTIONS, GROUPS};

/// One change to `keys.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// An action's sequences in keys.toml notation; `None` removes the entry (back to the default).
    Binding { context: Context, name: &'static str, seqs: Option<Vec<String>> },
    /// The leader key; `None` removes the entry (back to the default).
    Leader(Option<String>),
    /// The leader while typing and in dialogs; `None` removes the entry (back to the default).
    InsertLeader(Option<String>),
    /// A prefix's name and icon in `[groups]`; both `None` removes the entry.
    Group { prefix: String, name: Option<String>, icon: Option<String> },
}

/// The file text with `change` applied and nothing else touched; Err when the text does not parse.
pub fn apply(text: &str, change: &Change) -> Result<String, String> {
    let mut doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;
    match change {
        Change::Binding { context, name, seqs } => set(&mut doc, context.name(), name, seqs.as_ref().map(|s| Value::Array(s.iter().map(|s| Value::from(s.as_str())).collect())))?,
        Change::Leader(key) => set_root(&mut doc, "leader", key.as_deref()),
        Change::InsertLeader(key) => set_root(&mut doc, "insert_leader", key.as_deref()),
        Change::Group { prefix, name, icon } => {
            let value = match (name, icon) {
                (None, None) => None,
                (Some(name), None) => Some(Value::from(name.as_str())),
                (name, Some(icon)) => {
                    let mut t = toml_edit::InlineTable::new();
                    if let Some(name) = name {
                        t.insert("name", Value::from(name.as_str()));
                    }
                    t.insert("icon", Value::from(icon.as_str()));
                    Some(Value::InlineTable(t))
                }
            };
            set(&mut doc, "groups", prefix, value)?;
        }
    }
    Ok(doc.to_string())
}

/// Write `text` to `path` as a whole: into a file beside it, then renamed over it, so a crash
/// never leaves half a file. The folder is made if needed; the file keeps its permissions, and
/// a symlinked keys.toml (dotfiles) stays a link: its target is written.
pub fn write(path: &std::path::Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    std::fs::create_dir_all(path.parent().ok_or("no folder for keys.toml")?).map_err(|e| e.to_string())?;
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = target.parent().ok_or("no folder for keys.toml")?;
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let temp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let written = (|| {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(text.as_bytes())?;
        if let Ok(meta) = std::fs::metadata(&target) {
            file.set_permissions(meta.permissions())?;
        }
        file.sync_all()?;
        std::fs::rename(&temp, &target)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written.map_err(|e| e.to_string())
}

/// What F6 says once keys.toml is written to `path`: saved, with a note when the keymap warns
/// (a warning about the changed entry first, e.g. a default that gave way to it).
pub fn saved_note(path: &str, warnings: &[String], change: &Change) -> String {
    let about = match change {
        Change::Binding { name, .. } => name.to_string(),
        Change::Leader(_) => "leader".into(),
        Change::InsertLeader(_) => "insert_leader".into(),
        Change::Group { prefix, .. } => prefix.clone(),
    };
    // Whole words only: `copy` is not in `copy_link`.
    let mentions = |w: &str| if about.contains(' ') { w.contains(&about) } else { w.split(|c: char| !(c.is_alphanumeric() || c == '_')).any(|t| t == about) };
    let note = warnings.iter().find(|w| mentions(w)).or(warnings.first());
    match note {
        Some(w) => format!("{path} · note: {}", w.trim_start_matches("keys.toml: ")),
        None => path.to_string(),
    }
}

/// A top-level string (`leader`), or removed.
fn set_root(doc: &mut DocumentMut, key: &str, value: Option<&str>) {
    match value {
        Some(v) => set_value(doc.as_table_mut(), key, Value::from(v)),
        None => {
            doc.remove(key);
        }
    }
}

/// `key` in the table `table` set to `value` (removed for None); the table is made if it is missing.
fn set(doc: &mut DocumentMut, table: &str, key: &str, value: Option<Value>) -> Result<(), String> {
    let Some(value) = value else {
        if let Some(item) = doc.get_mut(table) {
            item.as_table_like_mut().ok_or_else(|| format!("[{table}] is not a table"))?.remove(key);
        }
        return Ok(());
    };
    if !doc.contains_key(table) {
        doc.insert(table, Item::Table(Table::new()));
    }
    let t = doc.get_mut(table).and_then(Item::as_table_like_mut).ok_or_else(|| format!("[{table}] is not a table"))?;
    match t.get_mut(key) {
        // The line keeps its comment.
        Some(item) => {
            let mut value = value;
            if let Some(old) = item.as_value() {
                *value.decor_mut() = old.decor().clone();
            }
            *item = Item::Value(value);
        }
        None => {
            t.insert(key, Item::Value(value));
        }
    }
    Ok(())
}

fn set_value(t: &mut Table, key: &str, mut value: Value) {
    if let Some(old) = t.get(key).and_then(Item::as_value) {
        *value.decor_mut() = old.decor().clone();
    }
    t.insert(key, Item::Value(value));
}

/// A string as a TOML basic string.
fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A complete keys.toml with every default written out and commented, by section of the shared tree.
pub fn template(keys: &Keymap) -> String {
    let spec = |seq: &[Key]| seq.iter().map(Key::spec).collect::<Vec<_>>().join(" ");
    let mut out = String::from("# klack key bindings. Everything here is the default; change or remove a line.\n# An action is a key or a list of keys (\"ctrl+k\", \"<leader> s\", [\"G\", \"end\"]); [] unbinds it.\n# A line that is left out keeps its default. F6 > Keys edits this file without touching your comments.\n\n");
    let leader = |key: Option<Key>| quote(&key.map(|k| k.spec()).unwrap_or_default());
    out += &format!("# The leader of the panes, and of typing and dialogs (\"\" for none).\nleader = {}\ninsert_leader = {}\n\n", leader(keys.leader), leader(keys.insert_leader));
    out += "# Names (and icons) of the prefixes in the which-key popup.\n[groups]\n";
    for (seq, name, icon) in GROUPS {
        out += &format!("{} = {{ name = {}, icon = {} }}\n", quote(seq), quote(name), quote(icon));
    }
    out += "\n# Labels and icons of single actions: [labels.messages] react = { label = \"quick\", icon = \"⚡\" }\n";
    // One table per context, in the order the tree first reaches it; a comment where a section starts.
    let mut tables: Vec<(Context, Vec<(String, Vec<String>)>)> = Vec::new();
    for section in super::keymap::tree(keys) {
        for row in &section.rows {
            let info = &ACTIONS[row.info];
            let names: Vec<String> = if info.name == "workspace_1" { (1..=9).map(|n| format!("workspace_{n}")).collect() } else { vec![info.name.to_string()] };
            for name in names {
                let Some(info) = ACTIONS.iter().find(|a| a.context == info.context && a.name == name.as_str()) else { continue };
                let list: Vec<String> = keys.sequences(info.context, &name).iter().map(|s| quote(&spec(s))).collect();
                let line = format!("{name} = [{}]  # {}", list.join(", "), info.description);
                let at = match tables.iter().position(|(c, _)| *c == info.context) {
                    Some(at) => at,
                    None => {
                        tables.push((info.context, Vec::new()));
                        tables.len() - 1
                    }
                };
                let sections = &mut tables[at].1;
                match sections.last_mut() {
                    Some((title, lines)) if *title == section.title => lines.push(line),
                    _ => sections.push((section.title.clone(), vec![line])),
                }
            }
        }
    }
    for (context, sections) in tables {
        out += &format!("\n[{}]\n", context.name());
        for (title, lines) in sections {
            out += &format!("# {title}\n{}\n", lines.join("\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::keymap::LEADER;
    use super::*;

    fn bind(context: Context, name: &'static str, seqs: Option<&[&str]>) -> Change {
        Change::Binding { context, name, seqs: seqs.map(|s| s.iter().map(|s| s.to_string()).collect()) }
    }

    #[test]
    fn comments_and_order_survive() {
        let text = "# my keys\n\n[messages]\n# mine\nreact = \"r\"  # quick\n";
        let out = apply(text, &bind(Context::Messages, "pin", Some(&["P"]))).unwrap();
        assert!(out.starts_with("# my keys\n\n[messages]\n# mine\nreact = \"r\"  # quick\n"), "{out}");
        assert!(out.contains("pin = [\"P\"]"), "{out}");
        assert!(out.find("react").unwrap() < out.find("pin").unwrap(), "{out}");
        let (map, warnings) = Keymap::load(&out);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(map.sequences(Context::Messages, "pin").len(), 1);
    }

    #[test]
    fn rebinding_keeps_the_comment_on_the_line() {
        let out = apply("[messages]\nreact = \"r\"  # quick\n", &bind(Context::Messages, "react", Some(&["<leader> x", "y"]))).unwrap();
        assert_eq!(out, "[messages]\nreact = [\"<leader> x\", \"y\"]  # quick\n");
    }

    #[test]
    fn other_defaults_are_not_written() {
        let out = apply("[messages]\nreact = \"e\"\n", &bind(Context::Messages, "pin", Some(&["P"]))).unwrap();
        assert!(!out.contains("edit"), "{out}");
        assert_eq!(out, "[messages]\nreact = \"e\"\npin = [\"P\"]\n");
    }

    #[test]
    fn reset_removes_the_entry() {
        let out = apply("[messages]\nreact = \"r\"\npin = [\"P\"]\n", &bind(Context::Messages, "pin", None)).unwrap();
        assert_eq!(out, "[messages]\nreact = \"r\"\n");
        assert_eq!(apply("", &bind(Context::Messages, "pin", None)).unwrap(), "");
    }

    #[test]
    fn unbind_writes_empty_list() {
        let out = apply("", &bind(Context::Messages, "pin", Some(&[]))).unwrap();
        assert_eq!(out, "[messages]\npin = []\n");
        assert!(Keymap::load(&out).0.sequences(Context::Messages, "pin").is_empty());
    }

    #[test]
    fn creates_from_nothing() {
        let out = apply("", &bind(Context::Global, "quit", Some(&["ctrl+q"]))).unwrap();
        assert_eq!(out, "[global]\nquit = [\"ctrl+q\"]\n");
    }

    #[test]
    fn leader_and_group_edits() {
        let out = apply("# top\n[messages]\nreact = \"r\"\n", &Change::Leader(Some("ctrl+b".into()))).unwrap();
        assert!(out.starts_with("# top\nleader = \"ctrl+b\"\n") || out.starts_with("leader = \"ctrl+b\"\n# top"), "{out}");
        assert_eq!(Keymap::load(&out).0.leader.map(|k| k.spec()), Some("ctrl+b".to_string()));
        let out = apply(&out, &Change::InsertLeader(Some(String::new()))).unwrap();
        assert!(out.contains("insert_leader = \"\""), "{out}");
        let out = apply(&out, &Change::InsertLeader(None)).unwrap();
        assert!(!out.contains("insert_leader"), "{out}");
        let out = apply(&out, &Change::Leader(None)).unwrap();
        assert!(!out.contains("leader"), "{out}");
        let out = apply(&out, &Change::Group { prefix: "<leader> h".into(), name: Some("calls".into()), icon: Some("📞".into()) }).unwrap();
        assert!(out.contains("[groups]\n\"<leader> h\" = { name = \"calls\", icon = \"📞\" }"), "{out}");
        let (map, warnings) = Keymap::load(&out);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(map.group(&[LEADER, Key::parse("h").unwrap()]), Some(("calls".into(), "📞".into())));
        let out = apply(&out, &Change::Group { prefix: "<leader> h".into(), name: Some("calls".into()), icon: None }).unwrap();
        assert!(out.contains("\"<leader> h\" = \"calls\""), "{out}");
        let out = apply(&out, &Change::Group { prefix: "<leader> h".into(), name: None, icon: None }).unwrap();
        assert!(!out.contains("<leader> h"), "{out}");
    }

    #[test]
    fn inline_and_dotted_tables() {
        let out = apply("messages = { react = \"r\" }\n", &bind(Context::Messages, "pin", Some(&["P"]))).unwrap();
        let (map, warnings) = Keymap::load(&out);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!((map.sequences(Context::Messages, "react").len(), map.sequences(Context::Messages, "pin").len()), (1, 1), "{out}");
        assert!(out.starts_with("messages = { react = \"r\""), "{out}");
        let out = apply("messages.react = \"r\"\n", &bind(Context::Messages, "react", Some(&["R"]))).unwrap();
        assert_eq!(Keymap::load(&out).0.sequences(Context::Messages, "react"), vec![vec![Key::parse("R").unwrap()]], "{out}");
        assert!(out.starts_with("messages.react = "), "dotted stays dotted: {out}");
    }

    #[test]
    fn quotes_and_backslashes_round_trip() {
        let out = apply("", &bind(Context::Messages, "react", Some(&["\""]))).unwrap();
        let out = apply(&out, &bind(Context::Messages, "pin", Some(&["\\"]))).unwrap();
        let (map, warnings) = Keymap::load(&out);
        assert!(warnings.is_empty(), "{warnings:?}\n{out}");
        assert_eq!(map.sequences(Context::Messages, "react"), vec![vec![Key::parse("\"").unwrap()]], "{out}");
        assert_eq!(map.sequences(Context::Messages, "pin"), vec![vec![Key::parse("\\").unwrap()]], "{out}");
    }

    #[test]
    fn writing_replaces_the_file_whole() {
        let dir = std::env::temp_dir().join(format!("klack-keys-write-{}", std::process::id()));
        let path = dir.join("keys.toml");
        write(&path, "leader = \"space\"\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "leader = \"space\"\n", "the folder is made");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        write(&path, "leader = \"ctrl+b\"\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "leader = \"ctrl+b\"\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "the file keeps its permissions");
        }
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left, ["keys.toml"], "no temporary file is left");
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_file_stays_a_link() {
        let dir = std::env::temp_dir().join(format!("klack-keys-link-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("dotfiles")).unwrap();
        let (target, link) = (dir.join("dotfiles/keys.toml"), dir.join("keys.toml"));
        std::fs::write(&target, "# mine\n").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        write(&link, "leader = \"ctrl+b\"\n").unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link stays a link");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "leader = \"ctrl+b\"\n", "its target is written");
        let left: Vec<_> = std::fs::read_dir(dir.join("dotfiles")).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left, ["keys.toml"], "no temporary file is left");
        std::fs::remove_file(&link).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saved_with_a_note() {
        let copy = bind(Context::Messages, "copy", Some(&["<leader> r"]));
        assert_eq!(saved_note("/k.toml", &[], &copy), "/k.toml");
        let warnings = vec!["keys.toml: [global] mute: unknown key 'hyper' (keeping the default)".to_string(), "keys.toml: [messages] default Space r of react gives way to copy".to_string()];
        assert_eq!(saved_note("/k.toml", &warnings, &copy), "/k.toml · note: [messages] default Space r of react gives way to copy", "the warning about the change first");
        let copy_link = bind(Context::Messages, "copy_link", Some(&["x"]));
        assert_eq!(saved_note("/k.toml", &warnings, &copy_link), "/k.toml · note: [global] mute: unknown key 'hyper' (keeping the default)", "else the first; copy is not copy_link");
        let group = Change::Group { prefix: "<leader> h".into(), name: Some("calls".into()), icon: None };
        assert_eq!(saved_note("/k.toml", &["keys.toml: x".into(), "keys.toml: [groups] \"<leader> h\": bad".into()], &group), "/k.toml · note: [groups] \"<leader> h\": bad");
    }

    #[test]
    fn broken_file_is_not_touched() {
        assert!(apply("[messages\nreact=", &bind(Context::Messages, "pin", Some(&["P"]))).is_err());
        assert!(apply("messages = 3\n", &bind(Context::Messages, "pin", Some(&["P"]))).is_err());
    }

    #[test]
    fn template_loads_clean() {
        let defaults = Keymap::load("").0;
        let text = template(&defaults);
        let (map, warnings) = Keymap::load(&text);
        assert!(warnings.is_empty(), "{warnings:?}");
        for info in ACTIONS {
            assert_eq!(map.sequences(info.context, info.name), defaults.sequences(info.context, info.name), "{} {}", info.context.name(), info.name);
        }
        assert!(text.contains("insert_leader = \"ctrl+space\""), "{text}");
        assert!(text.contains("leader = \"space\""), "{text}");
        for section in super::super::keymap::tree(&defaults) {
            assert!(text.lines().any(|l| l.trim_start().starts_with('#') && l.contains(&section.title)), "no comment for {}", section.title);
        }
        assert!(text.contains("react = ") && text.contains("  # "), "{text}");
        assert_eq!(map.group(&[LEADER, Key::parse("h").unwrap()]).map(|g| g.1), Some("🎧".to_string()));
    }
}

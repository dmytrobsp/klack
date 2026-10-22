//! Slack data types. Every field is optional/defaulted: the web API omits fields freely
//! and adds new ones all the time.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TextValue {
    pub value: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Conversation {
    pub id: String,
    pub name: String,
    pub is_channel: bool,
    pub is_group: bool,
    pub is_im: bool,
    pub is_mpim: bool,
    pub is_private: bool,
    pub is_archived: bool,
    pub is_member: bool,
    pub is_general: bool,
    /// Other party of a DM.
    pub user: Option<String>,
    pub topic: TextValue,
    pub purpose: TextValue,
    pub last_read: Option<String>,
    pub num_members: Option<u64>,
    pub created: Option<f64>,
    pub latest: Option<Value>,
}

impl Conversation {
    pub fn is_dm(&self) -> bool {
        self.is_im || self.is_mpim
    }
}

/// A name Slack accepts for a new channel: lowercase, spaces and other characters as
/// hyphens (runs collapsed), at most 80 characters.
pub fn channel_name(input: &str) -> String {
    let mut out = String::new();
    for c in input.trim().trim_start_matches('#').chars().flat_map(char::to_lowercase) {
        let c = if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '-' };
        if !(c == '-' && out.ends_with('-')) {
            out.push(c);
        }
    }
    out.trim_matches('-').chars().take(80).collect()
}

/// A slash command Slack knows in a workspace: its own and its apps'.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlashCommand {
    /// With its slash: `/remind`.
    pub name: String,
    /// What it takes, e.g. `[#channel] [what] [when]`.
    pub usage: String,
    pub desc: String,
}

impl SlashCommand {
    /// The commands in a `commands.list` answer, by name.
    pub fn from_list(v: &serde_json::Value) -> Vec<SlashCommand> {
        let text = |c: &serde_json::Value, k: &str| c[k].as_str().unwrap_or_default().trim().to_string();
        let mut out: Vec<SlashCommand> = v["commands"]
            .as_object()
            .map(|all| all.values().filter_map(|c| Some(SlashCommand { name: c["name"].as_str().filter(|n| n.starts_with('/'))?.to_string(), usage: text(c, "usage"), desc: text(c, "desc") })).collect())
            .unwrap_or_default();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn profile_reads_huddle_state() {
        let p: super::UserProfile = serde_json::from_value(serde_json::json!({"huddle_state": "in_a_huddle", "huddle_state_expiration_ts": 1000})).unwrap();
        assert_eq!((p.huddle_state.as_str(), p.huddle_state_expiration_ts), ("in_a_huddle", 1000));
        let p: super::UserProfile = serde_json::from_value(serde_json::json!({"display_name": "bob"})).unwrap();
        assert_eq!((p.huddle_state.as_str(), p.huddle_state_expiration_ts), ("", 0));
        // Slack also sends null (one person's null must not lose the whole user list).
        let p: super::UserProfile = serde_json::from_value(serde_json::json!({"huddle_state": null, "huddle_state_expiration_ts": null, "display_name": "bob"})).unwrap();
        assert_eq!((p.huddle_state.as_str(), p.huddle_state_expiration_ts, p.display_name.as_str()), ("", 0, "bob"));
        // Slack's client (and klack) renew it with a fraction of a second: not a lost profile.
        let p: super::UserProfile = serde_json::from_value(serde_json::json!({"huddle_state": "in_a_huddle", "huddle_state_expiration_ts": 1792616700.75, "display_name": "bob"})).unwrap();
        assert_eq!((p.huddle_state_expiration_ts, p.display_name.as_str()), (1792616700, "bob"));
        assert!(serde_json::from_value::<super::UserProfile>(serde_json::json!({"huddle_state_expiration_ts": "soon"})).is_err(), "not a number");
    }

    #[test]
    fn profile_reads_the_big_pictures() {
        let p: super::UserProfile = serde_json::from_value(serde_json::json!({"image_192": "https://a/1_192.png", "image_512": "https://a/1_512.png", "image_original": "https://a/1_original.png"})).unwrap();
        assert_eq!((p.image_192.as_deref(), p.image_512.as_deref(), p.image_original.as_deref()), (Some("https://a/1_192.png"), Some("https://a/1_512.png"), Some("https://a/1_original.png")));
        let p: super::UserProfile = serde_json::from_value(serde_json::json!({"image_192": "https://a/1_192.png"})).unwrap();
        assert_eq!((p.image_512, p.image_original), (None, None), "Slack's default pictures have no original");
    }

    #[test]
    fn slash_commands_from_slack() {
        use super::SlashCommand;
        let v = serde_json::json!({"ok": true, "commands": {
            "/remind": {"name": "/remind", "usage": "[#channel] [what] [when]", "desc": "Set a reminder", "type": "core"},
            "/close": {"name": "/close", "usage": null, "desc": "", "type": "core"},
            "/deploy": {"name": "/deploy", "usage": "app ", "desc": "Ship it", "type": "app"}}});
        let list = SlashCommand::from_list(&v);
        assert_eq!(list.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["/close", "/deploy", "/remind"]);
        assert_eq!(list[1], SlashCommand { name: "/deploy".into(), usage: "app".into(), desc: "Ship it".into() });
        assert_eq!(list[0].usage, "", "null usage");
        assert!(SlashCommand::from_list(&serde_json::json!({"ok": false})).is_empty());
    }

    #[test]
    fn channel_names() {
        assert_eq!(super::channel_name("#Team Updates!"), "team-updates");
        assert_eq!(super::channel_name("  a  --  b_c "), "a-b_c");
        assert_eq!(super::channel_name("Café"), "café");
        assert_eq!(super::channel_name("x".repeat(100).as_str()).len(), 80);
    }

    /// A live huddle object as Flannel's `huddles/list` and `huddles/info` both send it (shape seen 2026-10-21).
    fn live_object(channel: &str, call: &str, active: &[&str], dropped: &[&str]) -> serde_json::Value {
        serde_json::json!({"active_members": active, "background_id": "B0000000001", "call_id": call, "channel_id": channel,
            "created_by": "U0000000001", "dropped_members": dropped, "expiration": 1_800_000_000, "is_private": false, "is_scheduled": false,
            "knocks": {}, "last_invite_status_by_user": {}, "locale": "en-US", "name": "", "pending_invitees": {},
            "recording": {"can_record_summary": "available", "is_paused": false, "recording_user": "", "summary": false, "transcript": false},
            "start_date": 1_700_000_000, "thread_root_ts": "1700000000.000100", "updated": 1_700_000_010})
    }

    #[test]
    fn parses_huddles_info() {
        use super::LiveHuddle;
        let v = serde_json::json!({"ok": true, "huddles": [
            live_object("C0000000001", "R0000000001", &["U0000000001", "U0000000002", "U0000000003"], &["U0000000002"]),
            live_object("D0000000001", "R0000000002", &["U0000000004"], &[])]});
        assert_eq!(LiveHuddle::parse(&v), vec![
            LiveHuddle { channel: "C0000000001".into(), call_id: "R0000000001".into(), members: vec!["U0000000001".into(), "U0000000003".into()], start: 1_700_000_000, thread_ts: Some("1700000000.000100".into()) },
            LiveHuddle { channel: "D0000000001".into(), call_id: "R0000000002".into(), members: vec!["U0000000004".into()], start: 1_700_000_000, thread_ts: Some("1700000000.000100".into()) },
        ], "members are the active ones less the dropped ones; when it started, its thread");
        // Slack writes zeros for a thread not posted yet.
        let mut early = live_object("C0000000001", "R0000000001", &["U0000000001"], &[]);
        early["thread_root_ts"] = "0000000000.000000".into();
        assert_eq!(LiveHuddle::parse(&serde_json::json!({"huddles": [early]}))[0].thread_ts, None);
        // `huddles/list` sends the same objects: read as they are, each channel once (no `huddles/info` for them).
        let list = serde_json::json!({"ok": true, "huddles": [live_object("C0000000001", "R0000000001", &["U0000000001"], &[]), live_object("C0000000001", "R0000000001", &["U0000000001"], &[]), live_object("D0000000001", "R0000000002", &["U0000000004"], &[])]});
        let live = LiveHuddle::parse(&list);
        assert_eq!(live.iter().map(|h| (h.channel.as_str(), h.members.len())).collect::<Vec<_>>(), [("C0000000001", 1), ("D0000000001", 1)]);
        assert_eq!(LiveHuddle::listed_channels(&list), ["C0000000001", "D0000000001"], "(for `klack ctl huddles --shape`)");
        // An object without a channel or a room says nothing usable.
        let odd = serde_json::json!({"ok": true, "huddles": [{"channel_id": "C0000000001"}, {"call_id": "R0000000001", "active_members": ["U0000000001"]}]});
        assert!(LiveHuddle::parse(&odd).is_empty());
    }

    #[test]
    fn parses_huddles_history() {
        use super::PastHuddle;
        let item = |id: &str, ended: bool| serde_json::json!({"access_type_by_user": {"U0000000001": 1}, "attached_file_ids": [], "background_id": "B0000000001",
            "channel_type": "public", "channels": ["C0000000001"], "created_by": "U0000000001", "date_end": 1_700_000_600, "date_start": 1_700_000_000,
            "external_unique_id": "00000000-0000-0000-0000-000000000000", "has_ended": ended, "huddle_link": "https://example.slack.com/huddle/T0/C0", "id": id,
            "last_invite_status_by_user": {}, "name": "", "participant_history": ["U0000000001", "U0000000002"], "participants": [], "pending_invitees": {},
            "prototypes": {"is_prewarmed": false, "is_scheduled": false}, "recording": {"can_record_summary": "available"}, "thread_root_ts": "1700000000.000100"});
        let mut missed = item("R0000000002", true);
        missed["channel_type"] = "dm".into();
        missed["channels"] = serde_json::json!(["D0000000001"]);
        missed["participant_history"] = serde_json::json!(["U0000000002"]);
        missed["missed_participants"] = serde_json::json!({"U0000000003": "M0000000001", "U0000000001": "M0000000002"});
        missed["thread_root_ts"] = "".into();
        let v = serde_json::json!({"ok": true, "huddles": [item("R0000000001", false), missed], "response_metadata": {"next_cursor": "bmV4dA=="}});
        let (huddles, next) = PastHuddle::from_history(&v);
        assert_eq!(next.as_deref(), Some("bmV4dA=="));
        assert_eq!(huddles, vec![
            PastHuddle { id: "R0000000001".into(), channel: "C0000000001".into(), channel_type: "public".into(), start: 1_700_000_000, end: 1_700_000_600, ended: false,
                thread_ts: Some("1700000000.000100".into()), people: vec!["U0000000001".into(), "U0000000002".into()], missed: vec![] },
            PastHuddle { id: "R0000000002".into(), channel: "D0000000001".into(), channel_type: "dm".into(), start: 1_700_000_000, end: 1_700_000_600, ended: true,
                thread_ts: None, people: vec!["U0000000002".into()], missed: vec!["U0000000001".into(), "U0000000003".into()] },
        ], "missed: the keys of missed_participants, sorted");
    }

    #[test]
    fn empty_lists() {
        use super::{LiveHuddle, PastHuddle};
        let none = serde_json::json!({"ok": true, "huddles": []});
        assert!(LiveHuddle::listed_channels(&none).is_empty() && LiveHuddle::parse(&none).is_empty());
        assert_eq!(PastHuddle::from_history(&none), (vec![], None));
        // The last page's cursor is empty.
        let last = serde_json::json!({"ok": true, "huddles": [], "response_metadata": {"next_cursor": ""}});
        assert_eq!(PastHuddle::from_history(&last), (vec![], None));
        assert!(LiveHuddle::parse(&serde_json::json!({"ok": true})).is_empty(), "no huddles key");
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UserProfile {
    pub display_name: String,
    pub real_name: String,
    pub status_text: String,
    pub status_emoji: String,
    pub image_48: Option<String>,
    pub image_192: Option<String>,
    /// The big pictures, for the profile card (a default picture has no original).
    pub image_512: Option<String>,
    pub image_original: Option<String>,
    pub email: Option<String>,
    pub title: String,
    pub pronouns: Option<String>,
    /// A bot user's app (`A…`): messages people send through that app name it.
    pub api_app_id: Option<String>,
    /// When the status clears (epoch seconds, 0 = never).
    pub status_expiration: Option<i64>,
    /// `in_a_huddle` while the person is in one (else empty).
    #[serde(default, deserialize_with = "null_as_default")]
    pub huddle_state: String,
    /// Until when `huddle_state` holds (epoch seconds; Slack may keep the fraction a renewal sent).
    #[serde(default, deserialize_with = "whole_seconds")]
    pub huddle_state_expiration_ts: i64,
}

/// Epoch seconds sent as a whole or a fractional number (or null: 0), to whole seconds.
fn whole_seconds<'de, D: serde::Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    Ok(Option::<f64>::deserialize(d)?.map_or(0, |s| s.floor() as i64))
}

/// A field Slack may send as null: its default then.
fn null_as_default<'de, D: serde::Deserializer<'de>, T: Default + Deserialize<'de>>(d: D) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct User {
    pub id: String,
    pub name: String,
    pub real_name: Option<String>,
    pub deleted: bool,
    pub is_bot: bool,
    pub is_app_user: bool,
    pub tz: Option<String>,
    pub tz_label: Option<String>,
    /// Seconds east of UTC, for the person's local time.
    pub tz_offset: Option<i64>,
    pub profile: UserProfile,
}

impl User {
    /// What Slack shows: display name, else real name, else handle.
    pub fn display(&self) -> &str {
        if !self.profile.display_name.is_empty() {
            &self.profile.display_name
        } else if !self.profile.real_name.is_empty() {
            &self.profile.real_name
        } else if let Some(r) = self.real_name.as_deref().filter(|r| !r.is_empty()) {
            r
        } else {
            &self.name
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Reaction {
    pub name: String,
    pub users: Vec<String>,
    pub count: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Edited {
    pub user: String,
    pub ts: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct File {
    pub id: String,
    pub name: String,
    pub title: String,
    pub mimetype: String,
    pub filetype: String,
    pub pretty_type: String,
    pub size: u64,
    pub url_private: Option<String>,
    pub url_private_download: Option<String>,
    pub permalink: Option<String>,
    pub thumb_360: Option<String>,
    pub thumb_480: Option<String>,
    pub thumb_720: Option<String>,
    pub thumb_1024: Option<String>,
    pub thumb_video: Option<String>,
    pub original_w: Option<u64>,
    pub original_h: Option<u64>,
    /// Transcoded video for video files.
    pub mp4: Option<String>,
    pub duration_ms: Option<u64>,
    pub mode: String,
    /// First lines of a text snippet.
    pub preview: Option<String>,
    pub lines: Option<u64>,
    pub lines_more: Option<u64>,
}

impl File {
    pub fn is_snippet(&self) -> bool {
        self.mode == "snippet"
    }

    pub fn is_image(&self) -> bool {
        self.mimetype.starts_with("image/")
    }
    pub fn is_video(&self) -> bool {
        self.mimetype.starts_with("video/")
    }
    pub fn is_audio(&self) -> bool {
        self.mimetype.starts_with("audio/")
    }
    /// Best image URL to preview at roughly `width` pixels.
    pub fn preview_url(&self, width: u32) -> Option<&str> {
        let candidates = if width <= 360 {
            [&self.thumb_360, &self.thumb_480, &self.thumb_720, &self.thumb_1024]
        } else if width <= 480 {
            [&self.thumb_480, &self.thumb_720, &self.thumb_360, &self.thumb_1024]
        } else {
            [&self.thumb_720, &self.thumb_1024, &self.thumb_480, &self.thumb_360]
        };
        candidates.iter().find_map(|c| c.as_deref()).or(if self.is_image() { self.url_private.as_deref() } else { None })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Message {
    #[serde(rename = "type")]
    pub kind: String,
    pub subtype: Option<String>,
    pub ts: String,
    pub user: Option<String>,
    pub bot_id: Option<String>,
    /// The app a person sent this through (Slack shows "Sent using @App").
    pub app_id: Option<String>,
    pub username: Option<String>,
    pub text: String,
    pub thread_ts: Option<String>,
    pub reply_count: u64,
    pub reply_users: Vec<String>,
    pub latest_reply: Option<String>,
    pub edited: Option<Edited>,
    pub reactions: Vec<Reaction>,
    pub files: Vec<File>,
    pub attachments: Vec<Value>,
    pub blocks: Vec<Value>,
    pub channel: Option<String>,
    pub client_msg_id: Option<String>,
    pub bot_profile: Option<Value>,
    /// A bot or webhook message's own icon (`image_48`, …), when it was posted with one.
    pub icons: Option<Value>,
    /// Shown to you alone (a command's answer); gone after a reload.
    pub is_ephemeral: bool,
    pub hidden: bool,
    /// Conversations this message is pinned in.
    pub pinned_to: Vec<String>,
}

impl Message {
    /// A thread parent (has replies) as opposed to a reply.
    pub fn is_thread_parent(&self) -> bool {
        self.reply_count > 0 || self.thread_ts.as_deref() == Some(&self.ts)
    }
    pub fn is_reply(&self) -> bool {
        self.thread_ts.as_deref().is_some_and(|t| t != self.ts)
    }
    /// Seconds since the epoch.
    pub fn time(&self) -> f64 {
        self.ts.parse().unwrap_or(0.0)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ResponseMetadata {
    pub next_cursor: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct HistoryPage {
    pub messages: Vec<Message>,
    pub has_more: bool,
    pub response_metadata: ResponseMetadata,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AuthTest {
    pub url: String,
    pub team: String,
    pub user: String,
    pub team_id: String,
    pub user_id: String,
}

/// A huddle happening now (Flannel `huddles/list`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveHuddle {
    pub channel: String,
    /// The room (`R…`).
    pub call_id: String,
    /// Who is in it now: the active members less the dropped ones, as Slack's client counts them.
    pub members: Vec<String>,
    /// When it started (unix seconds; 0: not said).
    pub start: i64,
    /// Its thread (the "A huddle started" message), once Slack has posted it.
    pub thread_ts: Option<String>,
}

impl LiveHuddle {
    /// The channels with a huddle in a `huddles/list` answer (for `klack ctl huddles --shape`, which shows
    /// `huddles/info`'s answer for them too).
    pub fn listed_channels(v: &Value) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for h in v["huddles"].as_array().into_iter().flatten() {
            if let Some(c) = h["channel_id"].as_str().or(h.as_str()).filter(|c| !c.is_empty() && !out.iter().any(|o| o == c)) {
                out.push(c.to_string());
            }
        }
        out
    }

    /// The huddles in a `huddles/list` or `huddles/info` answer (the same objects), each channel once; a channel
    /// without one has no object there.
    pub fn parse(v: &Value) -> Vec<LiveHuddle> {
        let ids = |h: &Value, k: &str| -> Vec<String> { h[k].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect() };
        let mut seen = std::collections::HashSet::new();
        v["huddles"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|h| {
                let text = |k: &str| h[k].as_str().filter(|s| !s.is_empty()).map(String::from);
                let dropped = ids(h, "dropped_members");
                let members = ids(h, "active_members").into_iter().filter(|m| !dropped.contains(m)).collect();
                // (Zeros until the thread's message exists.)
                let thread_ts = text("thread_root_ts").filter(|t| t.trim_start_matches(['0', '.']).len() > 0);
                Some(LiveHuddle { channel: text("channel_id")?, call_id: text("call_id")?, members, start: h["start_date"].as_i64().unwrap_or_default(), thread_ts })
            })
            .filter(|h| seen.insert(h.channel.clone()))
            .collect()
    }
}

/// A huddle from `huddles.history`: one this account was in or missed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PastHuddle {
    /// The room (`R…`).
    pub id: String,
    pub channel: String,
    /// `public`, `dm`, `mpim`, … as Slack names it.
    pub channel_type: String,
    /// Unix seconds.
    pub start: i64,
    pub end: i64,
    pub ended: bool,
    /// The "A huddle started" message (the huddle's thread).
    pub thread_ts: Option<String>,
    /// Everyone who was in it.
    pub people: Vec<String>,
    /// Who was asked or rang and did not come.
    pub missed: Vec<String>,
}

impl PastHuddle {
    /// The huddles of a `huddles.history` page and the next page's cursor.
    pub fn from_history(v: &Value) -> (Vec<PastHuddle>, Option<String>) {
        let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
        let huddles = v["huddles"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|h| {
                let mut missed: Vec<String> = h["missed_participants"].as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default();
                missed.sort();
                PastHuddle {
                    id: text(&h["id"]),
                    channel: text(&h["channels"][0]),
                    channel_type: text(&h["channel_type"]),
                    start: h["date_start"].as_i64().unwrap_or_default(),
                    end: h["date_end"].as_i64().unwrap_or_default(),
                    ended: h["has_ended"].as_bool().unwrap_or_default(),
                    thread_ts: h["thread_root_ts"].as_str().filter(|t| !t.is_empty()).map(String::from),
                    people: h["participant_history"].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect(),
                    missed,
                }
            })
            .collect();
        let next = v["response_metadata"]["next_cursor"].as_str().filter(|c| !c.is_empty()).map(String::from);
        (huddles, next)
    }
}

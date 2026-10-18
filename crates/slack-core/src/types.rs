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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UserProfile {
    pub display_name: String,
    pub real_name: String,
    pub status_text: String,
    pub status_emoji: String,
    pub image_48: Option<String>,
    pub image_192: Option<String>,
    pub email: Option<String>,
    pub title: String,
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
}

impl File {
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

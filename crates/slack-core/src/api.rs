//! Slack Web API client authenticated like the web client (xoxc token + `d` cookie).

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::session::{USER_AGENT, WorkspaceSession};
use crate::types::*;

#[derive(Debug, thiserror::Error)]
#[error("{method}: {error}")]
pub struct ApiError {
    pub method: String,
    pub error: String,
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    pub session: WorkspaceSession,
}

pub type Params<'a> = Vec<(&'a str, String)>;

impl Client {
    pub fn new(session: WorkspaceSession) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(120))
            .build()?;
        Ok(Client { http, session })
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// Call `method` and return the raw JSON (after the `ok` check). Retries on HTTP 429
    /// honouring `Retry-After`.
    pub async fn call_raw(&self, method: &str, params: &[(&str, String)]) -> Result<Value> {
        let url = format!("{}{}", self.session.api_base(), method);
        let mut form: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
        form.push(("token", self.session.token.as_str()));
        for attempt in 0..5 {
            let resp = self
                .http
                .post(&url)
                .header(reqwest::header::COOKIE, self.session.cookie_header())
                .form(&form)
                .send()
                .await
                .with_context(|| format!("{method}: request failed"))?;
            if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let wait = resp
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(1 + attempt);
                tracing::warn!("{method}: rate limited, retrying in {wait}s");
                tokio::time::sleep(Duration::from_secs(wait)).await;
                continue;
            }
            let body: Value = resp.json().await.with_context(|| format!("{method}: invalid JSON"))?;
            if body["ok"].as_bool() == Some(true) {
                return Ok(body);
            }
            return Err(ApiError { method: method.to_string(), error: body["error"].as_str().unwrap_or("unknown_error").to_string() }.into());
        }
        Err(anyhow!("{method}: still rate limited after retries"))
    }

    pub async fn call<T: DeserializeOwned>(&self, method: &str, params: &[(&str, String)]) -> Result<T> {
        let v = self.call_raw(method, params).await?;
        serde_json::from_value(v).with_context(|| format!("{method}: unexpected response shape"))
    }

    /// Like `call` but extracting one field.
    pub async fn call_field<T: DeserializeOwned>(&self, method: &str, field: &str, params: &[(&str, String)]) -> Result<T> {
        let mut v = self.call_raw(method, params).await?;
        serde_json::from_value(v[field].take()).with_context(|| format!("{method}: unexpected `{field}`"))
    }

    // ---- auth / boot ---------------------------------------------------------------

    pub async fn auth_test(&self) -> Result<AuthTest> {
        self.call("auth.test", &[]).await
    }

    /// The web client's boot payload: channels, IMs, prefs, DND.
    pub async fn user_boot(&self) -> Result<Value> {
        self.call_raw("client.userBoot", &[("_x_reason", "initial-data".into())]).await
    }

    /// Unread state for every conversation (`client.counts`).
    pub async fn counts(&self) -> Result<Value> {
        self.call_raw("client.counts", &[("thread_counts_by_channel", "true".into()), ("org_wide_aware", "true".into())]).await
    }

    // ---- conversations --------------------------------------------------------------

    pub async fn conversations_list(&self) -> Result<Vec<Conversation>> {
        let mut out = Vec::new();
        let mut cursor = String::new();
        loop {
            let mut params: Params = vec![
                ("types", "public_channel,private_channel,mpim,im".into()),
                ("exclude_archived", "true".into()),
                ("limit", "999".into()),
            ];
            if !cursor.is_empty() {
                params.push(("cursor", cursor.clone()));
            }
            let mut v = self.call_raw("conversations.list", &params).await?;
            let page: Vec<Conversation> = serde_json::from_value(v["channels"].take())?;
            out.extend(page);
            cursor = v["response_metadata"]["next_cursor"].as_str().unwrap_or_default().to_string();
            if cursor.is_empty() {
                return Ok(out);
            }
        }
    }

    pub async fn conversation_info(&self, channel: &str) -> Result<Conversation> {
        self.call_field("conversations.info", "channel", &[("channel", channel.into())]).await
    }

    pub async fn history(&self, channel: &str, latest: Option<&str>, limit: u32) -> Result<HistoryPage> {
        let mut params: Params = vec![("channel", channel.into()), ("limit", limit.to_string()), ("inclusive", "false".into())];
        if let Some(l) = latest {
            params.push(("latest", l.into()));
        }
        self.call("conversations.history", &params).await
    }

    pub async fn replies(&self, channel: &str, thread_ts: &str) -> Result<HistoryPage> {
        let mut all = HistoryPage::default();
        let mut cursor = String::new();
        loop {
            let mut params: Params = vec![("channel", channel.into()), ("ts", thread_ts.into()), ("limit", "200".into())];
            if !cursor.is_empty() {
                params.push(("cursor", cursor.clone()));
            }
            let page: HistoryPage = self.call("conversations.replies", &params).await?;
            all.messages.extend(page.messages);
            cursor = page.response_metadata.next_cursor;
            if cursor.is_empty() {
                return Ok(all);
            }
        }
    }

    pub async fn mark(&self, channel: &str, ts: &str) -> Result<()> {
        self.call_raw("conversations.mark", &[("channel", channel.into()), ("ts", ts.into())]).await.map(|_| ())
    }

    pub async fn open_dm(&self, users: &[&str]) -> Result<Conversation> {
        self.call_field("conversations.open", "channel", &[("users", users.join(",")), ("return_im", "true".into())]).await
    }

    pub async fn members(&self, channel: &str) -> Result<Vec<String>> {
        self.call_field("conversations.members", "members", &[("channel", channel.into()), ("limit", "1000".into())]).await
    }

    // ---- users ------------------------------------------------------------------------

    pub async fn users_list(&self) -> Result<Vec<User>> {
        let mut out = Vec::new();
        let mut cursor = String::new();
        loop {
            let mut params: Params = vec![("limit", "500".into())];
            if !cursor.is_empty() {
                params.push(("cursor", cursor.clone()));
            }
            let mut v = self.call_raw("users.list", &params).await?;
            let page: Vec<User> = serde_json::from_value(v["members"].take())?;
            out.extend(page);
            cursor = v["response_metadata"]["next_cursor"].as_str().unwrap_or_default().to_string();
            if cursor.is_empty() {
                return Ok(out);
            }
        }
    }

    pub async fn user_info(&self, user: &str) -> Result<User> {
        self.call_field("users.info", "user", &[("user", user.into())]).await
    }

    pub async fn set_presence(&self, away: bool) -> Result<()> {
        self.call_raw("users.setPresence", &[("presence", if away { "away" } else { "auto" }.into())]).await.map(|_| ())
    }

    // ---- messages -----------------------------------------------------------------------

    /// Post mrkdwn text (Slack renders formatting, mentions written as `<@U…>`).
    pub async fn post_message(&self, channel: &str, text: &str, thread_ts: Option<&str>, broadcast: bool) -> Result<Message> {
        let mut params: Params = vec![("channel", channel.into()), ("text", text.into()), ("mrkdwn", "true".into())];
        if let Some(t) = thread_ts {
            params.push(("thread_ts", t.into()));
            if broadcast {
                params.push(("reply_broadcast", "true".into()));
            }
        }
        let mut v = self.call_raw("chat.postMessage", &params).await?;
        let mut msg: Message = serde_json::from_value(v["message"].take())?;
        msg.channel = v["channel"].as_str().map(String::from);
        Ok(msg)
    }

    pub async fn update_message(&self, channel: &str, ts: &str, text: &str) -> Result<()> {
        self.call_raw("chat.update", &[("channel", channel.into()), ("ts", ts.into()), ("text", text.into())]).await.map(|_| ())
    }

    pub async fn delete_message(&self, channel: &str, ts: &str) -> Result<()> {
        self.call_raw("chat.delete", &[("channel", channel.into()), ("ts", ts.into())]).await.map(|_| ())
    }

    pub async fn react(&self, channel: &str, ts: &str, name: &str, add: bool) -> Result<()> {
        let method = if add { "reactions.add" } else { "reactions.remove" };
        self.call_raw(method, &[("channel", channel.into()), ("timestamp", ts.into()), ("name", name.into())]).await.map(|_| ())
    }

    pub async fn permalink(&self, channel: &str, ts: &str) -> Result<String> {
        self.call_field("chat.getPermalink", "permalink", &[("channel", channel.into()), ("message_ts", ts.into())]).await
    }

    pub async fn pin(&self, channel: &str, ts: &str, add: bool) -> Result<()> {
        let method = if add { "pins.add" } else { "pins.remove" };
        self.call_raw(method, &[("channel", channel.into()), ("timestamp", ts.into())]).await.map(|_| ())
    }

    pub async fn pins(&self, channel: &str) -> Result<Value> {
        self.call_raw("pins.list", &[("channel", channel.into())]).await
    }

    pub async fn search_messages(&self, query: &str, count: u32) -> Result<Value> {
        self.call_raw("search.messages", &[("query", query.into()), ("count", count.to_string()), ("highlight", "false".into())]).await
    }

    pub async fn emoji_list(&self) -> Result<std::collections::HashMap<String, String>> {
        self.call_field("emoji.list", "emoji", &[]).await
    }

    // ---- files -----------------------------------------------------------------------------

    /// Download a private file URL (needs the session cookie + token).
    pub async fn download(&self, url: &str) -> Result<Vec<u8>> {
        let resp = self
            .http
            .get(url)
            .header(reqwest::header::COOKIE, self.session.cookie_header())
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {}", self.session.token))
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.bytes().await?.to_vec())
    }

    /// Upload a file to a conversation (new external upload flow).
    pub async fn upload(&self, channel: &str, filename: &str, bytes: Vec<u8>, comment: Option<&str>, thread_ts: Option<&str>) -> Result<String> {
        let len = bytes.len();
        let v = self.call_raw("files.getUploadURLExternal", &[("filename", filename.into()), ("length", len.to_string())]).await?;
        let upload_url = v["upload_url"].as_str().ok_or_else(|| anyhow!("no upload_url"))?.to_string();
        let file_id = v["file_id"].as_str().ok_or_else(|| anyhow!("no file_id"))?.to_string();
        let part = reqwest::multipart::Part::bytes(bytes).file_name(filename.to_string());
        self.http
            .post(&upload_url)
            .header(reqwest::header::COOKIE, self.session.cookie_header())
            .multipart(reqwest::multipart::Form::new().part("file", part))
            .send()
            .await?
            .error_for_status()?;
        let files = serde_json::json!([{"id": file_id, "title": filename}]).to_string();
        let mut params: Params = vec![("files", files), ("channel_id", channel.into())];
        if let Some(c) = comment.filter(|c| !c.is_empty()) {
            params.push(("initial_comment", c.into()));
        }
        if let Some(t) = thread_ts {
            params.push(("thread_ts", t.into()));
        }
        self.call_raw("files.completeUploadExternal", &params).await?;
        Ok(file_id)
    }

    // ---- real time ------------------------------------------------------------------------

    /// `rtm.connect` → websocket URL for live events.
    pub async fn rtm_url(&self) -> Result<String> {
        self.call_field("rtm.connect", "url", &[("batch_presence_aware", "1".into()), ("presence_sub", "true".into())]).await
    }
}

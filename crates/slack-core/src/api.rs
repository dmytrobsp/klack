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

/// An HTTP client checking certificates against the system's store or — where the system has
/// none (a minimal container without ca-certificates) — against Mozilla's roots bundled in klack.
pub fn http_client(builder: impl Fn() -> reqwest::ClientBuilder) -> Result<reqwest::Client> {
    match builder().build() {
        Ok(client) => Ok(client),
        Err(e) => {
            tracing::warn!("no usable system certificates ({e:#}); using the bundled Mozilla roots");
            let roots = webpki_root_certs::TLS_SERVER_ROOT_CERTS.iter().filter_map(|c| reqwest::Certificate::from_der(c).ok());
            Ok(builder().tls_certs_only(roots).build()?)
        }
    }
}

impl Client {
    pub fn new(session: WorkspaceSession) -> Result<Self> {
        let http = http_client(|| reqwest::Client::builder().user_agent(USER_AGENT).connect_timeout(Duration::from_secs(15)).timeout(Duration::from_secs(120)))?;
        Ok(Client { http, session })
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// Call `method` and return the raw JSON (after the `ok` check). Retries on HTTP 429
    /// honouring `Retry-After`.
    pub async fn call_raw(&self, method: &str, params: &[(&str, String)]) -> Result<Value> {
        // Debug builds only: `KLACK_FAIL=chat.delete,chat.update` makes those calls fail, to
        // test what the app does when Slack refuses.
        #[cfg(debug_assertions)]
        if std::env::var("KLACK_FAIL").is_ok_and(|v| v.split(',').any(|m| m.trim() == method)) {
            return Err(anyhow!("{method}: injected_failure (KLACK_FAIL)"));
        }
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

    /// Every conversation of `types` (e.g. `public_channel,im`) that is not archived. Public
    /// channels come back whether or not we are a member.
    pub async fn conversations_list(&self, types: &str) -> Result<Vec<Conversation>> {
        let mut out = Vec::new();
        let mut cursor = String::new();
        loop {
            let mut params: Params = vec![
                ("types", types.into()),
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

    /// Mark thread `thread_ts` in `channel` read up to its reply `ts` (Slack's Threads view and
    /// unread counts follow).
    pub async fn mark_thread(&self, channel: &str, thread_ts: &str, ts: &str) -> Result<()> {
        let params = [("channel", channel.into()), ("thread_ts", thread_ts.into()), ("ts", ts.into()), ("read", "1".into())];
        self.call_raw("subscriptions.thread.mark", &params).await.map(|_| ())
    }

    /// Run slash command `command` (with its `/`) in `channel` as Slack's clients do; Slack's
    /// answer, if any, comes as an ephemeral message.
    pub async fn slash_command(&self, channel: &str, command: &str, text: &str, thread_ts: Option<&str>) -> Result<()> {
        let mut params: Params = vec![("channel", channel.into()), ("command", command.into()), ("text", text.into())];
        if let Some(ts) = thread_ts {
            params.push(("thread_ts", ts.into()));
        }
        self.call_raw("chat.command", &params).await.map(|_| ())
    }

    pub async fn open_dm(&self, users: &[&str]) -> Result<Conversation> {
        self.call_field("conversations.open", "channel", &[("users", users.join(",")), ("return_im", "true".into())]).await
    }

    pub async fn join(&self, channel: &str) -> Result<Conversation> {
        self.call_field("conversations.join", "channel", &[("channel", channel.into())]).await
    }

    pub async fn leave(&self, channel: &str) -> Result<()> {
        self.call_raw("conversations.leave", &[("channel", channel.into())]).await.map(|_| ())
    }

    /// Create a channel; Slack expects lowercase names without spaces (see `types::channel_name`).
    pub async fn create_channel(&self, name: &str, private: bool) -> Result<Conversation> {
        self.call_field("conversations.create", "channel", &[("name", name.into()), ("is_private", private.to_string())]).await
    }

    pub async fn members(&self, channel: &str) -> Result<Vec<String>> {
        self.call_field("conversations.members", "members", &[("channel", channel.into()), ("limit", "1000".into())]).await
    }

    // ---- sidebar: stars and sections (web client methods, see docs/protocol/sections.md) -----

    pub async fn star_channel(&self, channel: &str, add: bool) -> Result<()> {
        let method = if add { "stars.add" } else { "stars.remove" };
        self.call_raw(method, &[("channel", channel.into())]).await.map(|_| ())
    }

    pub async fn channel_sections(&self) -> Result<Value> {
        self.call_raw("users.channelSections.list", &[]).await
    }

    /// Create a section (appended at the end); returns its id.
    pub async fn create_section(&self, name: &str) -> Result<String> {
        self.call_field("users.channelSections.create", "channel_section_id", &[("name", name.into()), ("emoji", String::new())]).await
    }

    pub async fn rename_section(&self, id: &str, name: &str) -> Result<()> {
        self.call_raw("users.channelSections.set", &[("channel_section_id", id.into()), ("name", name.into()), ("emoji", String::new())]).await.map(|_| ())
    }

    pub async fn delete_section(&self, id: &str) -> Result<()> {
        self.call_raw("users.channelSections.delete", &[("channel_section_id", id.into())]).await.map(|_| ())
    }

    /// Move `channel` out of section `from` and/or into section `to` in one call.
    pub async fn move_to_section(&self, channel: &str, from: Option<&str>, to: Option<&str>) -> Result<()> {
        let item = |s: Option<&str>| serde_json::json!(s.map(|s| vec![serde_json::json!({"channel_section_id": s, "channel_ids": [channel]})]).unwrap_or_default()).to_string();
        self.call_raw("users.channelSections.channels.bulkUpdate", &[("insert", item(to)), ("remove", item(from))]).await.map(|_| ())
    }

    // ---- activity (web client method, see docs/protocol/activity.md) ----------------------

    /// The activity feed, newest first: mentions, reactions to your messages, threads.
    pub async fn activity_feed(&self, limit: u32) -> Result<Value> {
        let types = "at_user,at_user_group,at_channel,at_everyone,keyword,message_reaction,thread_v2";
        self.call_raw("activity.feed", &[("mode", "chrono_reads_and_unreads".into()), ("types", types.into()), ("limit", limit.to_string())]).await
    }

    /// The threads you are in (the client's Threads view): unread ones first, then by latest
    /// reply; with `before`, the page of threads active before that time.
    pub async fn thread_view(&self, before: Option<&str>, limit: u32) -> Result<Value> {
        let mut params: Params = vec![("limit", limit.to_string())];
        if let Some(ts) = before {
            params.push(("current_ts", ts.into()));
        }
        self.call_raw("subscriptions.thread.getView", &params).await
    }

    /// One message by its ts, whether it is top level or a thread reply.
    pub async fn message_at(&self, channel: &str, ts: &str) -> Result<Option<Message>> {
        let params: Params = vec![("channel", channel.into()), ("ts", ts.into()), ("latest", ts.into()), ("oldest", ts.into()), ("inclusive", "true".into()), ("limit", "1".into())];
        let page: HistoryPage = self.call("conversations.replies", &params).await?;
        Ok(page.messages.into_iter().find(|m| m.ts == ts))
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

    /// Set your status; `expiration` is epoch seconds (0 = never). Empty values clear it.
    pub async fn set_status(&self, emoji: &str, text: &str, expiration: i64) -> Result<()> {
        let profile = serde_json::json!({"status_emoji": emoji, "status_text": text, "status_expiration": expiration}).to_string();
        self.call_raw("users.profile.set", &[("profile", profile)]).await.map(|_| ())
    }

    /// Pause notifications for `minutes`, or resume them (None).
    pub async fn snooze(&self, minutes: Option<u64>) -> Result<()> {
        match minutes {
            Some(m) => self.call_raw("dnd.setSnooze", &[("num_minutes", m.to_string())]).await.map(|_| ()),
            None => self.call_raw("dnd.endSnooze", &[]).await.map(|_| ()),
        }
    }

    /// `users.prefs.get` (the `prefs` object).
    pub async fn user_prefs(&self) -> Result<Value> {
        self.call_field("users.prefs.get", "prefs", &[]).await
    }

    /// A notification pref: for one conversation (`desktop`, `muted`, …) or global
    /// (`global_desktop`, `global_keywords`, …); see docs/protocol/notifications.md.
    pub async fn set_notification_pref(&self, name: &str, value: &str, channel: Option<&str>) -> Result<()> {
        let mut params: Params = vec![("name", name.into()), ("value", value.into()), ("global", channel.is_none().to_string())];
        if let Some(c) = channel {
            params.push(("channel_id", c.into()));
        }
        self.call_raw("users.prefs.setNotifications", &params).await.map(|_| ())
    }

    pub async fn set_pref(&self, name: &str, value: &str) -> Result<()> {
        self.call_raw("users.prefs.set", &[("name", name.into()), ("value", value.into())]).await.map(|_| ())
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

    /// One page (1-based) of `count` matches; `messages.paging.pages` says how many there are.
    pub async fn search_messages(&self, query: &str, count: u32, page: u32) -> Result<Value> {
        self.call_raw("search.messages", &[("query", query.into()), ("count", count.to_string()), ("page", page.to_string()), ("highlight", "false".into())]).await
    }

    /// The workspace's slash commands — Slack's own and its apps' (`commands.list`).
    pub async fn commands(&self) -> Result<Vec<crate::types::SlashCommand>> {
        Ok(crate::types::SlashCommand::from_list(&self.call_raw("commands.list", &[]).await?))
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

    /// Upload a file to a conversation (new external upload flow). With `snippet_type` (a
    /// language such as `rust` or `text`) the file becomes a text snippet.
    pub async fn upload(&self, channel: &str, filename: &str, bytes: Vec<u8>, comment: Option<&str>, thread_ts: Option<&str>, snippet_type: Option<&str>) -> Result<String> {
        let len = bytes.len() as u64;
        let part = reqwest::multipart::Part::bytes(bytes).file_name(filename.to_string());
        self.upload_part(channel, filename, len, part, comment, thread_ts, snippet_type).await
    }

    /// Upload the file at `path` without reading it into memory, calling `progress(sent, total)`
    /// as it goes.
    pub async fn upload_file(&self, channel: &str, path: &std::path::Path, filename: &str, comment: Option<&str>, thread_ts: Option<&str>, progress: impl Fn(u64, u64) + Send + Sync + 'static) -> Result<String> {
        let item = Upload { name: filename.to_string(), source: UploadSource::Path(path.to_path_buf()) };
        let ids = self.upload_files(channel, vec![item], comment, thread_ts, progress).await?;
        Ok(ids.into_iter().next().unwrap_or_default())
    }

    /// Upload several files and share them as one message with `comment` as its text (Slack
    /// takes up to 10), calling `progress(sent, total)` over all of them as they go. Files on
    /// disk are streamed, not read into memory. Returns the file ids.
    pub async fn upload_files(&self, channel: &str, items: Vec<Upload>, comment: Option<&str>, thread_ts: Option<&str>, progress: impl Fn(u64, u64) + Send + Sync + 'static) -> Result<Vec<String>> {
        let mut sized = Vec::with_capacity(items.len());
        for item in items {
            let len = match &item.source {
                UploadSource::Path(path) => tokio::fs::metadata(path).await.map_err(|e| anyhow!("{}: {e}", path.display()))?.len(),
                UploadSource::Bytes(bytes) => bytes.len() as u64,
            };
            sized.push((item, len));
        }
        let total: u64 = sized.iter().map(|(_, len)| len).sum();
        let progress = std::sync::Arc::new(progress);
        let mut files = Vec::with_capacity(sized.len());
        let mut done = 0u64;
        for (item, len) in sized {
            let part = match item.source {
                UploadSource::Path(path) => {
                    let file = tokio::fs::File::open(&path).await.map_err(|e| anyhow!("{}: {e}", path.display()))?;
                    reqwest::multipart::Part::stream_with_length(reqwest::Body::wrap_stream(read_chunks(file, done, total, progress.clone())), len)
                }
                UploadSource::Bytes(bytes) => {
                    let part = reqwest::multipart::Part::bytes(bytes);
                    progress(done + len, total);
                    part
                }
            };
            let id = self.send_part(&item.name, len, part.file_name(item.name.clone()), None).await?;
            files.push((id, item.name));
            done += len;
        }
        self.complete_upload(channel, &files, comment, thread_ts).await?;
        Ok(files.into_iter().map(|(id, _)| id).collect())
    }

    #[allow(clippy::too_many_arguments)]
    async fn upload_part(&self, channel: &str, filename: &str, len: u64, part: reqwest::multipart::Part, comment: Option<&str>, thread_ts: Option<&str>, snippet_type: Option<&str>) -> Result<String> {
        let file_id = self.send_part(filename, len, part, snippet_type).await?;
        self.complete_upload(channel, &[(file_id.clone(), filename.to_string())], comment, thread_ts).await?;
        Ok(file_id)
    }

    /// Hand one file's bytes to Slack; returns its id (shared by `complete_upload`).
    async fn send_part(&self, filename: &str, len: u64, part: reqwest::multipart::Part, snippet_type: Option<&str>) -> Result<String> {
        let mut params: Params = vec![("filename", filename.into()), ("length", len.to_string())];
        if let Some(t) = snippet_type {
            params.push(("snippet_type", t.into()));
        }
        let v = self.call_raw("files.getUploadURLExternal", &params).await?;
        let upload_url = v["upload_url"].as_str().ok_or_else(|| anyhow!("no upload_url"))?.to_string();
        let file_id = v["file_id"].as_str().ok_or_else(|| anyhow!("no file_id"))?.to_string();
        self.http
            .post(&upload_url)
            .header(reqwest::header::COOKIE, self.session.cookie_header())
            .multipart(reqwest::multipart::Form::new().part("file", part))
            .send()
            .await?
            .error_for_status()?;
        Ok(file_id)
    }

    /// Share uploaded files `(id, title)` in `channel` as one message.
    async fn complete_upload(&self, channel: &str, files: &[(String, String)], comment: Option<&str>, thread_ts: Option<&str>) -> Result<()> {
        let files = serde_json::Value::Array(files.iter().map(|(id, title)| serde_json::json!({"id": id, "title": title})).collect()).to_string();
        let mut params: Params = vec![("files", files), ("channel_id", channel.into())];
        if let Some(c) = comment.filter(|c| !c.is_empty()) {
            params.push(("initial_comment", c.into()));
        }
        if let Some(t) = thread_ts {
            params.push(("thread_ts", t.into()));
        }
        self.call_raw("files.completeUploadExternal", &params).await?;
        Ok(())
    }

    // ---- real time ------------------------------------------------------------------------

    /// `rtm.connect` → websocket URL for live events.
    pub async fn rtm_url(&self) -> Result<String> {
        self.call_field("rtm.connect", "url", &[("batch_presence_aware", "1".into()), ("presence_sub", "true".into())]).await
    }
}

/// A file for `Client::upload_files`.
pub struct Upload {
    /// The name it gets on Slack.
    pub name: String,
    pub source: UploadSource,
}

pub enum UploadSource {
    /// A file on disk, streamed.
    Path(std::path::PathBuf),
    /// Bytes in memory (a pasted picture).
    Bytes(Vec<u8>),
}

/// A file's contents in 64 KiB chunks, reporting `progress(offset + sent, total)` as they are read.
fn read_chunks(file: tokio::fs::File, offset: u64, total: u64, progress: std::sync::Arc<impl Fn(u64, u64) + Send + Sync + 'static>) -> impl futures_util::Stream<Item = std::io::Result<Vec<u8>>> {
    use tokio::io::AsyncReadExt;
    futures_util::stream::unfold((file, offset), move |(mut file, sent)| {
        let progress = progress.clone();
        async move {
            let mut buf = vec![0u8; 64 * 1024];
            match file.read(&mut buf).await {
                Ok(0) => None,
                Ok(n) => {
                    buf.truncate(n);
                    let sent = sent + n as u64;
                    progress(sent, total);
                    Some((Ok(buf), (file, sent)))
                }
                Err(e) => Some((Err(e), (file, sent))),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;

    #[tokio::test]
    async fn chunks_report_progress_across_files() {
        let dir = std::env::temp_dir().join(format!("klack-chunks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.bin");
        std::fs::write(&path, vec![7u8; 100_000]).unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        let progress = std::sync::Arc::new(move |sent: u64, total: u64| log.lock().unwrap().push((sent, total)));
        // The second of two files: 5 000 bytes of the first were sent before it.
        let file = tokio::fs::File::open(&path).await.unwrap();
        let chunks: Vec<Vec<u8>> = super::read_chunks(file, 5_000, 105_000, progress).map(|c| c.unwrap()).collect().await;
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(chunks.iter().map(Vec::len).sum::<usize>(), 100_000);
        assert_eq!(*seen.lock().unwrap(), vec![(70_536, 105_000), (105_000, 105_000)]);
    }
}

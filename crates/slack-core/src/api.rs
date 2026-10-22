//! Slack Web API client authenticated like the web client (xoxc token + `d` cookie).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::dnd::{Schedule, ScheduleDays, ScheduleEdit};
use crate::session::{USER_AGENT, WorkspaceSession};
use crate::types::*;

#[derive(Debug, thiserror::Error)]
#[error("{method}: {error}")]
pub struct ApiError {
    pub method: String,
    pub error: String,
}

/// Whether `e` is Slack saying the session has ended (signed out, revoked, or the account
/// deactivated), as opposed to any other refusal or a network failure.
pub fn is_signed_out(e: &anyhow::Error) -> bool {
    e.downcast_ref::<ApiError>().is_some_and(|a| ["invalid_auth", "not_authed", "token_revoked", "account_inactive"].contains(&a.error.as_str()))
}

/// Whether `e` says nothing about what was asked for: the request failed (the network, rate limits, Slack's own
/// trouble) or the session has ended, as opposed to Slack's answer about it (`channel_not_found`, `thread_not_found`, …).
pub fn is_unanswered(e: &anyhow::Error) -> bool {
    match e.downcast_ref::<ApiError>() {
        Some(a) => is_signed_out(e) || ["ratelimited", "internal_error", "fatal_error", "service_unavailable", "request_timeout"].contains(&a.error.as_str()),
        None => true,
    }
}

/// Whether Slack has answered that a client's session has ended, shared by the client's clones.
#[derive(Clone, Default)]
struct SignedOutFlag(Arc<AtomicBool>);

impl SignedOutFlag {
    fn get(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Keep `e` as the call's answer, first noting when it says the session has ended (a network
    /// failure or another refusal leaves the flag as it is).
    fn note(&self, e: anyhow::Error) -> anyhow::Error {
        if is_signed_out(&e) {
            self.0.store(true, Ordering::Relaxed);
        }
        e
    }
}

/// The failure `TTYAK_FAIL` (`spec`: `method` or `method=error` items, comma separated) makes
/// `method`'s call answer with: Slack's `error` (an [`ApiError`]) or, for a plain `method`,
/// `injected_failure`. Debug builds only.
#[cfg(debug_assertions)]
fn injected(spec: &str, method: &str) -> Option<anyhow::Error> {
    spec.split(',').find_map(|item| match item.trim().split_once('=') {
        Some((m, error)) if m.trim() == method => Some(ApiError { method: method.to_string(), error: error.trim().to_string() }.into()),
        None if item.trim() == method => Some(anyhow!("{method}: injected_failure (TTYAK_FAIL)")),
        _ => None,
    })
}

/// `activity.feed`'s arguments: reads and unreads, newest first, of the item `types` (from `cursor` on).
fn activity_params(types: &str, limit: u32, cursor: Option<&str>) -> Params<'static> {
    let mut params: Params = vec![("mode", "chrono_reads_and_unreads".into()), ("types", types.into()), ("limit", limit.to_string())];
    if let Some(cursor) = cursor {
        params.push(("cursor", cursor.into()));
    }
    params
}

/// The form of a huddle-state renewal: `profile` with only `huddle_state*` fields (at least
/// one), and the reason Slack's client gives.
fn huddle_state_params(fields: &Value) -> Result<Params<'static>> {
    let map = fields.as_object().filter(|m| !m.is_empty()).ok_or_else(|| anyhow!("users.profile.set: no huddle state fields"))?;
    if let Some(other) = map.keys().find(|k| !k.starts_with("huddle_state")) {
        return Err(anyhow!("users.profile.set: {other} is not a huddle state field"));
    }
    Ok(vec![("profile", fields.to_string()), ("_x_reason", "increment-huddle-state-expiration-ts".into())])
}

/// `search.messages`'s arguments: a page of `count` matches, the newest first (Slack's default is by score).
fn search_params(query: &str, count: u32, page: u32) -> Vec<(&'static str, String)> {
    vec![("query", query.into()), ("count", count.to_string()), ("page", page.to_string()), ("highlight", "false".into()), ("sort", "timestamp".into()), ("sort_dir", "desc".into())]
}

/// `huddles.history`'s arguments: a page of `limit` (1–30, the most Slack's client asks for) from `cursor` on.
fn history_params(cursor: Option<&str>, limit: u32) -> Params<'static> {
    let mut params: Params = vec![("limit", limit.clamp(1, 30).to_string())];
    if let Some(cursor) = cursor.filter(|c| !c.is_empty()) {
        params.push(("cursor", cursor.into()));
    }
    params
}

/// The Flannel (edge API) address Slack gave and until when it holds, shared by a client's clones.
#[derive(Clone, Default)]
struct FlannelUrl(Arc<std::sync::Mutex<Option<(String, std::time::Instant)>>>);

impl FlannelUrl {
    fn fresh(&self) -> Option<String> {
        let kept = self.0.lock().unwrap_or_else(|e| e.into_inner());
        kept.as_ref().filter(|(_, until)| std::time::Instant::now() < *until).map(|(url, _)| url.clone())
    }

    fn keep(&self, url: String, ttl_seconds: u64) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some((url, std::time::Instant::now() + Duration::from_secs(ttl_seconds)));
    }
}

/// `url` parsed the way the request will be (WHATWG: `\` is `/`, what comes before `@` is a user name), when it
/// is safe to send the session to: https on Slack's own hosts (`slack.com`, `slack-gov.com` and their
/// subdomains), the default port, no user name or password.
pub fn slack_https_url(url: &str) -> Option<reqwest::Url> {
    slack_url(url, "https")
}

/// Like [`slack_https_url`], for a websocket address (`wss`): the gateway's, which carries the token.
pub fn slack_wss_url(url: &str) -> Option<reqwest::Url> {
    slack_url(url, "wss")
}

fn slack_url(url: &str, scheme: &str) -> Option<reqwest::Url> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    let slack = ["slack.com", "slack-gov.com"].iter().any(|d| host == *d || host.ends_with(&format!(".{d}")));
    let plain = parsed.scheme() == scheme && parsed.port().is_none() && parsed.username().is_empty() && parsed.password().is_none();
    (slack && plain).then_some(parsed)
}

/// The address of Slack's own websocket gateway as the web client opens it: `base` (`client.getWebSocketURL`'s
/// `primary_websocket_url`) with the token and the web client's parameters (`flannel=3`, the connection only — no
/// boot data). None unless `base` is one of Slack's own `wss` hosts ([`slack_wss_url`]): the token goes there.
fn gateway_url(base: &str, token: &str, routing_context: Option<&str>) -> Option<String> {
    let mut url = slack_wss_url(base)?;
    let start_args: String = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("agent", "client"),
            ("org_wide_aware", "true"),
            ("eac_cache_ts", "true"),
            ("cache_ts", "0"),
            ("name_tagging", "true"),
            ("only_self_subteams", "true"),
            ("connect_only", "true"),
            ("ms_latest", "true"),
        ])
        .finish();
    {
        let mut q = url.query_pairs_mut();
        // Not the web client's `sync_desync=1`: it brings `desync` events (caches of some channels to refetch),
        // and ttyak keeps no such caches beyond what a reconnect fetches.
        q.append_pair("token", token)
            .append_pair("slack_client", "desktop")
            .append_pair("start_args", &start_args)
            .append_pair("no_query_on_subscribe", "1")
            .append_pair("flannel", "3")
            .append_pair("lazy_channels", "1");
        if let Some(context) = routing_context.filter(|c| !c.is_empty()) {
            q.append_pair("gateway_server", context);
        }
        q.append_pair("batch_presence_aware", "1");
    }
    Some(url.into())
}

/// `e`'s text with the websocket address `url` and the `token` taken out: neither goes into logs or errors.
pub(crate) fn scrub(e: &anyhow::Error, url: &str, token: &str) -> String {
    let mut text = format!("{e:#}").replace(url, "<gateway>");
    if !token.is_empty() {
        text = text.replace(token, "<token>");
    }
    text
}

/// Which websocket ttyak is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Socket {
    /// Slack's own gateway, as the web client's: profile changes of the people asked for are pushed.
    Gateway,
    /// `rtm.connect`'s (a legacy connection): no profile changes of other people arrive there.
    Rtm,
}

/// The Flannel address and how many seconds it holds, from `api.getFlannelHttpUrl`'s answer. The session goes
/// to it, so it must be one of Slack's own https hosts ([`slack_https_url`]).
fn flannel_answer(v: &Value) -> Result<(String, u64)> {
    let url = v["url"].as_str().and_then(slack_https_url).ok_or_else(|| anyhow!("api.getFlannelHttpUrl: no Slack address"))?;
    Ok((url.as_str().trim_end_matches('/').to_string(), v["ttl_seconds"].as_u64().unwrap_or(0)))
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    pub session: WorkspaceSession,
    /// Set once Slack answered that the session has ended; shared by every clone.
    signed_out: SignedOutFlag,
    flannel: FlannelUrl,
}

pub type Params<'a> = Vec<(&'a str, String)>;

/// An HTTP client checking certificates against the system's store or — where the system has
/// none (a minimal container without ca-certificates) — against Mozilla's roots bundled in ttyak.
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
        Ok(Client { http, session, signed_out: SignedOutFlag::default(), flannel: FlannelUrl::default() })
    }

    /// Whether Slack has answered one of this client's calls (or a clone's) saying the session
    /// has ended (signed out, revoked, the account deactivated).
    pub fn signed_out(&self) -> bool {
        self.signed_out.get()
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// Call `method` and return the raw JSON (after the `ok` check). Retries on HTTP 429
    /// honouring `Retry-After`. An answer saying the session has ended sets [`Client::signed_out`].
    pub async fn call_raw(&self, method: &str, params: &[(&str, String)]) -> Result<Value> {
        self.ask(method, params).await.map_err(|e| self.signed_out.note(e))
    }

    async fn ask(&self, method: &str, params: &[(&str, String)]) -> Result<Value> {
        // Debug builds only: `TTYAK_FAIL=chat.delete,conversations.history=invalid_auth` makes
        // those calls fail (the second with Slack's `invalid_auth`), to test what the app does
        // when Slack refuses.
        #[cfg(debug_assertions)]
        if let Some(e) = std::env::var("TTYAK_FAIL").ok().and_then(|v| injected(&v, method)) {
            return Err(e);
        }
        let url = format!("{}{}", self.session.api_base(), method);
        let mut form: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
        form.push(("token", self.session.token.as_str()));
        self.answer(method, || self.http.post(&url).header(reqwest::header::COOKIE, self.session.cookie_header()).form(&form)).await
    }

    /// Send the request `request` builds — again after `Retry-After` when Slack answers HTTP 429 — and return
    /// Slack's answer after the `ok` check.
    async fn answer(&self, method: &str, request: impl Fn() -> reqwest::RequestBuilder) -> Result<Value> {
        for attempt in 0..5 {
            // reqwest's errors name the address (a Flannel one holds the team's id): leave it out.
            let resp = request().send().await.map_err(reqwest::Error::without_url).with_context(|| format!("{method}: request failed"))?;
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
            let body: Value = resp.json().await.map_err(reqwest::Error::without_url).with_context(|| format!("{method}: invalid JSON"))?;
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

    /// A thread's parent and its newest `limit` replies (one `conversations.replies` page: Slack gives
    /// the newest replies first when there are more).
    pub async fn latest_replies(&self, channel: &str, thread_ts: &str, limit: usize) -> Result<Vec<Message>> {
        let params: Params = vec![("channel", channel.into()), ("ts", thread_ts.into()), ("limit", limit.to_string())];
        let page: HistoryPage = self.call("conversations.replies", &params).await?;
        Ok(page.messages)
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

    /// The activity feed, newest first, of the item `types` (comma separated: `at_user`,
    /// `message_reaction`, `thread_v2`, `dm`, …; an unknown one fails the whole request).
    pub async fn activity_feed(&self, types: &str, limit: u32) -> Result<Value> {
        self.activity_feed_after(types, limit, None).await
    }

    /// [`Client::activity_feed`] from `cursor` on (the answer's `response_metadata.next_cursor`: the next page).
    pub async fn activity_feed_after(&self, types: &str, limit: u32, cursor: Option<&str>) -> Result<Value> {
        self.call_raw("activity.feed", &activity_params(types, limit, cursor)).await
    }

    /// Mark an Activity item read (`params`: its `type`, `feed_ts`, `key` and for some types `ts`), as Slack's client
    /// does when one is opened; Slack marks its conversation or thread read too.
    pub async fn activity_mark_read(&self, params: &[(&str, String)]) -> Result<()> {
        self.call_raw("activity.markRead", params).await.map(|_| ())
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

    /// Renew your "in a huddle" state, as Slack's client does while you are in a huddle
    /// (`users.profile.set` with only `huddle_state*` fields; anything else is refused, so the
    /// status is never touched). See docs/protocol/huddles.md.
    pub async fn set_huddle_state(&self, fields: &Value) -> Result<()> {
        self.call_raw("users.profile.set", &huddle_state_params(fields)?).await.map(|_| ())
    }

    /// Pause notifications for `minutes`, or resume them (None).
    pub async fn snooze(&self, minutes: Option<u64>) -> Result<()> {
        match minutes {
            Some(m) => self.call_raw("dnd.setSnooze", &[("num_minutes", m.to_string())]).await.map(|_| ()),
            None => self.call_raw("dnd.endSnooze", &[]).await.map(|_| ()),
        }
    }

    /// End the current scheduled quiet period early (and a snooze, if any); the schedule itself
    /// stays as it is.
    pub async fn end_dnd(&self) -> Result<()> {
        self.call_raw("dnd.endDnd", &[]).await.map(|_| ())
    }

    /// The notification schedule (`users.prefs.get`).
    pub async fn dnd_schedule(&self) -> Result<Schedule> {
        Ok(Schedule::from_prefs(&self.user_prefs().await?))
    }

    /// Write the notification schedule in one `users.prefs.set`. Slack answers ok even when it
    /// ignores the write (a value it doesn't take), so the result is read back and compared.
    pub async fn set_dnd_schedule(&self, schedule: &Schedule) -> Result<()> {
        self.write_dnd_schedule(schedule).await.map(|_| ())
    }

    /// Make `edits` to the schedule as Slack has it now (read just before, so what another client
    /// changed meanwhile stays), write it and read it back. Returns the prefs read back (the
    /// whole `prefs` object).
    pub async fn edit_dnd_schedule(&self, edits: &[ScheduleEdit]) -> Result<Value> {
        let schedule = Schedule::from_prefs(&self.user_prefs().await?).edited(edits).map_err(|e| anyhow!(e))?;
        self.write_dnd_schedule(&schedule).await
    }

    /// `set_dnd_schedule`, returning the prefs read back.
    async fn write_dnd_schedule(&self, schedule: &Schedule) -> Result<Value> {
        let prefs = match schedule.to_prefs() {
            Some(prefs) => prefs,
            None if schedule.days == ScheduleDays::Unknown => return Err(anyhow!("a schedule ttyak doesn't know is not written")),
            None => return Err(anyhow!("not a time of day (H:MM)")),
        };
        self.call_raw("users.prefs.set", &[("prefs", prefs.to_string())]).await?;
        let after = self.user_prefs().await?;
        if Schedule::from_prefs(&after).to_prefs().as_ref() != Some(&prefs) {
            return Err(anyhow!("users.prefs.set: Slack did not take the schedule"));
        }
        Ok(after)
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

    /// One page (1-based) of `count` matches, the newest first; `messages.paging.pages` says how many there are.
    pub async fn search_messages(&self, query: &str, count: u32, page: u32) -> Result<Value> {
        self.call_raw("search.messages", &search_params(query, count, page)).await
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

    // ---- huddles (see docs/protocol/huddles.md) ---------------------------------------------

    /// The Flannel (edge API) address of this workspace (`api.getFlannelHttpUrl`), kept for as long as Slack says.
    pub async fn flannel_url(&self) -> Result<String> {
        if let Some(url) = self.flannel.fresh() {
            return Ok(url);
        }
        let (url, ttl) = flannel_answer(&self.call_raw("api.getFlannelHttpUrl", &[("include_external_workspaces", "true".into())]).await?)?;
        self.flannel.keep(url.clone(), ttl);
        Ok(url)
    }

    /// Call Flannel's `method` (`huddles/list`, …) with the JSON object `body` (the token is added to it, never to
    /// the address); Slack's answer after the `ok` check, retried on HTTP 429 like [`Client::call_raw`].
    pub async fn flannel(&self, method: &str, body: Value) -> Result<Value> {
        self.ask_flannel(method, body).await.map_err(|e| self.signed_out.note(e))
    }

    async fn ask_flannel(&self, method: &str, body: Value) -> Result<Value> {
        #[cfg(debug_assertions)]
        if let Some(e) = std::env::var("TTYAK_FAIL").ok().and_then(|v| injected(&v, method)) {
            return Err(e);
        }
        let mut body = match body {
            Value::Object(map) => map,
            Value::Null => serde_json::Map::new(),
            _ => return Err(anyhow!("{method}: the body must be a JSON object")),
        };
        body.insert("token".into(), self.session.token.clone().into());
        // Checked again with the method added, and sent as parsed.
        let url = slack_https_url(&format!("{}/{method}", self.flannel_url().await?)).ok_or_else(|| anyhow!("{method}: not a Slack address"))?;
        self.answer(method, || self.http.post(url.clone()).header(reqwest::header::COOKIE, self.session.cookie_header()).json(&body)).await
    }

    /// The huddles happening now that this account can see, with who is in each (Flannel `huddles/list`, whose
    /// objects are whole: no `huddles/info` needed).
    pub async fn huddles_live(&self) -> Result<Vec<LiveHuddle>> {
        Ok(LiveHuddle::parse(&self.flannel("huddles/list", serde_json::json!({})).await?))
    }

    /// One page (newest first, at most 30) of the huddles this account was in or missed (`huddles.history`), from
    /// `cursor` on; the second value is the next page's cursor, if there is one.
    pub async fn huddles_history(&self, cursor: Option<&str>, limit: u32) -> Result<(Vec<PastHuddle>, Option<String>)> {
        Ok(PastHuddle::from_history(&self.call_raw("huddles.history", &history_params(cursor, limit)).await?))
    }

    // ---- real time ------------------------------------------------------------------------

    /// `rtm.connect` → websocket URL for live events.
    pub async fn rtm_url(&self) -> Result<String> {
        self.call_field("rtm.connect", "url", &[("batch_presence_aware", "1".into()), ("presence_sub", "true".into())]).await
    }

    /// The websocket to open: with `gateway`, Slack's own gateway (`client.getWebSocketURL`), as the web client
    /// does, else — or Slack not giving one of its own addresses — `rtm.connect`'s. The gateway address carries the
    /// token: never log or show it.
    pub async fn socket_url(&self, gateway: bool) -> Result<(String, Socket)> {
        if !gateway {
            return Ok((self.rtm_url().await?, Socket::Rtm));
        }
        match self.call_raw("client.getWebSocketURL", &[]).await {
            Ok(v) => {
                let base = v["primary_websocket_url"].as_str().unwrap_or_default();
                // Debug builds: `TTYAK_FAIL=gateway.connect` opens the gateway with a token Slack refuses.
                #[cfg(debug_assertions)]
                let refused = std::env::var("TTYAK_FAIL").ok().and_then(|v| injected(&v, "gateway.connect")).is_some();
                #[cfg(not(debug_assertions))]
                let refused = false;
                let token = if refused { "xoxc-ttyak-test-refused" } else { self.session.token.as_str() };
                if let Some(url) = gateway_url(base, token, v["routing_context"].as_str()) {
                    return Ok((url, Socket::Gateway));
                }
                tracing::warn!("client.getWebSocketURL: no Slack wss address; using rtm.connect");
            }
            Err(e) if is_signed_out(&e) => return Err(e),
            Err(e) => tracing::info!("client.getWebSocketURL: {e:#}; using rtm.connect"),
        }
        Ok((self.rtm_url().await?, Socket::Rtm))
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

    #[test]
    fn a_search_asks_for_the_newest_first() {
        let params = super::search_params("hello", 20, 2);
        assert!(params.contains(&("sort", "timestamp".to_string())) && params.contains(&("sort_dir", "desc".to_string())), "{params:?}");
        assert!(params.contains(&("page", "2".to_string())) && params.contains(&("query", "hello".to_string())));
    }

    #[test]
    fn the_activity_feed_asks_for_the_types_given() {
        let params = super::activity_params("dm,mpdm,bot_dm_bundle", 50, None);
        assert_eq!(params, vec![("mode", "chrono_reads_and_unreads".to_string()), ("types", "dm,mpdm,bot_dm_bundle".to_string()), ("limit", "50".to_string())]);
        let next = super::activity_params("thread_v2", 50, Some("abc"));
        assert_eq!((next[1].1.as_str(), next.last()), ("thread_v2", Some(&("cursor", "abc".to_string()))), "the next page");
    }

    #[test]
    fn the_huddle_state_write_carries_only_huddle_fields() {
        let fields = serde_json::json!({"huddle_state_expiration_ts": 1792616700.25});
        let params = super::huddle_state_params(&fields).unwrap();
        assert_eq!(params, vec![("profile", r#"{"huddle_state_expiration_ts":1792616700.25}"#.to_string()), ("_x_reason", "increment-huddle-state-expiration-ts".to_string())], "as Slack's client sends it");
        // Never the status (or anything else of the profile).
        for other in [serde_json::json!({"status_emoji": ":x:"}), serde_json::json!({"huddle_state_expiration_ts": 1, "status_text": ""}), serde_json::json!({}), serde_json::json!(5)] {
            assert!(super::huddle_state_params(&other).is_err(), "{other}");
        }
    }

    #[test]
    fn huddle_history_pages_are_at_most_30() {
        assert_eq!(super::history_params(None, 30), vec![("limit", "30".to_string())]);
        assert_eq!(super::history_params(Some("abc"), 100), vec![("limit", "30".to_string()), ("cursor", "abc".to_string())], "Slack's client never asks for more");
        assert_eq!(super::history_params(Some(""), 0), vec![("limit", "1".to_string())], "an empty cursor is the first page");
    }

    #[test]
    fn the_flannel_address_is_slacks_own() {
        let answer = |url: &str| super::flannel_answer(&serde_json::json!({"ok": true, "url": url, "ttl_seconds": 600}));
        assert_eq!(answer("https://edgeapi.slack.com/cache/E0000000001/").unwrap(), ("https://edgeapi.slack.com/cache/E0000000001".to_string(), 600));
        assert!(answer("https://edgeapi.slack-gov.com/cache/T0").is_ok());
        // The session cookie and token go there: nowhere else.
        for url in [
            "http://edgeapi.slack.com/cache/T0",
            "https://slack.com.example.org/cache",
            "https://example.org/x.slack.com",
            "https://evilslack.com/cache",
            "",
            // Browsers and reqwest read `\` as `/`: the host is evil.example.
            "https://evil.example\\.slack.com/cache/E0",
            // Before the `@` is a user name: the host is evil.example.
            "https://edgeapi.slack.com@evil.example/cache/E0",
            "https://user:pw@edgeapi.slack.com/cache/E0",
            "https://edgeapi.slack.com:8443/cache/E0",
        ] {
            assert!(answer(url).is_err(), "{url}");
        }
        assert!(super::flannel_answer(&serde_json::json!({"ok": true})).is_err(), "no url");
        assert_eq!(super::flannel_answer(&serde_json::json!({"url": "https://edgeapi.slack.com/cache/T0"})).unwrap().1, 0, "no ttl: not kept");
    }

    #[test]
    fn slack_urls_are_parsed_like_reqwest_does() {
        assert_eq!(super::slack_https_url("https://files.slack.com/files-pri/T0-F0/x.png").map(|u| u.to_string()).as_deref(), Some("https://files.slack.com/files-pri/T0-F0/x.png"));
        assert!(super::slack_https_url("https://slack.com/api/x").is_some());
        for url in ["https://evil.example\\.slack.com/x", "https://a.slack.com@evil.example/", "http://a.slack.com/", "https://u@a.slack.com/", "not a url"] {
            assert!(super::slack_https_url(url).is_none(), "{url}");
        }
    }

    #[test]
    fn the_gateway_is_slacks_own_wss_host() {
        assert!(super::slack_wss_url("wss://wss-primary.slack.com/").is_some());
        assert!(super::slack_wss_url("wss://wss-primary.slack-gov.com/").is_some());
        // The token goes there in the address: nowhere else.
        for url in [
            "https://wss-primary.slack.com/",
            "ws://wss-primary.slack.com/",
            "wss://slack.com.example.org/",
            "wss://evilslack.com/",
            "wss://evil.example\\.slack.com/",
            "wss://wss-primary.slack.com@evil.example/",
            "wss://u:p@wss-primary.slack.com/",
            "wss://wss-primary.slack.com:8443/",
            "",
        ] {
            assert!(super::slack_wss_url(url).is_none(), "{url}");
        }
    }

    #[test]
    fn the_gateway_address_is_the_web_clients() {
        let url = super::gateway_url("wss://wss-primary.slack.com", "xoxc-1", Some("ctx 1")).unwrap();
        let parsed = reqwest::Url::parse(&url).unwrap();
        assert_eq!((parsed.scheme(), parsed.host_str(), parsed.path()), ("wss", Some("wss-primary.slack.com"), "/"));
        let q: std::collections::HashMap<String, String> = parsed.query_pairs().into_owned().collect();
        // Not `sync_desync`: ttyak has nothing to do with the `desync` events it brings.
        assert!(!q.contains_key("sync_desync"));
        for (k, v) in [("token", "xoxc-1"), ("slack_client", "desktop"), ("no_query_on_subscribe", "1"), ("flannel", "3"), ("lazy_channels", "1"), ("gateway_server", "ctx 1"), ("batch_presence_aware", "1")] {
            assert_eq!(q.get(k).map(String::as_str), Some(v), "{k}");
        }
        // The boot data is not wanted: the connection only.
        let start: std::collections::HashMap<String, String> = url::form_urlencoded::parse(q["start_args"].as_bytes()).into_owned().collect();
        assert_eq!(start.get("connect_only").map(String::as_str), Some("true"));
        assert_eq!(start.get("org_wide_aware").map(String::as_str), Some("true"));
        assert!(!super::gateway_url("wss://wss-primary.slack.com/", "t", None).unwrap().contains("gateway_server"), "no routing context: none sent");
        for base in ["https://wss-primary.slack.com/", "wss://evil.example/", "wss://wss-primary.slack.com@evil.example/"] {
            assert!(super::gateway_url(base, "xoxc-1", None).is_none(), "{base}");
        }
    }

    /// A Slack on a local port answering `answers` (method → body; others `ok`), passing on each method asked.
    async fn fake_slack(answers: &'static [(&'static str, &'static str)]) -> (String, tokio::sync::mpsc::UnboundedReceiver<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut req = Vec::new();
                let mut buf = [0u8; 8192];
                loop {
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    req.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&req).to_string();
                    let done = text.find("\r\n\r\n").is_some_and(|h| {
                        let len = text[..h].lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok())).unwrap_or(0);
                        req.len() >= h + 4 + len
                    });
                    if n == 0 || done {
                        break;
                    }
                }
                let text = String::from_utf8_lossy(&req).to_string();
                let method = text.split_whitespace().nth(1).and_then(|p| p.strip_prefix("/api/")).unwrap_or_default().to_string();
                let body = answers.iter().find(|(m, _)| *m == method).map_or(r#"{"ok":true}"#, |(_, b)| *b);
                let _ = tx.send(method);
                let reply = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = sock.write_all(reply.as_bytes()).await;
            }
        });
        (url, rx)
    }

    #[tokio::test]
    async fn a_gateway_not_on_slacks_own_wss_host_falls_back_to_rtm_connect() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        const RTM: &str = r#"{"ok":true,"url":"wss://wss-primary.slack.com/websocket/LEGACY"}"#;
        for (base, answers) in [
            ("https", &[("client.getWebSocketURL", r#"{"ok":true,"primary_websocket_url":"https://wss-primary.slack.com/","routing_context":"c"}"#), ("rtm.connect", RTM)]),
            ("evil", &[("client.getWebSocketURL", r#"{"ok":true,"primary_websocket_url":"wss://evil.example/","routing_context":"c"}"#), ("rtm.connect", RTM)]),
        ] {
            let (url, mut asked) = fake_slack(answers).await;
            let session = crate::session::WorkspaceSession { url, token: "xoxc-T".into(), ..Default::default() };
            let (got, socket) = super::Client::new(session).unwrap().socket_url(true).await.unwrap();
            assert_eq!((got.as_str(), socket), ("wss://wss-primary.slack.com/websocket/LEGACY", super::Socket::Rtm), "{base}");
            assert!(!got.contains("xoxc"), "{base}");
            assert_eq!((asked.recv().await.as_deref(), asked.recv().await.as_deref()), (Some("client.getWebSocketURL"), Some("rtm.connect")), "{base}");
        }
        // A good one; and after failed connects (gateway: false) not even asked for.
        let (url, mut asked) = fake_slack(&[("client.getWebSocketURL", r#"{"ok":true,"primary_websocket_url":"wss://wss-primary.slack.com/"}"#), ("rtm.connect", RTM)]).await;
        let client = super::Client::new(crate::session::WorkspaceSession { url, token: "xoxc-T".into(), ..Default::default() }).unwrap();
        assert_eq!(client.socket_url(true).await.unwrap().1, super::Socket::Gateway);
        assert_eq!(client.socket_url(false).await.unwrap().1, super::Socket::Rtm);
        assert_eq!((asked.recv().await.as_deref(), asked.recv().await.as_deref()), (Some("client.getWebSocketURL"), Some("rtm.connect")));
    }

    #[test]
    fn a_gateway_error_does_not_name_its_address() {
        let url = super::gateway_url("wss://wss-primary.slack.com/", "xoxc-SECRET", Some("ctx")).unwrap();
        let e = anyhow::anyhow!("could not connect to {url}: refused (token xoxc-SECRET)");
        let text = super::scrub(&e, &url, "xoxc-SECRET");
        assert!(!text.contains("SECRET") && !text.contains("wss-primary"), "{text}");
        assert!(text.contains("refused"), "{text}");
    }

    #[tokio::test]
    async fn a_failed_request_does_not_name_its_address() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        // Nothing listens on port 1: the request fails at once.
        let session = crate::session::WorkspaceSession { url: "http://127.0.0.1:1/T0SECRET".into(), ..Default::default() };
        let e = super::Client::new(session).unwrap().call_raw("auth.test", &[]).await.unwrap_err();
        let text = format!("{e:#} {e:?}");
        assert!(text.starts_with("auth.test: request failed"), "{text}");
        assert!(!text.contains("127.0.0.1") && !text.contains("T0SECRET"), "{text}");
    }

    #[test]
    fn the_flannel_address_is_kept_for_its_ttl() {
        let kept = super::FlannelUrl::default();
        let clone = kept.clone();
        assert_eq!(kept.fresh(), None);
        kept.keep("https://edgeapi.slack.com/cache/T0".into(), 600);
        assert_eq!(clone.fresh().as_deref(), Some("https://edgeapi.slack.com/cache/T0"), "shared by clones");
        kept.keep("https://edgeapi.slack.com/cache/T0".into(), 0);
        assert_eq!(kept.fresh(), None, "expired");
    }

    #[test]
    fn signed_out_errors_are_recognised() {
        let api = |error: &str| anyhow::Error::from(super::ApiError { method: "auth.test".into(), error: error.into() });
        for error in ["invalid_auth", "not_authed", "token_revoked", "account_inactive"] {
            assert!(super::is_signed_out(&api(error)), "{error}");
        }
        // Also under a context line.
        assert!(super::is_signed_out(&api("invalid_auth").context("client.userBoot")));
        assert!(!super::is_signed_out(&api("ratelimited")));
        assert!(!super::is_signed_out(&anyhow::anyhow!("error sending request for url (https://acme.slack.com/api/auth.test): connection refused")));
    }

    #[test]
    fn unanswered_is_told_from_an_answer() {
        let api = |error: &str| anyhow::Error::from(super::ApiError { method: "conversations.replies".into(), error: error.into() });
        for e in [api("ratelimited"), api("internal_error"), api("fatal_error"), api("service_unavailable"), api("request_timeout"), api("invalid_auth"), anyhow::anyhow!("conversations.replies: request failed"), anyhow::anyhow!("conversations.replies: still rate limited after retries")] {
            assert!(super::is_unanswered(&e), "{e}");
        }
        for error in ["thread_not_found", "channel_not_found", "not_in_channel", "message_not_found"] {
            assert!(!super::is_unanswered(&api(error)), "Slack's answer: {error}");
        }
        assert!(!super::is_unanswered(&api("channel_not_found").context("a link")), "under a context line too");
    }

    #[test]
    fn a_signed_out_answer_sets_the_flag_for_every_clone() {
        // What a Client holds (a Client clones it with itself; building one needs a TLS provider).
        let flag = super::SignedOutFlag::default();
        let copy = flag.clone();
        let api = |error: &str| anyhow::Error::from(super::ApiError { method: "conversations.history".into(), error: error.into() });
        // Other refusals and network failures say nothing about the session.
        for e in [api("ratelimited"), api("channel_not_found"), anyhow::anyhow!("conversations.history: request failed")] {
            let kept = flag.note(e);
            assert!(!kept.to_string().is_empty());
            assert!(!flag.get() && !copy.get());
        }
        let e = flag.note(api("invalid_auth"));
        assert_eq!(e.to_string(), "conversations.history: invalid_auth", "the error is handed back as it was");
        assert!(copy.get() && flag.get(), "every clone sees it");
        assert!(!super::SignedOutFlag::default().get(), "a new client starts signed in");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn ttyak_fail_takes_methods_and_errors() {
        let spec = "chat.delete, conversations.history=invalid_auth";
        let plain = super::injected(spec, "chat.delete").unwrap();
        assert_eq!(plain.to_string(), "chat.delete: injected_failure (TTYAK_FAIL)");
        assert!(!super::is_signed_out(&plain));
        let api = super::injected(spec, "conversations.history").unwrap();
        assert_eq!(api.to_string(), "conversations.history: invalid_auth");
        assert!(super::is_signed_out(&api), "an ApiError of that error");
        assert!(super::injected(spec, "chat.postMessage").is_none());
        assert!(super::injected("", "chat.delete").is_none());
    }

    #[tokio::test]
    async fn chunks_report_progress_across_files() {
        let dir = std::env::temp_dir().join(format!("ttyak-chunks-{}", std::process::id()));
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

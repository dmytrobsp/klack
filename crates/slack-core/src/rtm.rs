//! Real-time events over a websocket: Slack's own gateway, as the web client opens it, else
//! `rtm.connect`'s ([`Client::socket_url`]). Reconnects with backoff, and stops for good once the
//! session has ended.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{Message as WsMessage, client::IntoClientRequest, http::HeaderValue};

use crate::api::{Client, Socket};
use crate::session::USER_AGENT;

#[derive(Debug, Clone)]
pub enum RtmEvent {
    /// Connected (Slack said hello), on this kind of websocket.
    Connected(Socket),
    Disconnected(String),
    /// The session has ended (Slack said so): the loop has stopped and does not reconnect.
    SignedOut,
    /// A raw event (`{"type": "message", ...}`).
    Event(Value),
}

#[derive(Debug, Clone)]
pub enum RtmCommand {
    /// We are typing in `channel`, or in the thread under `thread_ts`.
    Typing { channel: String, thread_ts: Option<String> },
    /// Any frame (for research); the id is added.
    Raw(Value),
    /// Subscribe to presence changes of these users.
    PresenceSub { users: Vec<String> },
    /// Ask for `user_change` events of these users (at most [`USER_CHANGE_SUB_MAX`]), as the
    /// web client does for the people it shows: Slack sends them only for people asked for.
    UserChangeSub { users: Vec<String> },
}

/// The most users one `user_change` subscription names (the web client's limit).
pub const USER_CHANGE_SUB_MAX: usize = 100;

/// The websocket frame for a command, with request id `id`.
fn frame(cmd: RtmCommand, id: u64) -> Value {
    match cmd {
        RtmCommand::Typing { channel, thread_ts: None } => json!({"id": id, "type": "typing", "channel": channel}),
        // RTM's `typing` drops `thread_ts`; the web client's `user_typing` keeps it.
        RtmCommand::Typing { channel, thread_ts: Some(ts) } => json!({"id": id, "type": "user_typing", "channel": channel, "thread_ts": ts}),
        RtmCommand::Raw(mut v) => {
            v["id"] = json!(id);
            v
        }
        RtmCommand::PresenceSub { users } => json!({"id": id, "type": "presence_sub", "ids": users}),
        RtmCommand::UserChangeSub { users } => json!({"id": id, "type": "subscription_update", "subtype": "user_change_subscribe_request", "ids": users}),
    }
}

pub struct Rtm {
    pub events: mpsc::UnboundedReceiver<RtmEvent>,
    pub commands: mpsc::UnboundedSender<RtmCommand>,
}

/// What the loop does after a connection ended.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    /// The session has ended: no reconnecting.
    Stop,
    /// Connect again after this many seconds.
    Retry(u64),
}

/// After a connection ended with `error` (None: it closed cleanly), `backoff` being the current
/// wait: stop when Slack answered that the session has ended or the client knows it has
/// (`signed_out`), else try again — at once after a clean end, else after the wait.
fn next_step(error: Option<&anyhow::Error>, signed_out: bool, backoff: u64) -> Step {
    match error {
        _ if signed_out => Step::Stop,
        Some(e) if crate::api::is_signed_out(e) => Step::Stop,
        Some(_) => Step::Retry(backoff),
        None => Step::Retry(1),
    }
}

/// The wait after `wait`: twice as long, a minute at most.
fn next_backoff(wait: u64) -> u64 {
    (wait * 2).min(60)
}

/// What a frame from Slack is to the loop.
#[derive(Debug, PartialEq, Eq)]
enum Incoming {
    Hello,
    Goodbye,
    /// Not passed on: `pong`, and the gateway's `reconnect_url` (a fast-reconnect address with the token in it,
    /// which ttyak does not use).
    Skip,
    Event,
}

fn incoming(v: &Value) -> Incoming {
    match v["type"].as_str() {
        Some("hello") => Incoming::Hello,
        Some("goodbye") => Incoming::Goodbye,
        Some("pong" | "reconnect_url") => Incoming::Skip,
        _ => Incoming::Event,
    }
}

/// After this many connects to the gateway in a row that Slack did not say hello on, `rtm.connect` is used.
const GATEWAY_FAILURES: u32 = 2;
/// How long `rtm.connect` is used then before the gateway is tried again (at the next reconnect).
const GATEWAY_RETRY: Duration = Duration::from_secs(600);

/// Which websocket the next connect opens: the gateway, unless it failed [`GATEWAY_FAILURES`] times in a row
/// (refused with an HTTP error, closed by the server before Slack's hello; not a network drop) — then `rtm.connect`'s for [`GATEWAY_RETRY`].
#[derive(Debug, Default)]
struct Route {
    failures: u32,
    rtm_until: Option<std::time::Instant>,
}

/// How a connection ended, for [`Route`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// Slack said hello on it.
    Hello,
    /// No hello, and the server is to blame: the handshake got an HTTP error status, or the server
    /// closed the connection (or said goodbye) before its hello.
    Refused,
    /// No hello because the network went (an IO error, a timeout, a stall): not the gateway's failure.
    Lost,
}

/// How a websocket handshake that failed with `e` ended.
fn handshake_outcome(e: &tokio_tungstenite::tungstenite::Error) -> Outcome {
    match e {
        tokio_tungstenite::tungstenite::Error::Http(_) => Outcome::Refused,
        _ => Outcome::Lost,
    }
}

impl Route {
    fn gateway(&self, now: std::time::Instant) -> bool {
        self.rtm_until.is_none_or(|until| now >= until)
    }

    /// A connection on `socket` ended so.
    fn ended(&mut self, socket: Socket, outcome: Outcome, now: std::time::Instant) {
        match (socket, outcome) {
            (Socket::Gateway, Outcome::Hello) => *self = Route::default(),
            (Socket::Gateway, Outcome::Refused) => {
                self.rtm_until = None;
                self.failures += 1;
                if self.failures >= GATEWAY_FAILURES {
                    tracing::warn!("rtm: the gateway failed {} times; using rtm.connect for a while", self.failures);
                    *self = Route { failures: 0, rtm_until: Some(now + GATEWAY_RETRY) };
                }
            }
            // (A network drop says nothing about the gateway: the count stays as it was.)
            (Socket::Gateway, Outcome::Lost) | (Socket::Rtm, _) => {}
        }
    }
}

/// Start the RTM loop on the current tokio runtime.
pub fn spawn(client: Client) -> Rtm {
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<RtmCommand>();
    tokio::spawn(async move {
        let mut backoff = 1u64;
        let mut route = Route::default();
        loop {
            // (Which socket it was and how it ended; none when no address was had.)
            let mut opened = None;
            let result = run_once(&client, &ev_tx, &mut cmd_rx, route.gateway(std::time::Instant::now()), &mut opened).await;
            if let Some((socket, outcome)) = opened {
                route.ended(socket, outcome, std::time::Instant::now());
            }
            let step = next_step(result.as_ref().err(), client.signed_out(), backoff);
            if let Err(e) = &result {
                if step != Step::Stop && ev_tx.send(RtmEvent::Disconnected(e.to_string())).is_err() {
                    return; // UI gone
                }
            }
            let wait = match step {
                Step::Stop => {
                    tracing::warn!("rtm: the session of {} has ended; not reconnecting", client.session.team_name);
                    let _ = ev_tx.send(RtmEvent::SignedOut);
                    return;
                }
                Step::Retry(wait) => wait,
            };
            if ev_tx.is_closed() {
                return;
            }
            tokio::time::sleep(Duration::from_secs(wait)).await;
            backoff = next_backoff(wait);
        }
    });
    Rtm { events: ev_rx, commands: cmd_tx }
}

/// One connection, to the gateway if `gateway`; `opened` says which socket it tried and how it ended.
async fn run_once(
    client: &Client,
    ev_tx: &mpsc::UnboundedSender<RtmEvent>,
    cmd_rx: &mut mpsc::UnboundedReceiver<RtmCommand>,
    gateway: bool,
    opened: &mut Option<(Socket, Outcome)>,
) -> anyhow::Result<()> {
    let (url, socket) = client.socket_url(gateway).await?;
    // (Until Slack's hello, or the server is seen refusing or closing: a network drop.)
    *opened = Some((socket, Outcome::Lost));
    // The gateway's address carries the token: errors naming it lose it.
    let connect = async {
        let mut req = url.as_str().into_client_request()?;
        let headers = req.headers_mut();
        headers.insert("Cookie", HeaderValue::from_str(&client.session.cookie_header())?);
        headers.insert("User-Agent", HeaderValue::from_static(USER_AGENT));
        headers.insert("Origin", HeaderValue::from_static("https://app.slack.com"));
        anyhow::Ok(tokio_tungstenite::connect_async(req).await?)
    };
    let (ws, _) = connect.await.map_err(|e| {
        if let Some(e) = e.downcast_ref::<tokio_tungstenite::tungstenite::Error>() {
            *opened = Some((socket, handshake_outcome(e)));
        }
        anyhow::anyhow!("websocket: {}", crate::api::scrub(&e, &url, &client.session.token))
    })?;
    drop(url);
    let (mut sink, mut stream) = ws.split();
    let mut next_id = 1u64;
    let mut ping = tokio::time::interval(Duration::from_secs(20));
    let mut last_seen = tokio::time::Instant::now();
    loop {
        tokio::select! {
            frame = stream.next() => {
                let Some(frame) = frame else {
                    closed_before_hello(opened);
                    anyhow::bail!("websocket closed")
                };
                // Another call of the client found the session ended.
                if client.signed_out() {
                    anyhow::bail!("signed out");
                }
                last_seen = tokio::time::Instant::now();
                match frame? {
                    WsMessage::Text(text) => {
                        let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
                        match incoming(&v) {
                            Incoming::Hello => {
                                *opened = Some((socket, Outcome::Hello));
                                let _ = ev_tx.send(RtmEvent::Connected(socket));
                            }
                            Incoming::Skip => {}
                            Incoming::Goodbye => {
                                closed_before_hello(opened);
                                anyhow::bail!("server said goodbye")
                            }
                            Incoming::Event => {
                                if ev_tx.send(RtmEvent::Event(v)).is_err() {
                                    return Ok(());
                                }
                            }
                        }
                    }
                    WsMessage::Ping(p) => sink.send(WsMessage::Pong(p)).await?,
                    WsMessage::Close(c) => {
                        closed_before_hello(opened);
                        anyhow::bail!("websocket closed: {c:?}")
                    }
                    _ => {}
                }
            }
            _ = ping.tick() => {
                if client.signed_out() {
                    anyhow::bail!("signed out");
                }
                if last_seen.elapsed() > Duration::from_secs(70) {
                    anyhow::bail!("websocket stalled");
                }
                next_id += 1;
                sink.send(WsMessage::Text(json!({"id": next_id, "type": "ping"}).to_string().into())).await?;
            }
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { return Ok(()) };
                next_id += 1;
                sink.send(WsMessage::Text(frame(cmd, next_id).to_string().into())).await?;
            }
        }
    }
}

/// The server closed the connection: before Slack's hello, that is the gateway refusing.
fn closed_before_hello(opened: &mut Option<(Socket, Outcome)>) {
    if let Some((_, outcome @ Outcome::Lost)) = opened {
        *outcome = Outcome::Refused;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_change_subscription_frame() {
        let f = frame(RtmCommand::UserChangeSub { users: vec!["U2".into(), "U3".into()] }, 7);
        assert_eq!(f, json!({"id": 7, "type": "subscription_update", "subtype": "user_change_subscribe_request", "ids": ["U2", "U3"]}));
        assert_eq!(frame(RtmCommand::PresenceSub { users: vec!["U2".into()] }, 8), json!({"id": 8, "type": "presence_sub", "ids": ["U2"]}));
    }

    #[test]
    fn the_gateways_reconnect_address_is_never_passed_on() {
        // It carries the token (and ttyak does not use it): not an event for the app, ctl or logs.
        let reconnect = json!({"type": "reconnect_url", "url": "wss://wss-primary.slack.com/?frt=x&token=xoxc-SECRET"});
        assert_eq!(incoming(&reconnect), Incoming::Skip);
        assert_eq!(incoming(&json!({"type": "pong", "reply_to": 3})), Incoming::Skip);
        assert_eq!(incoming(&json!({"type": "hello"})), Incoming::Hello);
        assert_eq!(incoming(&json!({"type": "goodbye"})), Incoming::Goodbye);
        assert_eq!(incoming(&json!({"type": "message", "text": "hi"})), Incoming::Event);
    }

    #[test]
    fn two_gateway_connects_without_hello_fall_back_to_rtm_connect_for_a_while() {
        let t0 = std::time::Instant::now();
        let mut route = Route::default();
        assert!(route.gateway(t0), "the gateway first");
        route.ended(Socket::Gateway, Outcome::Refused, t0);
        assert!(route.gateway(t0), "one failed connect: the gateway again");
        route.ended(Socket::Gateway, Outcome::Refused, t0);
        assert!(!route.gateway(t0), "two: rtm.connect for the next attempt");
        route.ended(Socket::Rtm, Outcome::Hello, t0 + Duration::from_secs(60));
        assert!(!route.gateway(t0 + GATEWAY_RETRY - Duration::from_secs(1)), "rtm.connect for a while");
        assert!(route.gateway(t0 + GATEWAY_RETRY), "then the gateway again");
        route.ended(Socket::Gateway, Outcome::Refused, t0 + GATEWAY_RETRY);
        assert!(route.gateway(t0 + GATEWAY_RETRY), "counted afresh");
        // A hello in between: the failures are forgotten.
        let mut route = Route::default();
        route.ended(Socket::Gateway, Outcome::Refused, t0);
        route.ended(Socket::Gateway, Outcome::Hello, t0);
        route.ended(Socket::Gateway, Outcome::Refused, t0);
        assert!(route.gateway(t0));
    }

    #[test]
    fn a_network_drop_before_hello_is_not_the_gateways_failure() {
        let t0 = std::time::Instant::now();
        let mut route = Route::default();
        // The network gone (an IO error, a timeout): the gateway is not to blame.
        for _ in 0..4 {
            route.ended(Socket::Gateway, Outcome::Lost, t0);
        }
        assert!(route.gateway(t0), "dropped connects do not switch to rtm.connect");
        // Nor do they reset the count of real refusals.
        route.ended(Socket::Gateway, Outcome::Refused, t0);
        route.ended(Socket::Gateway, Outcome::Lost, t0);
        route.ended(Socket::Gateway, Outcome::Refused, t0);
        assert!(!route.gateway(t0), "two refusals: rtm.connect");
        // Which handshake errors are the gateway refusing.
        use tokio_tungstenite::tungstenite::{Error, http::Response};
        let status = |code: u16| Error::Http(Box::new(Response::builder().status(code).body(None).unwrap()));
        assert_eq!(handshake_outcome(&status(403)), Outcome::Refused, "an HTTP error status");
        assert_eq!(handshake_outcome(&status(502)), Outcome::Refused);
        let io = |kind: std::io::ErrorKind| Error::Io(std::io::Error::from(kind));
        for kind in [std::io::ErrorKind::ConnectionReset, std::io::ErrorKind::TimedOut, std::io::ErrorKind::ConnectionRefused, std::io::ErrorKind::UnexpectedEof] {
            assert_eq!(handshake_outcome(&io(kind)), Outcome::Lost, "{kind:?}");
        }
        assert_eq!(handshake_outcome(&Error::ConnectionClosed), Outcome::Lost);
        // The server closing (or saying goodbye) before its hello is a refusal; after it, nothing changes.
        let mut opened = Some((Socket::Gateway, Outcome::Lost));
        closed_before_hello(&mut opened);
        assert_eq!(opened, Some((Socket::Gateway, Outcome::Refused)));
        let mut opened = Some((Socket::Gateway, Outcome::Hello));
        closed_before_hello(&mut opened);
        assert_eq!(opened, Some((Socket::Gateway, Outcome::Hello)));
    }

    #[test]
    fn the_rtm_loop_stops_when_signed_out() {
        let api = |error: &str| anyhow::Error::from(crate::api::ApiError { method: "rtm.connect".into(), error: error.into() });
        // rtm.connect answering that the session has ended, or the client knowing it already.
        assert_eq!(next_step(Some(&api("invalid_auth")), false, 4), Step::Stop);
        assert_eq!(next_step(Some(&anyhow::anyhow!("websocket closed")), true, 4), Step::Stop);
        assert_eq!(next_step(None, true, 4), Step::Stop);
        // Anything else backs off as before: the wait doubles up to a minute, a clean end starts again at a second.
        assert_eq!(next_step(Some(&api("ratelimited")), false, 4), Step::Retry(4));
        assert_eq!(next_step(Some(&anyhow::anyhow!("rtm.connect: request failed")), false, 8), Step::Retry(8));
        assert_eq!(next_step(None, false, 16), Step::Retry(1));
        assert_eq!(next_backoff(4), 8);
        assert_eq!(next_backoff(60), 60);
    }
}

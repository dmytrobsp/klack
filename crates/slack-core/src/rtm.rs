//! Real-time events over the RTM websocket (`rtm.connect`). Reconnects with backoff.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{Message as WsMessage, client::IntoClientRequest, http::HeaderValue};

use crate::api::Client;
use crate::session::USER_AGENT;

#[derive(Debug, Clone)]
pub enum RtmEvent {
    Connected,
    Disconnected(String),
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
}

pub struct Rtm {
    pub events: mpsc::UnboundedReceiver<RtmEvent>,
    pub commands: mpsc::UnboundedSender<RtmCommand>,
}

/// Start the RTM loop on the current tokio runtime.
pub fn spawn(client: Client) -> Rtm {
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<RtmCommand>();
    tokio::spawn(async move {
        let mut backoff = 1u64;
        loop {
            match run_once(&client, &ev_tx, &mut cmd_rx).await {
                Ok(()) => backoff = 1,
                Err(e) => {
                    if ev_tx.send(RtmEvent::Disconnected(e.to_string())).is_err() {
                        return; // UI gone
                    }
                }
            }
            if ev_tx.is_closed() {
                return;
            }
            tokio::time::sleep(Duration::from_secs(backoff)).await;
            backoff = (backoff * 2).min(60);
        }
    });
    Rtm { events: ev_rx, commands: cmd_tx }
}

async fn run_once(client: &Client, ev_tx: &mpsc::UnboundedSender<RtmEvent>, cmd_rx: &mut mpsc::UnboundedReceiver<RtmCommand>) -> anyhow::Result<()> {
    let url = client.rtm_url().await?;
    let mut req = url.into_client_request()?;
    let headers = req.headers_mut();
    headers.insert("Cookie", HeaderValue::from_str(&client.session.cookie_header())?);
    headers.insert("User-Agent", HeaderValue::from_static(USER_AGENT));
    headers.insert("Origin", HeaderValue::from_static("https://app.slack.com"));
    let (ws, _) = tokio_tungstenite::connect_async(req).await?;
    let (mut sink, mut stream) = ws.split();
    let mut next_id = 1u64;
    let mut ping = tokio::time::interval(Duration::from_secs(20));
    let mut last_seen = tokio::time::Instant::now();
    loop {
        tokio::select! {
            frame = stream.next() => {
                let Some(frame) = frame else { anyhow::bail!("websocket closed") };
                last_seen = tokio::time::Instant::now();
                match frame? {
                    WsMessage::Text(text) => {
                        let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
                        match v["type"].as_str() {
                            Some("hello") => { let _ = ev_tx.send(RtmEvent::Connected); }
                            Some("pong") => {}
                            Some("goodbye") => anyhow::bail!("server said goodbye"),
                            _ => {
                                if ev_tx.send(RtmEvent::Event(v)).is_err() {
                                    return Ok(());
                                }
                            }
                        }
                    }
                    WsMessage::Ping(p) => sink.send(WsMessage::Pong(p)).await?,
                    WsMessage::Close(c) => anyhow::bail!("websocket closed: {c:?}"),
                    _ => {}
                }
            }
            _ = ping.tick() => {
                if last_seen.elapsed() > Duration::from_secs(70) {
                    anyhow::bail!("websocket stalled");
                }
                next_id += 1;
                sink.send(WsMessage::Text(json!({"id": next_id, "type": "ping"}).to_string().into())).await?;
            }
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { return Ok(()) };
                next_id += 1;
                let frame = match cmd {
                    RtmCommand::Typing { channel, thread_ts: None } => json!({"id": next_id, "type": "typing", "channel": channel}),
                    // RTM's `typing` drops `thread_ts`; the web client's `user_typing` keeps it.
                    RtmCommand::Typing { channel, thread_ts: Some(ts) } => json!({"id": next_id, "type": "user_typing", "channel": channel, "thread_ts": ts}),
                    RtmCommand::Raw(mut v) => {
                        v["id"] = json!(next_id);
                        v
                    }
                    RtmCommand::PresenceSub { users } => json!({"id": next_id, "type": "presence_sub", "ids": users}),
                };
                sink.send(WsMessage::Text(frame.to_string().into())).await?;
            }
        }
    }
}

//! Non-interactive commands. They exercise the same client code as the UI and are what
//! the end-to-end tests drive (e.g. account B posting to account A).

use anyhow::{Result, anyhow, bail};
use clap::Subcommand;
use slack_core::Client;
use slack_core::session::Profile;
use slack_core::types::Conversation;

#[derive(Subcommand)]
pub enum Ctl {
    /// Show who this profile is signed in as.
    Whoami,
    /// List conversations.
    Channels,
    /// Print recent messages.
    History {
        channel: String,
        #[arg(short, default_value_t = 10)]
        n: u32,
    },
    /// Join (or start) the huddle of a conversation via `rooms.join`; prints the response
    /// with secrets redacted.
    HuddleJoin { channel: String },
    /// Join a conversation's huddle and run Chime signaling (JOIN, frames for N seconds,
    /// LEAVE), printing a summary of each frame without secrets.
    HuddleSignal {
        channel: String,
        #[arg(default_value_t = 10)]
        seconds: u64,
    },
    /// Leave a huddle via `rooms.leave`.
    HuddleLeave { channel: String, call_id: String, attendee_id: String },
    /// Call any Web API method with key=value arguments; prints the response with
    /// secrets redacted (for research).
    Api { method: String, args: Vec<String> },
    /// Download a slack.com URL with the session (for research) into a file.
    Get { url: String, out: std::path::PathBuf },
    /// Search messages and print the matches as raw JSON fields.
    Search { query: String },
    /// Print a thread (parent first).
    Replies { channel: String, ts: String },
    /// Post a message (mrkdwn).
    Post {
        channel: String,
        text: String,
        #[arg(long)]
        thread: Option<String>,
    },
    /// Pin (or with --remove unpin) a message.
    Pin {
        channel: String,
        ts: String,
        #[arg(long)]
        remove: bool,
    },
    /// Add a reaction.
    React { channel: String, ts: String, emoji: String },
    /// Upload a file.
    Upload {
        channel: String,
        path: std::path::PathBuf,
        #[arg(long)]
        comment: Option<String>,
    },
    /// Print live events for N seconds.
    Listen {
        #[arg(default_value_t = 30)]
        seconds: u64,
    },
}

pub fn client(profile: &str, team: Option<&str>) -> Result<Client> {
    let p = Profile::load(profile)?;
    let ws = p.workspace(team).ok_or_else(|| anyhow!("profile '{profile}' has no such workspace"))?;
    Client::new(ws.clone())
}

/// Resolve `C123`, `#name`, `name` or `@user` to a conversation id.
pub async fn resolve_channel(client: &Client, spec: &str) -> Result<String> {
    if spec.len() >= 9 && spec.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
        return Ok(spec.to_string());
    }
    if let Some(user) = spec.strip_prefix('@') {
        let users = client.users_list().await?;
        let u = users
            .iter()
            .find(|u| u.name == user || u.display() == user || u.id == user)
            .ok_or_else(|| anyhow!("no user {user}"))?;
        return Ok(client.open_dm(&[&u.id]).await?.id);
    }
    let name = spec.trim_start_matches('#');
    let convs: Vec<Conversation> = client.conversations_list().await?;
    convs.into_iter().find(|c| c.name == name).map(|c| c.id).ok_or_else(|| anyhow!("no channel {spec}"))
}

pub async fn run(profile: &str, team: Option<&str>, cmd: Ctl) -> Result<()> {
    let client = client(profile, team)?;
    match cmd {
        Ctl::Whoami => {
            let a = client.auth_test().await?;
            println!("{} ({}) in {} ({}) — {}", a.user, a.user_id, a.team, a.team_id, a.url);
        }
        Ctl::Channels => {
            for c in client.conversations_list().await? {
                let kind = if c.is_im { "dm" } else if c.is_mpim { "group-dm" } else if c.is_private { "private" } else { "channel" };
                println!("{:<12} {:<9} {}", c.id, kind, if c.name.is_empty() { c.user.unwrap_or_default() } else { c.name });
            }
        }
        Ctl::History { channel, n } => {
            let id = resolve_channel(&client, &channel).await?;
            let page = client.history(&id, None, n).await?;
            for m in page.messages.iter().rev() {
                println!("{} {:<12} {}{}", m.ts, m.user.as_deref().or(m.username.as_deref()).unwrap_or("?"), m.text.replace('\n', " ⏎ "), reactions(m));
            }
        }
        Ctl::HuddleJoin { channel } => {
            let id = resolve_channel(&client, &channel).await?;
            let v = client.call_raw("rooms.join", &[("channel_id", id)]).await?;
            println!("{}", serde_json::to_string_pretty(&redact(v))?);
        }
        Ctl::HuddleSignal { channel, seconds } => {
            use huddle::proto::sdk_signal_frame::Type;
            let id = resolve_channel(&client, &channel).await?;
            let creds = huddle::signaling::Credentials::from_rooms_join(&client.call_raw("rooms.join", &[("channel_id", id)]).await?)?;
            println!("room {} meeting {} attendee {}", creds.room_id, creds.meeting.meeting_id, creds.attendee.attendee_id);
            let mut sig = huddle::signaling::Signaling::connect(&creds).await?;
            println!("signaling connected");
            sig.send(&huddle::signaling::join_frame(std::process::id() as u64)).await?;
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(seconds);
            let mut left = false;
            loop {
                let next = tokio::time::timeout_at(deadline, sig.next()).await;
                let frame = match next {
                    Ok(Some(f)) => f?,
                    Ok(None) => break,
                    Err(_) if !left => {
                        sig.send(&huddle::signaling::frame(Type::Leave)).await?;
                        println!("-> LEAVE");
                        left = true;
                        // Give the server a moment to acknowledge.
                        match tokio::time::timeout(std::time::Duration::from_secs(5), sig.next()).await {
                            Ok(Some(f)) => f?,
                            _ => break,
                        }
                    }
                    Err(_) => break,
                };
                println!("<- {}", describe_frame(&frame));
                if frame.r#type == Type::LeaveAck as i32 {
                    break;
                }
            }
            sig.close().await;
        }
        Ctl::HuddleLeave { channel, call_id, attendee_id } => {
            let id = resolve_channel(&client, &channel).await?;
            let v = client.call_raw("rooms.leave", &[("channel_id", id), ("call_id", call_id), ("attendee_id", attendee_id), ("reason", "user_initiated".into())]).await?;
            println!("{}", serde_json::to_string_pretty(&redact(v))?);
        }
        Ctl::Api { method, args } => {
            let params: Vec<(&str, String)> = args.iter().filter_map(|a| a.split_once('=')).map(|(k, v)| (k, v.to_string())).collect();
            let v = client.call_raw(&method, &params).await?;
            println!("{}", serde_json::to_string_pretty(&redact(v))?);
        }
        Ctl::Get { url, out } => {
            // The session (cookie + token) is only ever sent to Slack's own hosts.
            let host = url.strip_prefix("https://").and_then(|r| r.split('/').next()).unwrap_or_default();
            if !(host == "slack.com" || host.ends_with(".slack.com")) {
                return Err(anyhow!("refusing to send the session to {host}"));
            }
            let bytes = client.download(&url).await?;
            std::fs::write(&out, &bytes)?;
            println!("{} bytes", bytes.len());
        }
        Ctl::Search { query } => {
            let v = client.search_messages(&query, 20).await?;
            println!("total: {}", v["messages"]["total"]);
            for m in v["messages"]["matches"].as_array().into_iter().flatten() {
                let keys: Vec<&String> = m.as_object().map(|o| o.keys().collect()).unwrap_or_default();
                println!("{} #{} {} {:?} | {}", m["ts"].as_str().unwrap_or("?"), m["channel"]["name"].as_str().unwrap_or("?"), m["user"].as_str().unwrap_or("?"), m["text"].as_str().unwrap_or(""), keys.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(","));
            }
        }
        Ctl::Replies { channel, ts } => {
            let id = resolve_channel(&client, &channel).await?;
            for m in client.replies(&id, &ts).await?.messages {
                let kind = m.subtype.as_deref().map(|s| format!(" [{s}]")).unwrap_or_default();
                println!("{} {:<12} {}{kind}{}", m.ts, m.user.as_deref().unwrap_or("?"), m.text.replace('\n', " ⏎ "), reactions(&m));
            }
        }
        Ctl::Post { channel, text, thread } => {
            let id = resolve_channel(&client, &channel).await?;
            let m = client.post_message(&id, &text, thread.as_deref(), false).await?;
            println!("{}", m.ts);
        }
        Ctl::Pin { channel, ts, remove } => {
            let id = resolve_channel(&client, &channel).await?;
            client.pin(&id, &ts, !remove).await?;
        }
        Ctl::React { channel, ts, emoji } => {
            let id = resolve_channel(&client, &channel).await?;
            client.react(&id, &ts, emoji.trim_matches(':'), true).await?;
        }
        Ctl::Upload { channel, path, comment } => {
            let id = resolve_channel(&client, &channel).await?;
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).ok_or_else(|| anyhow!("bad path"))?;
            let bytes = std::fs::read(&path)?;
            let file_id = client.upload(&id, &name, bytes, comment.as_deref(), None).await?;
            println!("{file_id}");
        }
        Ctl::Listen { seconds } => {
            let mut rtm = slack_core::rtm::spawn(client.clone());
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(seconds);
            loop {
                tokio::select! {
                    ev = rtm.events.recv() => match ev {
                        Some(slack_core::rtm::RtmEvent::Event(v)) => println!("{}", serde_json::to_string(&v)?),
                        Some(other) => println!("# {other:?}"),
                        None => bail!("rtm ended"),
                    },
                    _ = tokio::time::sleep_until(deadline) => break,
                }
            }
        }
    }
    Ok(())
}

/// ` [:name: by U1,U2]…` for a printed message.
fn reactions(m: &slack_core::types::Message) -> String {
    let pinned = if m.pinned_to.is_empty() { "" } else { " [pinned]" };
    m.reactions.iter().map(|r| format!(" [:{}: by {}]", r.name, r.users.join(","))).collect::<String>() + pinned
}

/// Replace the values of secret-looking keys (tokens, passwords, credentials, signatures).
fn redact(v: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match v {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| {
                    let secret = ["token", "password", "secret", "credential", "signature"].iter().any(|s| k.to_lowercase().contains(s));
                    let v = if secret && !v.is_null() { Value::String("<redacted>".into()) } else { redact(v) };
                    (k, v)
                })
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.into_iter().map(redact).collect()),
        other => other,
    }
}

/// One-line summary of a signaling frame, without credentials.
fn describe_frame(f: &huddle::proto::SdkSignalFrame) -> String {
    use huddle::proto::sdk_signal_frame::Type;
    let kind = Type::try_from(f.r#type).map(|t| format!("{t:?}")).unwrap_or_else(|_| format!("type {}", f.r#type));
    let detail = if let Some(e) = &f.error {
        format!("error {:?} {:?}", e.status, e.description)
    } else if let Some(a) = &f.joinack {
        let turn = a.turn_credentials.as_ref().map(|t| format!("{} TURN uris, ttl {:?}", t.uris.len(), t.ttl)).unwrap_or_else(|| "no TURN credentials".into());
        format!("{turn}, video limit {:?}", a.video_subscription_limit)
    } else if let Some(i) = &f.index {
        format!("participants {:?}, {} sources", i.num_participants, i.sources.len())
    } else if let Some(s) = &f.audio_stream_id_info {
        s.streams.iter().map(|s| format!("[{:?} {:?} muted={:?} dropped={:?}]", s.audio_stream_id, s.external_user_id, s.muted, s.dropped)).collect::<Vec<_>>().join(" ")
    } else if let Some(m) = &f.audio_metadata {
        format!("{} attendee states", m.attendee_states.len())
    } else if let Some(p) = &f.ping_pong {
        format!("ping {}", p.ping_id)
    } else {
        String::new()
    };
    format!("{kind} {detail}")
}

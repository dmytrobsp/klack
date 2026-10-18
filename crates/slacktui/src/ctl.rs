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
    /// Post a message (mrkdwn).
    Post {
        channel: String,
        text: String,
        #[arg(long)]
        thread: Option<String>,
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
                println!("{} {:<12} {}", m.ts, m.user.as_deref().or(m.username.as_deref()).unwrap_or("?"), m.text.replace('\n', " ⏎ "));
            }
        }
        Ctl::Post { channel, text, thread } => {
            let id = resolve_channel(&client, &channel).await?;
            let m = client.post_message(&id, &text, thread.as_deref(), false).await?;
            println!("{}", m.ts);
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

//! klack — a native Slack client for the terminal.

mod ctl;
mod tui;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use slack_core::session::{self, Profile};

#[derive(Parser)]
#[command(name = "klack", version, about = "Native Slack client for the terminal")]
struct Cli {
    /// Session profile (separate accounts/workspace sets).
    #[arg(long, global = true, default_value = "default")]
    profile: String,

    /// Workspace (team id or name) when the profile has several.
    #[arg(long, global = true)]
    team: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Create or refresh the session for this profile.
    Login {
        /// Sign in with a browser session: the workspace URL (https://acme.slack.com) plus the
        /// `d` cookie from --cookie, $KLACK_COOKIE_D, or a prompt.
        #[arg(long, value_name = "URL")]
        workspace: String,
        /// Value of the browser's `d` cookie for slack.com (prefer the env var or the prompt,
        /// so it does not end up in your shell history).
        #[arg(long)]
        cookie: Option<String>,
    },
    /// Scriptable commands (used for automation and end-to-end tests).
    Ctl {
        #[command(subcommand)]
        cmd: ctl::Ctl,
    },
}

fn init_logging(profile: &str) -> Result<()> {
    let dir = session::cache_dir(profile)?;
    std::fs::create_dir_all(&dir)?;
    let file = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("klack.log"))?;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_env("KLACK_LOG").unwrap_or_else(|_| "klack=info,slack_core=info,huddle=info".into()))
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false)
        .init();
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // Both rustls crypto providers are compiled in (webrtc uses ring, reqwest/tungstenite
    // aws-lc-rs), so rustls cannot pick one by itself: choose explicitly.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cli = Cli::parse();
    // Folders of the old name (slacktui) become klack's.
    let adopted = session::adopt_old_dirs();
    init_logging(&cli.profile)?;
    for line in adopted {
        tracing::info!("{line}");
    }
    match cli.command {
        Some(Command::Login { workspace: url, cookie }) => {
            let cookie = match cookie.or_else(|| std::env::var("KLACK_COOKIE_D").ok()) {
                Some(c) => c,
                None => {
                    eprint!("value of the `d` cookie: ");
                    let mut line = String::new();
                    std::io::stdin().read_line(&mut line)?;
                    line
                }
            };
            let url = if url.starts_with("https://") { url } else { format!("https://{url}") };
            let sessions = vec![session::session_from_cookie(&url, cookie.trim().trim_start_matches("d=")).await.context("sign-in with the cookie failed")?];
            let mut profile = Profile::load(&cli.profile).unwrap_or_default();
            for ws in sessions {
                println!("signed in to {} ({}) as {} ({})", ws.team_name, ws.team_id, ws.user_name, ws.user_id);
                profile.upsert(ws);
            }
            let path = profile.save(&cli.profile)?;
            println!("session saved to {}", path.display());
            Ok(())
        }
        Some(Command::Ctl { cmd }) => ctl::run(&cli.profile, cli.team.as_deref(), cmd).await,
        None => tui::event_loop::run(slack_core::session::Profile::load(&cli.profile)?, cli.team.as_deref()).await,
    }
}

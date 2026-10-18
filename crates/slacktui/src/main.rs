//! slacktui — a native Slack client for the terminal.

mod ctl;
mod tui;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use slack_core::session::{self, Profile};

#[derive(Parser)]
#[command(name = "slacktui", version, about = "Native Slack client for the terminal")]
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
        /// Import from an RSlack profile ("default" for RSlack's default profile).
        #[arg(long, value_name = "RSLACK_PROFILE")]
        import_rslack: Option<String>,
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
    let file = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("slacktui.log"))?;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_env("SLACKTUI_LOG").unwrap_or_else(|_| "slacktui=info,slack_core=info".into()))
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false)
        .init();
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_logging(&cli.profile)?;
    match cli.command {
        Some(Command::Login { import_rslack }) => {
            let rslack_profile = import_rslack.as_deref().filter(|p| *p != "default");
            let sessions = session::import_rslack(rslack_profile).await.context("import from RSlack failed")?;
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
        None => tui::event_loop::run(ctl::client(&cli.profile, cli.team.as_deref())?).await,
    }
}

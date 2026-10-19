//! klack — a native Slack client for the terminal.

mod ctl;
mod login_browser;
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
        /// The workspace URL (https://acme.slack.com). Without --cookie / $KLACK_COOKIE_D a browser
        /// window opens to sign in (Chrome, Chromium, Edge or Brave, with a temporary profile);
        /// failing that, you are asked for the `d` cookie.
        #[arg(long, value_name = "URL")]
        workspace: String,
        /// Value of the browser's `d` cookie for slack.com (prefer the env var or the prompt,
        /// so it does not end up in your shell history).
        #[arg(long)]
        cookie: Option<String>,
        /// Do not open a browser window; ask for the cookie.
        #[arg(long)]
        no_browser: bool,
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
    let _ = tui::terminal::STDERR_LOG.set(dir.join("klack.log"));
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
        Some(Command::Login { workspace: url, cookie, no_browser }) => {
            let url = if url.starts_with("https://") { url } else { format!("https://{url}") };
            let given = cookie.or_else(|| std::env::var("KLACK_COOKIE_D").ok());
            // No cookie given: sign in in a browser window, else ask for it.
            let mut signed_in = None;
            if given.is_none() && !no_browser {
                match login_browser::find() {
                    Some(browser) => {
                        eprintln!("Opening {} to sign in to {url} — sign in there; the window closes by itself (Ctrl+C cancels).", browser.label);
                        match login_browser::sign_in(&browser, &url).await {
                            Ok(s) => signed_in = Some(s),
                            Err(e) => eprintln!("browser sign-in did not finish: {e:#}"),
                        }
                    }
                    None => eprintln!("{}", login_browser::NoBrowser),
                }
            }
            let ws = match signed_in {
                Some(s) => s,
                None => {
                    let cookie = match given {
                        Some(c) => c,
                        None => {
                            eprint!("value of the `d` cookie (devtools → Application → Cookies → slack.com): ");
                            let mut line = String::new();
                            std::io::stdin().read_line(&mut line)?;
                            line
                        }
                    };
                    session::session_from_cookie(&url, cookie.trim().trim_start_matches("d=")).await.context("sign-in with the cookie failed")?
                }
            };
            let sessions = vec![ws];
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

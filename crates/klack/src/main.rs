//! klack — a native Slack client for the terminal.

mod ctl;
mod login_browser;
mod login_import;
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
        /// Take the session from a browser you are signed in to Slack with (read-only).
        #[arg(long, value_enum, value_name = "BROWSER")]
        from_browser: Option<login_import::Source>,
        /// With --from-browser: the browser profile folder to read, or a folder of profiles (default:
        /// all of them). Several signing in as different people: you are asked which.
        #[arg(long, value_name = "DIR", requires = "from_browser")]
        browser_profile: Option<std::path::PathBuf>,
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

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

#[tokio::main]
async fn main() -> Result<()> {
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::new_heap();
    // rustls and reqwest are built without a crypto provider of their own (one fewer to carry):
    // ring, as webrtc uses, is the one for every TLS connection.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cli = Cli::parse();
    // Folders of the old name (slacktui) become klack's.
    let adopted = session::adopt_old_dirs();
    init_logging(&cli.profile)?;
    for line in adopted {
        tracing::info!("{line}");
    }
    match cli.command {
        Some(Command::Login { workspace: url, cookie, no_browser, from_browser, browser_profile }) => {
            let url = if url.starts_with("https://") { url } else { format!("https://{url}") };
            let given = cookie.or_else(|| std::env::var("KLACK_COOKIE_D").ok());
            // No cookie given: take it from a browser (when asked), sign in in a browser window,
            // or ask for it.
            let mut signed_in = None;
            if let Some(source) = from_browser {
                let found = login_import::find(source, browser_profile.as_deref()).await.with_context(|| format!("reading {}'s cookies", login_import::label(source)))?;
                if found.is_empty() {
                    anyhow::bail!("no Slack session in {} — sign in to Slack there first", login_import::label(source));
                }
                // Every profile that signs in to the workspace, one per account (freshest first).
                let mut working: Vec<(std::path::PathBuf, session::WorkspaceSession)> = Vec::new();
                for f in &found {
                    match session::session_from_cookie(&url, &f.cookie).await {
                        Ok(s) if !working.iter().any(|(_, w)| w.user_id == s.user_id) => working.push((f.profile.clone(), s)),
                        Ok(_) => {}
                        Err(e) => tracing::debug!("{}: {e:#}", f.profile.display()),
                    }
                }
                if working.is_empty() {
                    anyhow::bail!("{} has Slack cookies, but none signs in to {url} (signed out, or another workspace)", login_import::label(source));
                }
                let pick = if working.len() > 1 && std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                    eprintln!("{} profiles sign in to {url}:", working.len());
                    for (n, (p, s)) in working.iter().enumerate() {
                        eprintln!("  {}) {} — {}", n + 1, login_import::profile_name(p), s.user_name);
                    }
                    loop {
                        eprint!("which one? [1-{}] ", working.len());
                        let mut line = String::new();
                        if std::io::stdin().read_line(&mut line)? == 0 {
                            anyhow::bail!("no profile chosen");
                        }
                        match line.trim().parse::<usize>() {
                            Ok(n) if (1..=working.len()).contains(&n) => break n - 1,
                            _ => eprintln!("a number from 1 to {}, please", working.len()),
                        }
                    }
                } else {
                    // Not asked: the freshest (the list starts with it).
                    0
                };
                let (profile, s) = working.swap_remove(pick);
                eprintln!("took the session from {} ({})", login_import::label(source), login_import::profile_name(&profile));
                signed_in = Some(s);
            } else if given.is_none() && !no_browser {
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

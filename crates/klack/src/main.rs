//! klack — a native Slack client for the terminal.

mod auth;
mod ctl;
mod login_browser;
mod login_import;
mod pick;
mod tui;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use slack_core::profiles::{self, Dirs};
use slack_core::session::{self, Profile};

#[derive(Parser)]
#[command(name = "klack", version, about = "Native Slack client for the terminal")]
struct Cli {
    /// Session profile (separate accounts/workspace sets); without it the default profile
    /// (`klack profile default`), else `default`.
    #[arg(long, global = true)]
    profile: Option<String>,

    /// Workspace (team id or name) when the profile has several.
    #[arg(long, global = true)]
    team: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Sign in, sign out on this machine, or check the profile's sign-ins.
    Auth {
        #[command(subcommand)]
        cmd: AuthCmd,
    },
    /// List profiles, choose the default one, or remove one.
    Profile {
        #[command(subcommand)]
        cmd: ProfileCmd,
    },
    /// Delete everything klack wrote to this machine: every profile's sign-ins, settings, logs,
    /// drafts. Files you saved from messages stay.
    Purge {
        /// Do not ask.
        #[arg(long)]
        yes: bool,
    },
    /// Scriptable commands (used for automation and end-to-end tests).
    Ctl {
        #[command(subcommand)]
        cmd: ctl::Ctl,
    },
}

#[derive(Subcommand)]
enum AuthCmd {
    /// Sign the profile in (a new profile name makes that profile). Without flags a guide lists the
    /// Slack sign-ins of your browsers to pick from, or signs in to another workspace.
    Login {
        /// The workspace (acme, acme.slack.com or a link from the browser). In a terminal the ways
        /// to sign in to it are listed; without one a browser window opens to sign in (Chrome,
        /// Chromium, Edge or Brave, with a temporary profile), failing that the `d` cookie is read.
        #[arg(long, value_name = "URL")]
        workspace: Option<String>,
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
    /// Forget the sign-in of a workspace (--team) or of all (--all) on this machine; Slack is not
    /// signed out.
    Logout {
        /// Every workspace of the profile.
        #[arg(long)]
        all: bool,
    },
    /// Each workspace of the profile: who you are, where the session came from, whether it works.
    Status,
}

#[derive(Subcommand)]
enum ProfileCmd {
    /// Every profile (★ the default) with its workspaces.
    List,
    /// Make plain `klack` (and `auth`, `ctl`) use this profile.
    Default { name: String },
    /// Delete a profile's sign-ins and cache (drafts and sidebar layout stay).
    Remove {
        name: String,
        /// Do not ask.
        #[arg(long)]
        yes: bool,
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

fn main() -> Result<()> {
    // glibc gives every thread that allocates its own arena and keeps what is freed in it: with
    // a thread per core that held megabytes klack no longer used. Two are plenty for a terminal
    // client; this has to happen before the runtime starts its threads.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: mallopt only adjusts an allocator setting; no thread is running yet.
    unsafe {
        libc::mallopt(libc::M_ARENA_MAX, 2);
    }
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(run())
}

async fn run() -> Result<()> {
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::new_heap();
    // rustls and reqwest are built without a crypto provider of their own (one fewer to carry):
    // ring, as webrtc uses, is the one for every TLS connection.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cli = Cli::parse();
    // Folders of the old name (slacktui) become klack's.
    let adopted = session::adopt_old_dirs();
    let dirs = Dirs::system()?;
    let profile = match cli.profile {
        Some(name) => name,
        None => profiles::default_name(&dirs),
    };
    profiles::valid_name(&profile)?;
    // Only what works on a signed-in profile logs: a mistyped profile name leaves no folder.
    let logs = matches!(cli.command, None | Some(Command::Ctl { .. }) | Some(Command::Auth { cmd: AuthCmd::Status | AuthCmd::Logout { .. } }));
    let session_file = dirs.config.join(&profile).join("session.json");
    let logging = logs && session_file.is_file();
    if logging {
        init_logging(&profile)?;
        for line in adopted {
            tracing::info!("{line}");
        }
    }
    match cli.command {
        Some(Command::Auth { cmd }) => match cmd {
            AuthCmd::Login { workspace, cookie, no_browser, from_browser, browser_profile } => auth::login(&profile, auth::LoginFlags { workspace, cookie, no_browser, from_browser, browser_profile }).await,
            AuthCmd::Logout { all } => auth::logout(&profile, cli.team.as_deref(), all).await,
            AuthCmd::Status => auth::status(&profile).await,
        },
        Some(Command::Profile { cmd }) => profile_command(&dirs, cmd),
        Some(Command::Purge { yes }) => purge_command(&dirs, yes),
        Some(Command::Ctl { cmd }) => ctl::run(&profile, cli.team.as_deref(), cmd).await,
        None => {
            let mut p = Profile::read_or_new(&session_file)?;
            // No sign-ins yet: the guide first (in a terminal), then the TUI if it saved any.
            if p.workspaces.is_empty() {
                if !pick::interactive() {
                    bail!("profile {profile} has no sign-ins — klack auth login");
                }
                if auth::guide(&profile).await? == 0 {
                    return Ok(());
                }
                if !logging {
                    init_logging(&profile)?;
                }
                p = Profile::load(&profile)?;
            }
            // Sessions that have ended are offered a fix (or skipped) before the app starts.
            let Some((p, signed_out)) = auth::at_start(&profile, p).await? else { return Ok(()) };
            // Held while klack runs so the profile is not removed under it; a second klack on the
            // same profile runs without it.
            let _lock = profiles::lock(&dirs, &profile).ok().flatten();
            tui::event_loop::run(p, cli.team.as_deref(), signed_out).await
        }
    }
}

/// `klack purge`: lists, asks, then deletes klack's folders and temporary leftovers.
fn purge_command(dirs: &Dirs, yes: bool) -> Result<()> {
    let running = profiles::running_profiles(dirs);
    if !running.is_empty() {
        bail!("klack is running (profile {}) — quit it first", running.join(", "));
    }
    let targets = profiles::purge_targets(dirs, &Dirs::old_system(), &std::env::temp_dir());
    if targets.is_empty() {
        println!("nothing to delete");
        return Ok(());
    }
    eprintln!("This deletes everything klack wrote (files you saved from messages stay):");
    for path in &targets {
        eprintln!("  {}", path.display());
    }
    if !auth::confirm("Delete all of this?", yes)? {
        println!("nothing deleted");
        return Ok(());
    }
    let mut failed = 0;
    for (path, result) in profiles::purge(&targets) {
        match result {
            Ok(()) => println!("deleted {}", path.display()),
            Err(e) => {
                failed += 1;
                eprintln!("could not delete {}: {e}", path.display());
            }
        }
    }
    if failed > 0 {
        bail!("{failed} of {} could not be deleted (see above); quit what uses them and run purge again", targets.len());
    }
    Ok(())
}

/// `klack profile …`: reads and removes local files only.
fn profile_command(dirs: &Dirs, cmd: ProfileCmd) -> Result<()> {
    match cmd {
        ProfileCmd::List => {
            let names = profiles::list(dirs);
            if names.is_empty() {
                println!("no profiles yet — klack auth login");
            }
            let default = profiles::default_name(dirs);
            for name in names {
                println!("{} {name}", if name == default { '★' } else { ' ' });
                match Profile::read(&dirs.config.join(&name).join("session.json")) {
                    Ok(p) if p.workspaces.is_empty() => println!("    no sign-ins"),
                    Ok(p) => {
                        for w in &p.workspaces {
                            println!("    {} (as {})", w.team_name, w.user_name);
                        }
                    }
                    Err(e) => println!("    cannot read its session.json: {e:#}"),
                }
            }
            Ok(())
        }
        ProfileCmd::Default { name } => {
            profiles::set_default(dirs, &name)?;
            println!("plain klack now uses profile {name}");
            Ok(())
        }
        ProfileCmd::Remove { name, yes } => {
            let gone = profiles::removable(dirs, &name)?;
            eprintln!("This deletes (drafts and sidebar layout stay):");
            for path in &gone {
                eprintln!("  {}", path.display());
            }
            if !auth::confirm(&format!("Remove profile {name}?"), yes)? {
                println!("nothing removed");
                return Ok(());
            }
            for path in profiles::remove(dirs, &name)? {
                println!("removed {}", path.display());
            }
            Ok(())
        }
    }
}

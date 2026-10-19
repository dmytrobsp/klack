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
    /// Print a locale file with every translatable string (save it as
    /// locales/<code>.toml next to settings.toml and fill in the translations).
    LocaleTemplate,
    /// Print a complete theme file based on a built-in theme (dark, light, terminal).
    ThemeTemplate {
        #[arg(default_value = "dark")]
        base: String,
    },
    /// Print a complete keys.toml with every default binding, commented (save it as
    /// keys.toml next to settings.toml and change what you want).
    KeysTemplate,
    /// Check a theme file and install it (as themes/<file name>), for F6 → Appearance.
    ThemeImport { path: std::path::PathBuf },
    /// List conversations (`-` marks public channels you are not in).
    Channels,
    /// Join a public channel.
    Join { channel: String },
    /// Leave a channel.
    Leave { channel: String },
    /// Create a channel (lowercase, spaces become hyphens).
    Create {
        name: String,
        #[arg(long)]
        private: bool,
    },
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
    /// Join a conversation's huddle with audio for N seconds: optionally send a tone and
    /// record what arrives (48 kHz mono s16le).
    HuddleMedia {
        channel: String,
        #[arg(default_value_t = 20)]
        seconds: u64,
        #[arg(long)]
        tone: Option<f64>,
        /// With --tone: stop the tone this many seconds after connecting and stay (quiet).
        #[arg(long)]
        tone_for: Option<u64>,
        #[arg(long)]
        record: Option<std::path::PathBuf>,
        /// Send the default microphone (via the same capture path as the TUI).
        #[arg(long)]
        mic: bool,
        /// Send a synthetic camera: a moving test pattern (640×360, 15 fps, H.264).
        #[arg(long)]
        video_test: bool,
        /// With --video-test: switch the camera off after this many seconds and stay.
        #[arg(long)]
        video_for: Option<u64>,
        /// Save others' video into this folder: a PNG per person every 2 seconds.
        #[arg(long)]
        record_video: Option<std::path::PathBuf>,
        /// Also share a synthetic screen: the test pattern from a second attendee
        /// (`<attendee>#content`), the way Slack shares a screen.
        #[arg(long)]
        share_test: bool,
        /// Send this camera (part of its name or path; see `ctl cameras`), switched on once
        /// connected. Only the named camera is ever opened.
        #[arg(long)]
        camera: Option<String>,
        /// Share your screen: the desktop asks what to share (nothing is captured before you
        /// choose).
        #[arg(long)]
        share_screen: bool,
    },
    /// List audio input/output devices (names usable in KLACK_AUDIO_INPUT/OUTPUT).
    AudioDevices,
    /// List cameras (names usable with `huddle-media --camera` and in the preferences).
    Cameras,
    /// Edit a message (`chat.update`).
    Edit { channel: String, ts: String, text: String },
    /// Delete a message (`chat.delete`).
    Delete { channel: String, ts: String },
    /// Send typing indicators over RTM for N seconds (every 3 s), in a thread with --thread.
    Typing {
        channel: String,
        #[arg(default_value_t = 10)]
        seconds: u64,
        #[arg(long)]
        thread: Option<String>,
        /// Send the web client's frame (`user_typing` with `thread_ts`) instead of RTM's `typing`.
        #[arg(long)]
        web: bool,
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
        /// Post it as a text snippet in this language (e.g. rust, python, text).
        #[arg(long)]
        snippet: Option<String>,
        /// Upload into the thread under this message.
        #[arg(long)]
        thread: Option<String>,
    },
    /// Print live events for N seconds.
    Listen {
        #[arg(default_value_t = 30)]
        seconds: u64,
    },
}

const ALL_TYPES: &str = "public_channel,private_channel,mpim,im";

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
    let convs: Vec<Conversation> = client.conversations_list(ALL_TYPES).await?;
    convs.into_iter().find(|c| c.name == name).map(|c| c.id).ok_or_else(|| anyhow!("no channel {spec}"))
}

pub async fn run(profile: &str, team: Option<&str>, cmd: Ctl) -> Result<()> {
    let client = client(profile, team)?;
    match cmd {
        Ctl::LocaleTemplate => print!("{}", crate::tui::i18n::template()),
        Ctl::KeysTemplate => print!("{}", crate::tui::keys_file::template(&crate::tui::keymap::Keymap::load("").0)),
        Ctl::ThemeTemplate { base } => print!("{}", crate::tui::theme::template(&base).map_err(anyhow::Error::msg)?),
        Ctl::ThemeImport { path } => {
            let name = path.file_stem().and_then(|n| n.to_str()).ok_or_else(|| anyhow!("bad file name"))?.to_string();
            let text = std::fs::read_to_string(&path)?;
            crate::tui::theme::parse(&name, &text).map_err(|e| anyhow!("{}: {e}", path.display()))?;
            let dir = crate::tui::theme::dir().ok_or_else(|| anyhow!("no config directory"))?;
            std::fs::create_dir_all(&dir)?;
            let target = dir.join(format!("{name}.toml"));
            std::fs::write(&target, text)?;
            println!("installed theme '{name}' at {} — choose it in F6 → Appearance", target.display());
        }
        Ctl::Whoami => {
            let a = client.auth_test().await?;
            println!("{} ({}) in {} ({}) — {}", a.user, a.user_id, a.team, a.team_id, a.url);
        }
        Ctl::Channels => {
            for c in client.conversations_list(ALL_TYPES).await? {
                let kind = if c.is_im { "dm" } else if c.is_mpim { "group-dm" } else if c.is_private { "private" } else { "channel" };
                let member = if c.is_dm() || c.is_member { ' ' } else { '-' };
                println!("{:<12} {:<9} {member} {}", c.id, kind, if c.name.is_empty() { c.user.unwrap_or_default() } else { c.name });
            }
        }
        Ctl::Join { channel } => {
            let c = client.join(&resolve_channel(&client, &channel).await?).await?;
            println!("joined #{} ({})", c.name, c.id);
        }
        Ctl::Leave { channel } => client.leave(&resolve_channel(&client, &channel).await?).await?,
        Ctl::Create { name, private } => {
            let c = client.create_channel(&slack_core::types::channel_name(&name), private).await?;
            println!("created #{} ({})", c.name, c.id);
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
        Ctl::HuddleMedia { channel, seconds, tone, tone_for, record, mic, video_test, video_for, record_video, share_test, camera, share_screen } => {
            // Resolve the camera before joining, so a wrong name fails early.
            let mut camera = match camera {
                Some(wanted) => Some(huddle::camera::find_camera(&huddle::camera::list_cameras(), &wanted)?),
                None => None,
            };
            let mut capture: Option<huddle::camera::Camera> = None;
            use huddle::session::{FRAME_SAMPLES, Options, SAMPLE_RATE, Session, SessionEvent};
            let id = resolve_channel(&client, &channel).await?;
            let creds = huddle::signaling::Credentials::from_rooms_join(&client.call_raw("rooms.join", &[("channel_id", id)]).await?)?;
            println!("room {} attendee {}", creds.room_id, creds.attendee.attendee_id);
            let share = (share_test || share_screen).then(|| creds.content_share());
            let mut screen: Option<huddle::screen::ScreenCapture> = None;
            let mut session = Session::start(creds, Options { muted: false, send_video: video_test, receive_video: true });
            let sink = session.pcm_sink();
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let sent = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
            // The test camera has its own stop, so it can end while the session stays.
            let camera_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            if video_test {
                spawn_test_video(session.video_sink(), camera_stop.clone(), sent.clone())?;
            }
            let video_off = tokio::time::sleep(std::time::Duration::from_secs(video_for.filter(|_| video_test).unwrap_or(u64::MAX / 4)));
            tokio::pin!(video_off);
            // The screen share is a second attendee that only sends video.
            let share = share.map(|c| Session::start(c, Options { muted: true, send_video: true, receive_video: false }));
            let share = match share {
                Some(mut s) => {
                    if share_screen {
                        use huddle::screen::{SHARE_FPS, SHARE_HEIGHT, SHARE_WIDTH};
                        let mut encoder = huddle::video::H264Encoder::for_screen(SHARE_WIDTH, SHARE_HEIGHT, SHARE_FPS, 1_500_000)?;
                        let (video, sent) = (s.video_sink(), sent.clone());
                        let period = std::time::Duration::from_secs_f64(1.0 / SHARE_FPS as f64);
                        let asks = huddle::screen::Source::current().is_some_and(|s| s.asks_what_to_share());
                        println!("{}", if asks { "choose what to share in the desktop's dialog…" } else { "sharing the whole screen…" });
                        let (capture, what) = huddle::screen::ScreenCapture::start(SHARE_WIDTH, SHARE_HEIGHT, SHARE_FPS, move |rgb| {
                            if video.keyframe_wanted() {
                                encoder.request_keyframe();
                            }
                            match encoder.encode_rgb(&rgb) {
                                Ok(unit) => {
                                    sent.fetch_add(unit.len() as u64, std::sync::atomic::Ordering::Relaxed);
                                    video.send(unit, period);
                                }
                                Err(e) => eprintln!("{e}"),
                            }
                        })
                        .await?;
                        println!("sharing: {what}");
                        screen = Some(capture);
                    } else {
                        spawn_test_video(s.video_sink(), stop.clone(), sent.clone())?;
                    }
                    // It hears the meeting too; nobody listens.
                    s.audio = tokio::sync::mpsc::unbounded_channel().1;
                    let mut events = std::mem::replace(&mut s.events, tokio::sync::mpsc::unbounded_channel().1);
                    let (done_tx, done) = tokio::sync::oneshot::channel::<()>();
                    tokio::spawn(async move {
                        while let Some(ev) = events.recv().await {
                            match ev {
                                SessionEvent::Ended => break,
                                SessionEvent::Levels(_) | SessionEvent::Roster(_) | SessionEvent::VideoSources(_) => {}
                                ev => println!("share: {ev:?}"),
                            }
                        }
                        let _ = done_tx.send(());
                    });
                    Some((s, done))
                }
                None => None,
            };
            let _mic = if mic { Some(huddle::devices::Microphone::start(std::env::var(huddle::devices::INPUT_ENV).ok(), move |frame| sink.send(frame))?) } else { None };
            let mut file = match &record {
                Some(p) => Some(std::fs::File::create(p)?),
                None => None,
            };
            let mut tick = tokio::time::interval(std::time::Duration::from_millis(20));
            let deadline = tokio::time::sleep(std::time::Duration::from_secs(seconds));
            tokio::pin!(deadline);
            let (mut phase, mut received, mut sum_sq) = (0u64, 0usize, 0f64);
            // Pictures received per attendee, and when each was last saved.
            let mut pictures: std::collections::BTreeMap<String, (u64, usize, usize, Option<std::time::Instant>)> = Default::default();
            // When media connected, to report how long each first picture took.
            let mut connected_at: Option<std::time::Instant> = None;
            loop {
                tokio::select! {
                    _ = &mut deadline => break,
                    _ = &mut video_off, if !camera_stop.load(std::sync::atomic::Ordering::Relaxed) && video_for.is_some() => {
                        camera_stop.store(true, std::sync::atomic::Ordering::Relaxed);
                        session.set_camera(false);
                        println!("camera off (staying in the huddle)");
                    }
                    _ = tick.tick() => {
                        let tone_on = tone_for.is_none_or(|s| connected_at.is_none_or(|c| c.elapsed() < std::time::Duration::from_secs(s)));
                        if let Some(hz) = tone.filter(|_| tone_on) {
                            let frame: Vec<i16> = (0..FRAME_SAMPLES).map(|i| (((phase + i as u64) as f64 * hz * std::f64::consts::TAU / SAMPLE_RATE as f64).sin() * 1500.0) as i16).collect();
                            phase += FRAME_SAMPLES as u64;
                            session.send_pcm(frame);
                        }
                    }
                    ev = session.events.recv() => match ev {
                        Some(SessionEvent::Ended) | None => { println!("session ended"); break; }
                        Some(SessionEvent::Levels(_)) => {}
                        Some(SessionEvent::Connected) => {
                            println!("Connected");
                            connected_at = Some(std::time::Instant::now());
                            if let Some(info) = camera.take() {
                                session.set_camera(true);
                                capture = Some(start_camera(&info, session.video_sink(), sent.clone())?);
                                println!("camera on: {} ({})", info.name, info.id);
                            }
                        }
                        Some(ev) => println!("{ev:?}"),
                    },
                    Some(frame) = session.video.recv() => {
                        let entry = pictures.entry(frame.attendee_id.clone()).or_insert((0, 0, 0, None));
                        entry.0 += 1;
                        (entry.1, entry.2) = (frame.width, frame.height);
                        if entry.0 == 1 {
                            let after = connected_at.map(|t| format!(" {} ms after connecting", t.elapsed().as_millis())).unwrap_or_default();
                            println!("first picture from {}{after}", frame.attendee_id);
                        }
                        if let Some(dir) = &record_video {
                            if entry.3.is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(2)) {
                                entry.3 = Some(std::time::Instant::now());
                                std::fs::create_dir_all(dir)?;
                                let who = if frame.attendee_id.is_empty() { "unknown".to_string() } else { frame.attendee_id.chars().take(8).collect() };
                                let path = dir.join(format!("{who}-{:04}.png", entry.0));
                                image::RgbImage::from_raw(frame.width as u32, frame.height as u32, frame.rgb.to_vec()).ok_or_else(|| anyhow::anyhow!("bad frame size"))?.save(&path)?;
                            }
                        }
                    }
                    Some(pcm) = session.audio.recv() => {
                        received += pcm.len();
                        sum_sq += pcm.iter().map(|s| (*s as f64).powi(2)).sum::<f64>();
                        if let Some(f) = file.as_mut() {
                            use std::io::Write;
                            let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
                            f.write_all(&bytes)?;
                        }
                    }
                }
            }
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            camera_stop.store(true, std::sync::atomic::Ordering::Relaxed);
            drop(capture);
            drop(screen);
            if video_test || share_test || share_screen || camera.is_some() {
                println!("encoded {} kB of video", sent.load(std::sync::atomic::Ordering::Relaxed) / 1000);
            }
            for (who, (n, w, h, _)) in &pictures {
                println!("video from {who}: {n} pictures, {w}×{h}");
            }
            if let Some((s, done)) = share {
                s.leave();
                let _ = tokio::time::timeout(std::time::Duration::from_secs(5), done).await;
            }
            session.leave();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async { while let Some(ev) = session.events.recv().await { if matches!(ev, SessionEvent::Ended) { break; } } }).await;
            let rms = if received > 0 { (sum_sq / received as f64).sqrt() } else { 0.0 };
            println!("received {:.1} s of audio, rms {rms:.0}", received as f64 / SAMPLE_RATE as f64);
        }
        Ctl::Edit { channel, ts, text } => {
            let id = resolve_channel(&client, &channel).await?;
            client.update_message(&id, &ts, &text).await?;
            println!("edited {ts}");
        }
        Ctl::Delete { channel, ts } => {
            let id = resolve_channel(&client, &channel).await?;
            client.delete_message(&id, &ts).await?;
            println!("deleted {ts}");
        }
        Ctl::Typing { channel, seconds, thread, web } => {
            let id = resolve_channel(&client, &channel).await?;
            let mut rtm = slack_core::rtm::spawn(client.clone());
            // Wait for the socket before typing.
            while let Some(ev) = rtm.events.recv().await {
                if matches!(ev, slack_core::rtm::RtmEvent::Connected) {
                    break;
                }
            }
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(seconds);
            while tokio::time::Instant::now() < deadline {
                let cmd = if web {
                    slack_core::rtm::RtmCommand::Raw(serde_json::json!({"type": "user_typing", "channel": id, "thread_ts": thread}))
                } else {
                    slack_core::rtm::RtmCommand::Typing { channel: id.clone(), thread_ts: thread.clone() }
                };
                rtm.commands.send(cmd)?;
                println!("typing");
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            }
        }
        Ctl::Cameras => {
            for c in huddle::camera::list_cameras() {
                println!("{}  {}", c.id, c.name);
            }
        }
        Ctl::AudioDevices => {
            let (inputs, outputs) = huddle::devices::list_devices()?;
            println!("inputs (KLACK_AUDIO_INPUT):");
            inputs.iter().for_each(|d| println!("  {d}"));
            println!("outputs (KLACK_AUDIO_OUTPUT):");
            outputs.iter().for_each(|d| println!("  {d}"));
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
            let v = client.search_messages(&query, 20, 1).await?;
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
        Ctl::Upload { channel, path, comment, snippet, thread } => {
            let id = resolve_channel(&client, &channel).await?;
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).ok_or_else(|| anyhow!("bad path"))?;
            let bytes = std::fs::read(&path)?;
            let file_id = client.upload(&id, &name, bytes, comment.as_deref(), thread.as_deref(), snippet.as_deref()).await?;
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

/// Encode the test pattern on its own thread, paced at its frame rate, into `video` until
/// `stop`; `sent` counts the bytes.
fn spawn_test_video(video: huddle::session::VideoSink, stop: std::sync::Arc<std::sync::atomic::AtomicBool>, sent: std::sync::Arc<std::sync::atomic::AtomicU64>) -> Result<()> {
    use huddle::video::{H264Encoder, TEST_FPS, TEST_HEIGHT, TEST_WIDTH, test_pattern};
    let mut encoder = H264Encoder::new(TEST_WIDTH, TEST_HEIGHT, TEST_FPS, 600_000)?;
    std::thread::spawn(move || {
        let period = std::time::Duration::from_secs_f64(1.0 / TEST_FPS as f64);
        let start = std::time::Instant::now();
        for n in 0u64.. {
            if stop.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            if video.keyframe_wanted() {
                encoder.request_keyframe();
            }
            match encoder.encode_rgb(&test_pattern(n, TEST_WIDTH, TEST_HEIGHT)) {
                Ok(au) => {
                    sent.fetch_add(au.len() as u64, std::sync::atomic::Ordering::Relaxed);
                    video.send(au, period);
                }
                Err(e) => eprintln!("{e}"),
            }
            std::thread::sleep((start + period * (n as u32 + 1)).saturating_duration_since(std::time::Instant::now()));
        }
    });
    Ok(())
}

/// Capture `camera` at the test size and rate, encode it and feed `video`; `sent` counts bytes.
fn start_camera(camera: &huddle::camera::CameraInfo, video: huddle::session::VideoSink, sent: std::sync::Arc<std::sync::atomic::AtomicU64>) -> Result<huddle::camera::Camera> {
    use huddle::video::{H264Encoder, TEST_FPS, TEST_HEIGHT, TEST_WIDTH};
    let mut encoder = H264Encoder::new(TEST_WIDTH, TEST_HEIGHT, TEST_FPS, 600_000)?;
    let period = std::time::Duration::from_secs_f64(1.0 / TEST_FPS as f64);
    huddle::camera::Camera::start(camera, TEST_WIDTH, TEST_HEIGHT, TEST_FPS, move |rgb| {
        if video.keyframe_wanted() {
            encoder.request_keyframe();
        }
        match encoder.encode_rgb(&rgb) {
            Ok(au) => {
                sent.fetch_add(au.len() as u64, std::sync::atomic::Ordering::Relaxed);
                video.send(au, period);
            }
            Err(e) => eprintln!("{e}"),
        }
    })
}

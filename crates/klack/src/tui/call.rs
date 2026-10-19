//! The huddle you are in: session state, audio devices and the call bar.


use huddle::devices::{EchoCanceller, MicConfig, MicProcessor, Microphone, Speaker, Volume};
use huddle::session::{Participant, PcmSink, Session, SessionEvent};
use huddle::signaling::Credentials;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui_image::sliced::{SignedPosition, SlicedImage};
use slack_core::types::File;

use super::media::{Media, Thumb};
use super::state::Workspace;

pub struct Call {
    pub channel: String,
    /// The thread the huddle was started in, if any.
    pub thread: Option<String>,
    /// Microphone to open when unmuting (None: the default).
    input: Option<String>,
    pub session: Session,
    pub roster: Vec<Participant>,
    /// Speaking levels (0–1) by attendee id, from the meeting.
    pub levels: std::collections::HashMap<String, f32>,
    /// Who of the others is talking, or talked last (attendee id), for the call bar.
    last_speaker: Option<String>,
    /// Our own microphone level while unmuted (f32 bits), measured locally.
    mic_level: std::sync::Arc<std::sync::atomic::AtomicU32>,
    /// Whether the speaking threshold lets our microphone through.
    mic_open: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Gain, threshold, noise suppression and echo cancellation, read by the microphone for
    /// each frame.
    mic_config: std::sync::Arc<std::sync::Mutex<MicConfig>>,
    /// Fed with what the speaker plays, used on the microphone while unmuted.
    echo: EchoCanceller,
    /// Playback volume of the meeting (None without a speaker).
    volume: Option<Volume>,
    pub muted: bool,
    pub connected: bool,
    /// We asked to leave (the session ends next).
    pub leaving: bool,
    /// When media connected: roster changes in the first moments are the people already
    /// there arriving, not joins.
    connected_at: Option<std::time::Instant>,
    /// People who joined / left since the last [`Call::take_roster_changes`].
    joined: usize,
    left: usize,
    /// Since when we are the only person here, and the melody looping since the wait ran out
    /// (stopped when someone joins or the call ends).
    alone_since: Option<std::time::Instant>,
    alone_loop: Option<super::sound::Looping>,
    alone_started: bool,
    sink: PcmSink,
    /// Open only while unmuted, so a muted call never touches the microphone.
    mic: Option<Microphone>,
    /// Why there is no sound, if the speaker could not be opened.
    pub speaker_error: Option<String>,
    /// The latest picture of each participant's video and when it came, by attendee id.
    pub videos: Videos,
    /// Screen shares going on: (the share's attendee id, the sharer's user id). A share is a
    /// second attendee (`<attendee>#content`) that only shows up among the video sources.
    pub shares: Vec<(String, String)>,
    /// Our attendee id (our own picture is kept under it).
    self_attendee: String,
    /// Open only while the camera is on.
    camera: Option<huddle::camera::Camera>,
    /// The meeting credentials (a screen share joins with derived ones).
    pub creds: Credentials,
    /// Our screen share, while it runs.
    share: Option<ScreenShare>,
    /// The desktop is asking what to share.
    pub share_pending: bool,
}

/// Our running screen share: a second session (`<attendee>#content`) sending the capture.
pub struct ScreenShare {
    session: Session,
    _capture: huddle::screen::ScreenCapture,
    /// What is shared ("monitor 2560×1440").
    pub what: String,
}

/// Ask the desktop what to share (waits for the user), then join as our share attendee and
/// send it; our own picture of it is kept in `videos` for our share tile. The share session
/// joins only once something was chosen, so others never see an empty share.
pub async fn start_share(creds: Credentials, videos: Videos) -> anyhow::Result<ScreenShare> {
    use huddle::screen::{SHARE_FPS as FPS, SHARE_HEIGHT as H, SHARE_WIDTH as W};
    // Capture → encoder thread; a slow encoder drops pictures rather than queueing them.
    let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(2);
    let (capture, what) = huddle::screen::ScreenCapture::start(W, H, FPS, move |rgb| {
        let _ = tx.try_send(rgb);
    })
    .await?;
    let creds = creds.content_share();
    let me = creds.attendee.attendee_id.clone();
    let mut session = Session::start(creds, huddle::session::Options { muted: true, send_video: true, receive_video: false });
    // It hears the meeting too; nobody listens.
    session.audio = tokio::sync::mpsc::unbounded_channel().1;
    let mut events = std::mem::replace(&mut session.events, tokio::sync::mpsc::unbounded_channel().1);
    tokio::spawn(async move {
        while let Some(ev) = events.recv().await {
            if let SessionEvent::Error(e) = ev {
                tracing::warn!("screen share: {e}");
            }
        }
    });
    let sink = session.video_sink();
    let mut encoder = huddle::video::H264Encoder::for_screen(W, H, FPS, 1_500_000)?;
    let period = std::time::Duration::from_secs_f64(1.0 / FPS as f64);
    std::thread::spawn(move || {
        for rgb in rx {
            if sink.keyframe_wanted() {
                encoder.request_keyframe();
            }
            match encoder.encode_rgb(&rgb) {
                Ok(unit) => sink.send(unit, period),
                Err(e) => tracing::warn!("screen share: {e}"),
            }
            let frame = huddle::session::VideoFrame { attendee_id: me.clone(), stream_id: 0, width: W, height: H, rgb: std::sync::Arc::new(rgb) };
            videos.lock().unwrap_or_else(|e| e.into_inner()).insert(me.clone(), (std::time::Instant::now(), frame));
        }
    });
    Ok(ScreenShare { session, _capture: capture, what })
}

pub type Videos = std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, huddle::session::VideoFrame)>>>;

/// A picture older than this means the video has stopped.
pub const VIDEO_STALE: std::time::Duration = std::time::Duration::from_secs(3);

/// The screen shares among the video sources: (share attendee id, sharer's user id).
pub fn screen_shares(sources: &[huddle::session::VideoSource]) -> Vec<(String, String)> {
    let mut shares: Vec<(String, String)> = sources
        .iter()
        .filter(|s| huddle::signaling::is_content(&s.attendee_id))
        .map(|s| (s.attendee_id.clone(), s.external_user_id.rsplit('-').next().unwrap_or_default().to_string()))
        .collect();
    shares.sort();
    shares.dedup();
    shares
}

/// A four-bar level meter ("▂▄▆█" when loud, "▂···" when quiet).
pub fn meter(level: f32) -> String {
    const BARS: [char; 4] = ['▂', '▄', '▆', '█'];
    const STEPS: [f32; 4] = [0.02, 0.3, 0.55, 0.8];
    BARS.iter().zip(STEPS).map(|(b, s)| if level >= s { *b } else { '·' }).collect()
}

impl Call {
    /// Start the session (muted) and play the meeting's audio on the default output.
    /// `devices`: (microphone, speaker) names, None for the system default.
    pub fn start(channel: String, creds: Credentials, devices: (Option<String>, Option<String>), audio_settings: &super::settings::Audio) -> Call {
        let self_attendee = creds.attendee.attendee_id.clone();
        let creds_kept = creds.clone();
        let mut session = Session::start(creds, Default::default());
        let sink = session.pcm_sink();
        let (_, empty) = tokio::sync::mpsc::unbounded_channel();
        let mut audio = std::mem::replace(&mut session.audio, empty);
        // Keep only the latest picture per person (decoding already happened off this task).
        let (_, no_video) = tokio::sync::mpsc::unbounded_channel();
        let mut video = std::mem::replace(&mut session.video, no_video);
        let videos: Videos = Default::default();
        let latest = videos.clone();
        tokio::spawn(async move {
            while let Some(frame) = video.recv().await {
                latest.lock().unwrap_or_else(|e| e.into_inner()).insert(frame.attendee_id.clone(), (std::time::Instant::now(), frame));
            }
        });
        let echo = EchoCanceller::new();
        let (volume, speaker_error) = match Speaker::start(devices.1) {
            Ok(mut speaker) => {
                let volume = speaker.volume();
                let (echo, vol) = (echo.clone(), volume.clone());
                tokio::spawn(async move {
                    while let Some(frame) = audio.recv().await {
                        speaker.play(&frame);
                        // The canceller's reference: the meeting as loud as it is played.
                        let v = vol.get();
                        let played: Vec<i16> = frame.iter().map(|s| (*s as f32 * v).clamp(i16::MIN as f32, i16::MAX as f32) as i16).collect();
                        echo.render(&played);
                    }
                });
                (Some(volume), None)
            }
            Err(e) => {
                tracing::warn!("no speaker: {e}");
                (None, Some(e.to_string()))
            }
        };
        let call = Call {
            channel,
            thread: None,
            input: devices.0,
            session,
            roster: Vec::new(),
            levels: Default::default(),
            last_speaker: None,
            mic_level: Default::default(),
            mic_open: Default::default(),
            mic_config: Default::default(),
            echo,
            volume,
            muted: true,
            connected: false,
            leaving: false,
            connected_at: None,
            joined: 0,
            left: 0,
            alone_since: None,
            alone_loop: None,
            alone_started: false,
            sink,
            mic: None,
            speaker_error,
            videos,
            shares: Vec::new(),
            self_attendee,
            camera: None,
            creds: creds_kept,
            share: None,
            share_pending: false,
        };
        call.set_audio(audio_settings);
        call
    }

    /// Apply the volume and microphone settings to the running call.
    pub fn set_audio(&self, audio: &super::settings::Audio) {
        if let Some(v) = &self.volume {
            v.set(audio.huddle_volume as f32 / 100.0);
        }
        *self.mic_config.lock().unwrap_or_else(|e| e.into_inner()) = audio.mic_config();
    }

    /// Toggle the microphone; opening it can fail (no device, no permission).
    pub fn toggle_mute(&mut self) -> anyhow::Result<()> {
        if self.muted {
            let sink = self.sink.clone();
            let (level, open, config) = (self.mic_level.clone(), self.mic_open.clone(), self.mic_config.clone());
            let mut processor = MicProcessor::with_echo_canceller(self.echo.clone());
            self.mic = Some(Microphone::start(self.input.clone(), move |mut frame| {
                let cfg = *config.lock().unwrap_or_else(|e| e.into_inner());
                let (l, o) = processor.process(&mut frame, &cfg);
                level.store(l.to_bits(), std::sync::atomic::Ordering::Relaxed);
                open.store(o, std::sync::atomic::Ordering::Relaxed);
                sink.send(frame)
            })?);
            self.muted = false;
        } else {
            self.mic = None;
            self.muted = true;
        }
        self.session.set_muted(self.muted);
        Ok(())
    }

    pub fn camera_on(&self) -> bool {
        self.camera.is_some()
    }

    /// Switch the camera on or off; `wanted` is the camera chosen in the preferences (only
    /// that one is ever opened). Our own picture shows in our tile while it is on.
    pub fn toggle_camera(&mut self, wanted: Option<&str>) -> anyhow::Result<String> {
        use huddle::video::{H264Encoder, TEST_FPS as FPS, TEST_HEIGHT as H, TEST_WIDTH as W};
        if self.camera.take().is_some() {
            self.session.set_camera(false);
            self.videos.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.self_attendee);
            return Ok("camera off".into());
        }
        let wanted = wanted.filter(|w| !w.trim().is_empty()).ok_or_else(|| anyhow::anyhow!("no camera chosen (F6 → Audio & video → Camera)"))?;
        let info = huddle::camera::find_camera(&huddle::camera::list_cameras(), wanted)?;
        let mut encoder = H264Encoder::new(W, H, FPS, 600_000)?;
        let (sink, videos, me) = (self.session.video_sink(), self.videos.clone(), self.self_attendee.clone());
        let period = std::time::Duration::from_secs_f64(1.0 / FPS as f64);
        let camera = huddle::camera::Camera::start(&info, W, H, FPS, move |rgb| {
            if sink.keyframe_wanted() {
                encoder.request_keyframe();
            }
            match encoder.encode_rgb(&rgb) {
                Ok(unit) => sink.send(unit, period),
                Err(e) => tracing::warn!("camera: {e}"),
            }
            let frame = huddle::session::VideoFrame { attendee_id: me.clone(), stream_id: 0, width: W, height: H, rgb: std::sync::Arc::new(rgb) };
            videos.lock().unwrap_or_else(|e| e.into_inner()).insert(me.clone(), (std::time::Instant::now(), frame));
        })?;
        self.session.set_camera(true);
        self.camera = Some(camera);
        Ok(format!("camera on: {}", info.name))
    }

    pub fn leave(&mut self) {
        self.leaving = true;
        self.mic = None;
        self.camera = None;
        self.stop_share();
        self.session.leave();
    }

    /// Apply a session event; true once the session has ended.
    pub fn apply(&mut self, ev: SessionEvent) -> bool {
        match ev {
            SessionEvent::Connected => {
                self.connected = true;
                self.connected_at = Some(std::time::Instant::now());
            }
            // A screen share joins as a second attendee: it is shown as a share, not a person.
            SessionEvent::Roster(r) => {
                let roster: Vec<Participant> = r.into_iter().filter(|p| !huddle::signaling::is_content(&p.attendee_id)).collect();
                if self.connected_at.is_some_and(|t| t.elapsed() >= ROSTER_SETTLE) {
                    let (joined, left) = roster_changes(&self.roster, &roster, &self.self_attendee);
                    self.joined += joined;
                    self.left += left;
                }
                self.roster = roster;
                let alone = self.roster.iter().all(|p| p.attendee_id == self.self_attendee);
                if !alone {
                    (self.alone_since, self.alone_loop, self.alone_started) = (None, None, false);
                } else if self.alone_since.is_none() {
                    self.alone_since = Some(std::time::Instant::now());
                }
            }
            SessionEvent::Levels(l) => {
                self.levels = l.into_iter().collect();
                if let Some(id) = loudest_other(&self.levels, &self.roster, &self.self_attendee) {
                    self.last_speaker = Some(id);
                }
            }
            SessionEvent::Error(e) => tracing::warn!("huddle: {e}"),
            SessionEvent::VideoSources(sources) => self.shares = screen_shares(&sources),
            SessionEvent::SendingVideo { .. } => {}
            SessionEvent::ReceiveCodecs(c) => tracing::info!("meeting receive codecs: {c:?}"),
            SessionEvent::Ended => return true,
        }
        false
    }

    /// Start looping `sound` once we have been alone for `wait` (once per time alone).
    pub fn alone_tick(&mut self, wait: std::time::Duration, sound: impl FnOnce() -> Option<super::sound::Looping>) -> bool {
        if self.alone_started || !alone_long_enough(self.alone_since, std::time::Instant::now(), wait) {
            return false;
        }
        self.alone_started = true;
        self.alone_loop = sound();
        true
    }

    /// People who joined and left since the last call (for the huddle sounds).
    pub fn take_roster_changes(&mut self) -> (usize, usize) {
        (std::mem::take(&mut self.joined), std::mem::take(&mut self.left))
    }

    /// Where the huddle is: "#social", a DM partner, with " · thread" for a thread's huddle.
    pub fn place(&self, ws: &Workspace) -> String {
        let place = ws.conversations.get(&self.channel).map(|c| if c.is_dm() { ws.conversation_label(c) } else { format!("#{}", c.name) }).unwrap_or_else(|| self.channel.clone());
        if self.thread.is_some() { format!("{place} · thread") } else { place }
    }

    /// A participant's (level, speaking, muted, gated): our own level comes from the
    /// microphone at once, others' from the meeting; `gated`: our microphone is below the
    /// speaking threshold, so nothing is sent.
    pub fn participant_state(&self, p: &Participant, self_id: &str) -> (f32, bool, bool, bool) {
        let me = p.user_id() == self_id;
        let level = if me && !self.muted { f32::from_bits(self.mic_level.load(std::sync::atomic::Ordering::Relaxed)) } else { self.levels.get(&p.attendee_id).copied().unwrap_or(0.0) };
        let gated = me && !self.muted && !self.mic_open.load(std::sync::atomic::Ordering::Relaxed);
        let muted = p.muted || (me && self.muted);
        (level, level >= huddle::session::SPEAKING && !gated && !muted, muted, gated)
    }

    /// Our share's attendee id.
    pub fn share_attendee(&self) -> String {
        format!("{}{}", self.self_attendee, huddle::signaling::CONTENT_MODALITY)
    }

    pub fn is_sharing(&self) -> bool {
        self.share.is_some()
    }

    /// A share that started (from [`start_share`]).
    pub fn set_share(&mut self, share: ScreenShare) {
        self.share = Some(share);
    }

    /// Stop our screen share (the share attendee leaves the meeting).
    pub fn stop_share(&mut self) {
        if let Some(s) = self.share.take() {
            s.session.leave();
            self.videos.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.share_attendee());
        }
    }

    /// The shares to show: the meeting's, and ours while it runs (first).
    pub fn shown_shares(&self, self_user: &str) -> Vec<(String, String)> {
        let mut shares = self.shares.clone();
        let own = self.share_attendee();
        if self.share.is_some() && !shares.iter().any(|(a, _)| *a == own) {
            shares.insert(0, (own, self_user.to_string()));
        }
        shares
    }

    /// The participant's latest picture, if their video is running.
    pub fn video(&self, attendee_id: &str) -> Option<huddle::session::VideoFrame> {
        let videos = self.videos.lock().unwrap_or_else(|e| e.into_inner());
        videos.get(attendee_id).filter(|(at, _)| at.elapsed() < VIDEO_STALE).map(|(_, f)| f.clone())
    }

    /// The call bar's pieces: what is going on (the place, who is talking or talked last,
    /// how many are here, our microphone, camera and share); no key hints, which-key shows those. The avatar, if
    /// any, goes in the first two cells of its piece.
    fn bar(&self, ws: &Workspace, media: &Media) -> [Vec<Piece>; 1] {
        let p = super::theme::p();
        let (green, dim) = (Style::default().fg(p.own), Style::default().fg(p.dim));
        let piece = |text: String, style: Style| Piece { spans: vec![Span::styled(text, style)], avatar: None };
        let mut state = vec![piece(format!("🎧 {}", self.place(ws)), green.add_modifier(Modifier::BOLD))];
        if !self.connected {
            state.push(piece("connecting…".into(), dim));
        }
        let speaker = self.last_speaker.as_deref().and_then(|id| self.roster.iter().find(|p| p.attendee_id == id));
        if let Some(sp) = speaker {
            let (level, speaking, _, _) = self.participant_state(sp, &ws.self_id);
            let name = truncate(&ws.person_label(sp.user_id()), SPEAKER_NAME);
            let mut spans = Vec::new();
            let mut avatar = None;
            if media.avatars && ws.settings.messages.avatars {
                if let Some(file) = ws.users.get(sp.user_id()).and_then(super::media::avatar_file) {
                    // The initial stands in until the picture is there.
                    if !matches!(media.thumbs.get(&file.id), Some(Thumb::Ready(_))) {
                        let initial: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
                        spans.push(Span::styled(format!("{initial:<2}"), Style::default().add_modifier(Modifier::BOLD)));
                    } else {
                        spans.push(Span::raw("  "));
                    }
                    avatar = Some(file);
                }
            }
            if avatar.is_some() {
                spans.push(Span::raw(" "));
            }
            spans.push(Span::styled(name, if speaking { green.add_modifier(Modifier::BOLD) } else { Style::default() }));
            spans.push(Span::styled(format!(" {}", meter(level)), if speaking { green } else { dim }));
            if !speaking {
                spans.push(Span::styled(" talked last", dim));
            }
            state.push(Piece { spans, avatar });
        }
        state.push(piece(people(self.roster.len()), dim));
        let mic = if self.muted {
            piece("🔇 muted".into(), Style::default())
        } else {
            let (level, open) = (f32::from_bits(self.mic_level.load(std::sync::atomic::Ordering::Relaxed)), self.mic_open.load(std::sync::atomic::Ordering::Relaxed));
            Piece { spans: vec![Span::styled("🎙\u{fe0f} live ", green.add_modifier(Modifier::BOLD)), Span::styled(meter(level), if open { green } else { dim })], avatar: None }
        };
        state.push(mic);
        state.push(if self.camera_on() { piece("📹 camera on".into(), green) } else { piece("camera off".into(), dim) });
        if self.is_sharing() {
            state.push(piece("🖥\u{fe0f} sharing your screen".into(), green));
        } else if let Some((_, user)) = self.shares.first() {
            state.push(piece(format!("🖥\u{fe0f} {} is sharing", truncate(&ws.person_label(user), SPEAKER_NAME)), Style::default()));
        }
        if let Some(e) = &self.speaker_error {
            state.push(piece(format!("no sound: {e}"), Style::default().fg(p.danger)));
        }
        [state]
    }

    /// Rows the call bar takes at `width` cells.
    pub fn bar_rows(&self, ws: &Workspace, media: &Media, width: u16) -> u16 {
        let groups = self.bar(ws, media);
        flow(&widths(&groups), width as usize).1.min(BAR_ROWS)
    }

    /// Draw the call bar; returns a profile picture to load, if it is not cached yet.
    pub fn draw(&self, f: &mut Frame, area: Rect, ws: &Workspace, media: &Media) -> Option<(File, (u16, u16))> {
        let groups = self.bar(ws, media);
        let (places, rows) = flow(&widths(&groups), area.width as usize);
        let mut lines: Vec<Vec<Span>> = vec![Vec::new(); rows as usize];
        let mut pictures = Vec::new();
        for (group, places) in groups.into_iter().zip(places) {
            for (piece, (row, col)) in group.into_iter().zip(places) {
                let line = &mut lines[row as usize];
                line.push(Span::styled(if line.is_empty() { " " } else { SEP }, Style::default().fg(super::theme::p().dim)));
                line.extend(piece.spans);
                if let Some(file) = piece.avatar {
                    pictures.push((file, Rect { x: area.x + col, y: area.y + row, width: super::media::AVATAR_SIZE.0, height: super::media::AVATAR_SIZE.1 }));
                }
            }
        }
        let lines: Vec<Line> = lines.into_iter().map(Line::from).collect();
        f.render_widget(Paragraph::new(lines).style(Style::default().bg(super::theme::p().call_bg)), area);
        // The pictures over their cells (those that fit in the bar).
        let mut wanted = None;
        for (file, slot) in pictures {
            match media.thumbs.get(&file.id) {
                Some(Thumb::Ready(proto)) if slot.bottom() <= area.bottom() && slot.right() <= area.right() => f.render_widget(SlicedImage::new(proto, SignedPosition { x: 0, y: 0 }), slot),
                Some(_) => {}
                None => wanted = Some((file, super::media::AVATAR_SIZE)),
            }
        }
        wanted
    }
}

/// The call bar is at most this many rows; more is cut off.
const BAR_ROWS: u16 = 4;
/// Names in the call bar are cut to this many characters.
const SPEAKER_NAME: usize = 20;
/// Between pieces on a row of the call bar.
const SEP: &str = " · ";

/// A piece of the call bar: kept on one row, wrapped as a whole.
struct Piece {
    spans: Vec<Span<'static>>,
    /// A profile picture over the first two cells.
    avatar: Option<File>,
}

fn widths(groups: &[Vec<Piece>]) -> Vec<Vec<usize>> {
    groups.iter().map(|g| g.iter().map(|p| p.spans.iter().map(|s| s.width()).sum()).collect()).collect()
}

/// Lay out pieces of the given widths in rows `width` cells wide: a space before each row,
/// [`SEP`] between pieces on a row, a piece that does not fit goes to the next row, and each
/// group starts a row of its own. The (row, column) of each piece, and how many rows.
fn flow(groups: &[Vec<usize>], width: usize) -> (Vec<Vec<(u16, u16)>>, u16) {
    let sep = SEP.chars().count();
    let mut row = 0usize;
    let mut places = Vec::new();
    let mut used = false;
    for group in groups {
        if used && !group.is_empty() {
            row += 1;
        }
        let mut col: Option<usize> = None;
        let mut at = Vec::new();
        for &w in group {
            let start = match col {
                Some(c) if c + sep + w <= width => c + sep,
                Some(_) => {
                    row += 1;
                    1
                }
                None => 1,
            };
            at.push((row as u16, start as u16));
            col = Some(start + w);
            used = true;
        }
        places.push(at);
    }
    (places, if used { row as u16 + 1 } else { 0 })
}

/// "Bob Smith" → "Bob Smi…" when longer than `max` characters.
fn truncate(name: &str, max: usize) -> String {
    if name.chars().count() <= max { name.to_string() } else { name.chars().take(max.saturating_sub(1)).chain(['…']).collect() }
}

/// "just you", "2 here".
fn people(n: usize) -> String {
    if n <= 1 { "just you".into() } else { format!("{n} here") }
}

/// The loudest of the others who is talking (not muted, above the speaking level).
fn loudest_other(levels: &std::collections::HashMap<String, f32>, roster: &[Participant], me: &str) -> Option<String> {
    roster
        .iter()
        .filter(|p| p.attendee_id != me && !p.muted)
        .filter_map(|p| levels.get(&p.attendee_id).map(|l| (p, *l)))
        .filter(|(_, l)| *l >= huddle::session::SPEAKING)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(p, _)| p.attendee_id.clone())
}

/// After connecting, roster changes this soon are the people already there arriving.
const ROSTER_SETTLE: std::time::Duration = std::time::Duration::from_secs(3);

/// Whether we have been alone (since `since`) for at least `wait`.
fn alone_long_enough(since: Option<std::time::Instant>, now: std::time::Instant, wait: std::time::Duration) -> bool {
    since.is_some_and(|s| now.duration_since(s) >= wait)
}

/// How many people (other than `me`) joined and left between two rosters.
fn roster_changes(old: &[Participant], new: &[Participant], me: &str) -> (usize, usize) {
    let had = |r: &[Participant], id: &str| r.iter().any(|p| p.attendee_id == id);
    let joined = new.iter().filter(|p| p.attendee_id != me && !had(old, &p.attendee_id)).count();
    let left = old.iter().filter(|p| p.attendee_id != me && !had(new, &p.attendee_id)).count();
    (joined, left)
}

#[cfg(test)]
mod tests {
    #[test]
    fn alone_reminders() {
        let t0 = std::time::Instant::now();
        let m = |s: u64| t0 + std::time::Duration::from_secs(s);
        let wait = std::time::Duration::from_secs(300);
        assert!(!super::alone_long_enough(None, m(1000), wait), "not alone");
        assert!(!super::alone_long_enough(Some(t0), m(299), wait));
        assert!(super::alone_long_enough(Some(t0), m(300), wait), "after the wait");
    }

    #[test]
    fn roster_changes_count_others() {
        // Shaped like the real rosters (attendee ids, `<team>-<room>-<user>`).
        let p = |id: &str, user: &str| huddle::session::Participant { attendee_id: id.into(), external_user_id: format!("T0TEST00001-R0CA47XA37G-{user}"), muted: false };
        let me = p("ecfb4d87-adf6", "U0ANN000001");
        let b = p("95952636-2b52", "U0BOB000001");
        let c = p("11111111-2222", "U0C0000000C");
        assert_eq!(super::roster_changes(&[me.clone()], &[me.clone(), b.clone()], &me.attendee_id), (1, 0));
        assert_eq!(super::roster_changes(&[me.clone(), b.clone()], &[me.clone()], &me.attendee_id), (0, 1));
        assert_eq!(super::roster_changes(&[me.clone(), b.clone()], &[me.clone(), c.clone()], &me.attendee_id), (1, 1), "one left, one came");
        assert_eq!(super::roster_changes(&[], &[me.clone()], &me.attendee_id), (0, 0), "not for ourselves");
        assert_eq!(super::roster_changes(&[me.clone(), b.clone()], &[b.clone(), me.clone()], &me.attendee_id), (0, 0), "order does not matter");
    }

    #[test]
    fn shares_from_video_sources() {
        let src = |attendee: &str| huddle::session::VideoSource { stream_id: 1, group_id: 1, attendee_id: attendee.into(), external_user_id: "T1-R1-U2".into(), max_bitrate_kbps: 1000 };
        assert_eq!(super::screen_shares(&[src("a"), src("b#content"), src("b#content")]), vec![("b#content".to_string(), "U2".to_string())]);
    }

    #[test]
    fn bar_flow() {
        // Wide: the state on one row (the bar has no keys row).
        let (places, rows) = super::flow(&[vec![10, 8, 9, 7]], 80);
        assert_eq!((places, rows), (vec![vec![(0, 1), (0, 14), (0, 25), (0, 37)]], 1));
        // Groups still start rows of their own.
        let (places, rows) = super::flow(&[vec![10, 8], vec![9, 7]], 80);
        assert_eq!((places, rows), (vec![vec![(0, 1), (0, 14)], vec![(1, 1), (1, 13)]], 2));
        // Narrow: a piece that does not fit goes to the next row, whole.
        let (places, rows) = super::flow(&[vec![10, 8, 5], vec![9, 7]], 22);
        assert_eq!((places, rows), (vec![vec![(0, 1), (0, 14), (1, 1)], vec![(2, 1), (2, 13)]], 3));
        // Exactly fitting: 1 + 10 + 3 + 8 = 22.
        assert_eq!(super::flow(&[vec![10, 8]], 22).1, 1);
        assert_eq!(super::flow(&[vec![10, 8]], 21).1, 2);
        // A piece wider than the bar still gets a row of its own (cut off when drawn).
        assert_eq!(super::flow(&[vec![50, 4]], 20), (vec![vec![(0, 1), (1, 1)]], 2));
        assert_eq!(super::flow(&[vec![], vec![3]], 20), (vec![vec![], vec![(0, 1)]], 1), "an empty group takes no row");
    }

    #[test]
    fn bar_words() {
        assert_eq!(super::truncate("Dmytro Dudnyk-Longname-Here", 20), "Dmytro Dudnyk-Longn…");
        assert_eq!(super::truncate("Bob", 20), "Bob");
        assert_eq!(super::truncate("Ölgä Ünïcödé Näme Xyz", 20).chars().count(), 20);
        assert_eq!(super::people(1), "just you");
        assert_eq!(super::people(0), "just you");
        assert_eq!(super::people(3), "3 here");
    }

    #[test]
    fn last_speaker_is_the_loudest_other() {
        let p = |id: &str, muted: bool| huddle::session::Participant { attendee_id: id.into(), external_user_id: format!("T0TEST00001-R0CA47XA37G-U{id}"), muted };
        let roster = [p("me", false), p("b", false), p("c", false), p("m", true)];
        let levels = |l: &[(&str, f32)]| l.iter().map(|(k, v)| (k.to_string(), *v)).collect::<std::collections::HashMap<_, _>>();
        assert_eq!(super::loudest_other(&levels(&[("me", 0.9), ("b", 0.3), ("c", 0.5)]), &roster, "me").as_deref(), Some("c"));
        assert_eq!(super::loudest_other(&levels(&[("me", 0.9)]), &roster, "me"), None, "not ourselves");
        assert_eq!(super::loudest_other(&levels(&[("b", 0.05)]), &roster, "me"), None, "below the speaking level");
        assert_eq!(super::loudest_other(&levels(&[("m", 0.9)]), &roster, "me"), None, "muted");
        assert_eq!(super::loudest_other(&levels(&[("x", 0.9)]), &roster, "me"), None, "not in the roster");
    }

    #[test]
    fn meters() {
        assert_eq!(super::meter(0.0), "····");
        assert_eq!(super::meter(0.1), "▂···");
        assert_eq!(super::meter(0.6), "▂▄▆·");
        assert_eq!(super::meter(1.0), "▂▄▆█");
    }
}

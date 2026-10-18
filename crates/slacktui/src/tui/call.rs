//! The huddle you are in: session state, audio devices and the call bar.


use huddle::devices::{MicConfig, MicProcessor, Microphone, Speaker, Volume};
use huddle::session::{Participant, PcmSink, Session, SessionEvent};
use huddle::signaling::Credentials;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

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
    /// Our own microphone level while unmuted (f32 bits), measured locally.
    mic_level: std::sync::Arc<std::sync::atomic::AtomicU32>,
    /// Whether the speaking threshold lets our microphone through.
    mic_open: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Gain, threshold and noise suppression, read by the microphone for each frame.
    mic_config: std::sync::Arc<std::sync::Mutex<MicConfig>>,
    /// Playback volume of the meeting (None without a speaker).
    volume: Option<Volume>,
    pub muted: bool,
    pub connected: bool,
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
        let (volume, speaker_error) = match Speaker::start(devices.1) {
            Ok(mut speaker) => {
                let volume = speaker.volume();
                tokio::spawn(async move {
                    while let Some(frame) = audio.recv().await {
                        speaker.play(&frame);
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
            mic_level: Default::default(),
            mic_open: Default::default(),
            mic_config: Default::default(),
            volume,
            muted: true,
            connected: false,
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
            let mut processor = MicProcessor::new();
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
        self.mic = None;
        self.camera = None;
        self.stop_share();
        self.session.leave();
    }

    /// Apply a session event; true once the session has ended.
    pub fn apply(&mut self, ev: SessionEvent) -> bool {
        match ev {
            SessionEvent::Connected => self.connected = true,
            // A screen share joins as a second attendee: it is shown as a share, not a person.
            SessionEvent::Roster(r) => self.roster = r.into_iter().filter(|p| !huddle::signaling::is_content(&p.attendee_id)).collect(),
            SessionEvent::Levels(l) => self.levels = l.into_iter().collect(),
            SessionEvent::Error(e) => tracing::warn!("huddle: {e}"),
            SessionEvent::VideoSources(sources) => self.shares = screen_shares(&sources),
            SessionEvent::SendingVideo { .. } => {}
            SessionEvent::ReceiveCodecs(c) => tracing::info!("meeting receive codecs: {c:?}"),
            SessionEvent::Ended => return true,
        }
        false
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

    /// Whether the participant is sharing their screen.
    pub fn sharing(&self, attendee_id: &str) -> bool {
        (attendee_id == self.self_attendee && self.share.is_some()) || self.shares.iter().any(|(share, _)| share.strip_suffix(huddle::signaling::CONTENT_MODALITY) == Some(attendee_id))
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

    /// `keys`: the mute, huddle, thread and view keys, for the hint.
    pub fn draw(&self, f: &mut Frame, area: Rect, ws: &Workspace, keys: (&str, &str, &str, &str)) {
        let place = self.place(ws);
        let green = Style::default().fg(super::theme::p().own);
        let mut spans = vec![Span::styled(format!(" 🎧 {place} "), green.add_modifier(Modifier::BOLD))];
        if !self.connected {
            spans.push(Span::styled("connecting… ", Style::default().fg(super::theme::p().dim)));
        }
        for (n, p) in self.roster.iter().enumerate() {
            if n > 0 {
                spans.push(Span::raw(", "));
            }
            let (level, speaking, muted, _) = self.participant_state(p, &ws.self_id);
            let style = if speaking { green.add_modifier(Modifier::BOLD) } else { Style::default() };
            spans.push(Span::styled(ws.person_label(p.user_id()), style));
            if self.video(&p.attendee_id).is_some() {
                spans.push(Span::raw(" 📹"));
            }
            if self.sharing(&p.attendee_id) {
                spans.push(Span::raw(" 🖥"));
            }
            if muted {
                spans.push(Span::styled(" 🔇", Style::default().fg(super::theme::p().dim)));
            } else {
                spans.push(Span::styled(format!(" {}", meter(level)), if speaking { green } else { Style::default().fg(super::theme::p().dim) }));
            }
        }
        let (mute, leave, thread, view) = keys;
        let mic = if self.muted { format!("🔇 muted · {mute} unmute") } else { format!("🎙 live · {mute} mute") };
        spans.push(Span::styled(format!("  ·  {mic} · {view} view · {thread} thread · {leave} leave"), Style::default().fg(super::theme::p().dim)));
        if let Some(e) = &self.speaker_error {
            spans.push(Span::styled(format!(" · no sound: {e}"), Style::default().fg(super::theme::p().danger)));
        }
        f.render_widget(Paragraph::new(Line::from(spans)).style(Style::default().bg(super::theme::p().call_bg)), area);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shares_from_video_sources() {
        let src = |attendee: &str| huddle::session::VideoSource { stream_id: 1, group_id: 1, attendee_id: attendee.into(), external_user_id: "T1-R1-U2".into(), max_bitrate_kbps: 1000 };
        assert_eq!(super::screen_shares(&[src("a"), src("b#content"), src("b#content")]), vec![("b#content".to_string(), "U2".to_string())]);
    }

    #[test]
    fn meters() {
        assert_eq!(super::meter(0.0), "····");
        assert_eq!(super::meter(0.1), "▂···");
        assert_eq!(super::meter(0.6), "▂▄▆·");
        assert_eq!(super::meter(1.0), "▂▄▆█");
    }
}

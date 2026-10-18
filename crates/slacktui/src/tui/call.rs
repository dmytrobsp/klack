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
        let mut session = Session::start(creds, true, false);
        let sink = session.pcm_sink();
        let (_, empty) = tokio::sync::mpsc::unbounded_channel();
        let mut audio = std::mem::replace(&mut session.audio, empty);
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

    pub fn leave(&mut self) {
        self.mic = None;
        self.session.leave();
    }

    /// Apply a session event; true once the session has ended.
    pub fn apply(&mut self, ev: SessionEvent) -> bool {
        match ev {
            SessionEvent::Connected => self.connected = true,
            SessionEvent::Roster(r) => self.roster = r,
            SessionEvent::Levels(l) => self.levels = l.into_iter().collect(),
            SessionEvent::Error(e) => tracing::warn!("huddle: {e}"),
            // Video arrives with item 14.
            SessionEvent::VideoSources(_) | SessionEvent::SendingVideo { .. } => {}
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
    fn meters() {
        assert_eq!(super::meter(0.0), "····");
        assert_eq!(super::meter(0.1), "▂···");
        assert_eq!(super::meter(0.6), "▂▄▆·");
        assert_eq!(super::meter(1.0), "▂▄▆█");
    }
}

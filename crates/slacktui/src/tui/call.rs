//! The huddle you are in: session state, audio devices and the call bar.

use std::collections::HashSet;

use huddle::devices::{Microphone, Speaker};
use huddle::session::{Participant, PcmSink, Session, SessionEvent};
use huddle::signaling::Credentials;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::state::Workspace;
use super::text::DIM;

pub struct Call {
    pub channel: String,
    /// The thread the huddle was started in, if any.
    pub thread: Option<String>,
    /// Microphone to open when unmuting (None: the default).
    input: Option<String>,
    pub session: Session,
    pub roster: Vec<Participant>,
    /// Attendee ids currently speaking.
    pub speaking: HashSet<String>,
    pub muted: bool,
    pub connected: bool,
    sink: PcmSink,
    /// Open only while unmuted, so a muted call never touches the microphone.
    mic: Option<Microphone>,
    /// Why there is no sound, if the speaker could not be opened.
    pub speaker_error: Option<String>,
}

impl Call {
    /// Start the session (muted) and play the meeting's audio on the default output.
    /// `devices`: (microphone, speaker) names, None for the system default.
    pub fn start(channel: String, creds: Credentials, devices: (Option<String>, Option<String>)) -> Call {
        let mut session = Session::start(creds, true);
        let sink = session.pcm_sink();
        let (_, empty) = tokio::sync::mpsc::unbounded_channel();
        let mut audio = std::mem::replace(&mut session.audio, empty);
        let speaker_error = match Speaker::start(devices.1) {
            Ok(mut speaker) => {
                tokio::spawn(async move {
                    while let Some(frame) = audio.recv().await {
                        speaker.play(&frame);
                    }
                });
                None
            }
            Err(e) => {
                tracing::warn!("no speaker: {e}");
                Some(e.to_string())
            }
        };
        Call { channel, thread: None, input: devices.0, session, roster: Vec::new(), speaking: HashSet::new(), muted: true, connected: false, sink, mic: None, speaker_error }
    }

    /// Toggle the microphone; opening it can fail (no device, no permission).
    pub fn toggle_mute(&mut self) -> anyhow::Result<()> {
        if self.muted {
            let sink = self.sink.clone();
            self.mic = Some(Microphone::start(self.input.clone(), move |frame| sink.send(frame))?);
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
            SessionEvent::Speaking(s) => self.speaking = s.into_iter().collect(),
            SessionEvent::Error(e) => tracing::warn!("huddle: {e}"),
            SessionEvent::Ended => return true,
        }
        false
    }

    pub fn draw(&self, f: &mut Frame, area: Rect, ws: &Workspace) {
        let place = ws.conversations.get(&self.channel).map(|c| if c.is_dm() { ws.conversation_label(c) } else { format!("#{}", c.name) }).unwrap_or_else(|| self.channel.clone());
        let place = if self.thread.is_some() { format!("{place} · thread") } else { place };
        let green = Style::default().fg(Color::Rgb(0x2b, 0xac, 0x76));
        let mut spans = vec![Span::styled(format!(" 🎧 {place} "), green.add_modifier(Modifier::BOLD))];
        if !self.connected {
            spans.push(Span::styled("connecting… ", Style::default().fg(DIM)));
        }
        for (n, p) in self.roster.iter().enumerate() {
            if n > 0 {
                spans.push(Span::raw(", "));
            }
            let name = ws.person_label(p.user_id());
            let style = if self.speaking.contains(&p.attendee_id) { green.add_modifier(Modifier::BOLD) } else { Style::default() };
            spans.push(Span::styled(name, style));
            if p.muted {
                spans.push(Span::styled(" 🔇", Style::default().fg(DIM)));
            } else if self.speaking.contains(&p.attendee_id) {
                spans.push(Span::styled(" 🔊", green));
            }
        }
        let mic = if self.muted { "🔇 muted · F4 unmute" } else { "🎙 live · F4 mute" };
        spans.push(Span::styled(format!("  ·  {mic} · F3 leave"), Style::default().fg(DIM)));
        if let Some(e) = &self.speaker_error {
            spans.push(Span::styled(format!(" · no sound: {e}"), Style::default().fg(Color::Red)));
        }
        f.render_widget(Paragraph::new(Line::from(spans)).style(Style::default().bg(Color::Rgb(0x1b, 0x2b, 0x22))), area);
    }
}

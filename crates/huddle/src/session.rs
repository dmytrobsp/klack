//! A running huddle: signaling + media + Opus, driven by one task.
//!
//! Audio crosses the boundary as 48 kHz mono `i16` PCM in 20 ms frames (960 samples):
//! send frames with [`Session::send_pcm`], receive the meeting's mix from `audio`.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Result, anyhow};
use tokio::sync::mpsc;

use crate::media::Media;
use crate::proto::{SdkSignalFrame, sdk_signal_frame::Type};
use crate::signaling::{self, Credentials, Signaling};

pub const SAMPLE_RATE: u32 = 48_000;
/// Samples per 20 ms frame at 48 kHz mono.
pub const FRAME_SAMPLES: usize = 960;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Participant {
    pub attendee_id: String,
    /// `<team>-<room>-<user>` as Slack sets it.
    pub external_user_id: String,
    pub muted: bool,
}

impl Participant {
    /// Slack user id from the external user id.
    pub fn user_id(&self) -> &str {
        self.external_user_id.rsplit('-').next().unwrap_or(&self.external_user_id)
    }
}

#[derive(Debug, Clone)]
pub enum SessionEvent {
    /// Media connected: audio flows.
    Connected,
    /// Who is in the huddle (keyed by audio stream id).
    Roster(Vec<Participant>),
    /// Audio stream ids currently above the speaking threshold.
    Speaking(Vec<String>),
    Error(String),
    /// The session ended (left, kicked or failed).
    Ended,
}

enum Command {
    Pcm(Vec<i16>),
    Mute(bool),
    Leave,
}

pub struct Session {
    commands: mpsc::UnboundedSender<Command>,
    pub events: mpsc::UnboundedReceiver<SessionEvent>,
    /// The meeting's mix, 20 ms mono frames.
    pub audio: mpsc::UnboundedReceiver<Vec<i16>>,
}

impl Session {
    pub fn start(creds: Credentials, muted: bool) -> Session {
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (event_tx, events) = mpsc::unbounded_channel();
        let (audio_tx, audio) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            if let Err(e) = run(creds, muted, command_rx, event_tx.clone(), audio_tx).await {
                let _ = event_tx.send(SessionEvent::Error(e.to_string()));
            }
            let _ = event_tx.send(SessionEvent::Ended);
        });
        Session { commands, events, audio }
    }

    /// Queue 20 ms of microphone audio (960 samples, 48 kHz mono).
    pub fn send_pcm(&self, frame: Vec<i16>) {
        let _ = self.commands.send(Command::Pcm(frame));
    }

    pub fn set_muted(&self, muted: bool) {
        let _ = self.commands.send(Command::Mute(muted));
    }

    pub fn leave(&self) {
        let _ = self.commands.send(Command::Leave);
    }
}

/// Wait for a frame of `kind`, answering pings and skipping others.
async fn expect(sig: &mut Signaling, kind: Type) -> Result<SdkSignalFrame> {
    loop {
        let f = tokio::time::timeout(Duration::from_secs(15), sig.next()).await.map_err(|_| anyhow!("timed out waiting for {kind:?}"))?.ok_or_else(|| anyhow!("signaling closed"))??;
        if let Some(e) = &f.error {
            return Err(anyhow!("signaling error {:?}: {}", e.status, e.description.clone().unwrap_or_default()));
        }
        if let Some(p) = &f.ping_pong {
            sig.send(&signaling::pong_frame(p.ping_id)).await?;
        }
        if f.r#type == kind as i32 {
            return Ok(f);
        }
    }
}

async fn run(
    creds: Credentials,
    mut muted: bool,
    mut commands: mpsc::UnboundedReceiver<Command>,
    events: mpsc::UnboundedSender<SessionEvent>,
    audio: mpsc::UnboundedSender<Vec<i16>>,
) -> Result<()> {
    let mut sig = Signaling::connect(&creds).await?;
    sig.send(&signaling::join_frame(audio_session_id())).await?;
    let ack = expect(&mut sig, Type::JoinAck).await?;
    let turn = ack.joinack.and_then(|a| a.turn_credentials).ok_or_else(|| anyhow!("JOIN_ACK without TURN credentials"))?;
    let (mut media, offer) = Media::offer(&turn).await?;
    sig.send(&signaling::subscribe_frame(offer, &creds.meeting.media_placement.audio_host_url, &creds.attendee.attendee_id, muted)).await?;
    let suback = expect(&mut sig, Type::SubscribeAck).await?;
    let answer = suback.suback.and_then(|s| s.sdp_answer).ok_or_else(|| anyhow!("SUBSCRIBE_ACK without an SDP answer"))?;
    media.accept(answer).await?;

    let mut encoder = opus::Encoder::new(SAMPLE_RATE, opus::Channels::Mono, opus::Application::Voip)?;
    let mut decoder = opus::Decoder::new(SAMPLE_RATE, opus::Channels::Mono)?;
    let mut roster: BTreeMap<u32, Participant> = BTreeMap::new();
    let mut connected = false;
    let mut out = vec![0u8; 4000];
    let mut pcm = vec![0i16; FRAME_SAMPLES * 6];
    loop {
        tokio::select! {
            changed = media.state.changed() => {
                if changed.is_err() { break; }
                let state = *media.state.borrow();
                use webrtc::peer_connection::RTCPeerConnectionState as S;
                match state {
                    S::Connected if !connected => {
                        connected = true;
                        let _ = events.send(SessionEvent::Connected);
                    }
                    S::Failed | S::Closed => return Err(anyhow!("media connection {state}")),
                    _ => {}
                }
            }
            frame = sig.next() => {
                let Some(frame) = frame else { break };
                let frame = frame?;
                if let Some(p) = &frame.ping_pong {
                    sig.send(&signaling::pong_frame(p.ping_id)).await?;
                }
                if let Some(info) = &frame.audio_stream_id_info {
                    for s in &info.streams {
                        let Some(id) = s.audio_stream_id else { continue };
                        if s.dropped == Some(true) {
                            roster.remove(&id);
                            continue;
                        }
                        let p = roster.entry(id).or_insert_with(|| Participant { attendee_id: String::new(), external_user_id: String::new(), muted: false });
                        if let Some(a) = &s.attendee_id { p.attendee_id = a.clone(); }
                        if let Some(x) = &s.external_user_id { p.external_user_id = x.clone(); }
                        if let Some(m) = s.muted { p.muted = m; }
                    }
                    let _ = events.send(SessionEvent::Roster(roster.values().cloned().collect()));
                }
                if let Some(meta) = &frame.audio_metadata {
                    let speaking: Vec<String> = meta.attendee_states.iter().filter(|s| s.volume.unwrap_or(0) > 0 && s.muted != Some(true)).filter_map(|s| s.audio_stream_id.and_then(|id| roster.get(&id)).map(|p| p.attendee_id.clone())).collect();
                    let _ = events.send(SessionEvent::Speaking(speaking));
                }
                if let Some(e) = &frame.error {
                    return Err(anyhow!("signaling error {:?}: {}", e.status, e.description.clone().unwrap_or_default()));
                }
                if frame.r#type == Type::LeaveAck as i32 {
                    break;
                }
            }
            payload = media.incoming.recv() => {
                let Some(payload) = payload else { break };
                match decoder.decode(&payload, &mut pcm, false) {
                    Ok(n) => { let _ = audio.send(pcm[..n].to_vec()); }
                    Err(e) => tracing::debug!("opus decode: {e}"),
                }
            }
            cmd = commands.recv() => match cmd {
                Some(Command::Pcm(frame)) if !muted && connected && frame.len() == FRAME_SAMPLES => {
                    let n = encoder.encode(&frame, &mut out)?;
                    media.send_opus(out[..n].to_vec(), Duration::from_millis(20)).await?;
                }
                Some(Command::Pcm(_)) => {}
                Some(Command::Mute(m)) => {
                    muted = m;
                    sig.send(&signaling::mute_frame(m)).await?;
                }
                Some(Command::Leave) | None => {
                    sig.send(&signaling::frame(Type::Leave)).await?;
                    // Wait briefly for LEAVE_ACK so the server drops us at once.
                    let _ = tokio::time::timeout(Duration::from_secs(3), expect(&mut sig, Type::LeaveAck)).await;
                    break;
                }
            },
        }
    }
    media.close().await;
    sig.close().await;
    Ok(())
}

fn audio_session_id() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u32(std::process::id());
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_id_from_external_id() {
        let p = Participant { attendee_id: "a".into(), external_user_id: "T0CA6QW46KB-R0CA8VAPS76-U0C9YM9J3RR".into(), muted: false };
        assert_eq!(p.user_id(), "U0C9YM9J3RR");
    }

    #[test]
    fn opus_round_trip_keeps_a_tone() {
        let mut enc = opus::Encoder::new(SAMPLE_RATE, opus::Channels::Mono, opus::Application::Voip).unwrap();
        let mut dec = opus::Decoder::new(SAMPLE_RATE, opus::Channels::Mono).unwrap();
        let mut energy = 0f64;
        for f in 0..10 {
            let frame: Vec<i16> = (0..FRAME_SAMPLES).map(|i| (((f * FRAME_SAMPLES + i) as f64 * 440.0 * std::f64::consts::TAU / SAMPLE_RATE as f64).sin() * 8000.0) as i16).collect();
            let mut out = vec![0u8; 4000];
            let n = enc.encode(&frame, &mut out).unwrap();
            let mut pcm = vec![0i16; FRAME_SAMPLES * 6];
            let m = dec.decode(&out[..n], &mut pcm, false).unwrap();
            assert_eq!(m, FRAME_SAMPLES);
            if f > 2 {
                energy += pcm[..m].iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / m as f64;
            }
        }
        assert!(energy.sqrt() > 3000.0, "decoded tone too quiet: {energy}");
    }
}

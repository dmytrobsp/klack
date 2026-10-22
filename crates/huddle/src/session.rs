//! A running huddle: signaling + media + Opus, driven by one task.
//!
//! Audio crosses the boundary as 48 kHz mono `i16` PCM in 20 ms frames (960 samples):
//! send frames with [`Session::send_pcm`], receive the meeting's mix from `audio`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// Levels from here up count as speaking.
pub const SPEAKING: f32 = 0.1;

/// An attendee's speaking level (0–1) from the `volume` of its audio metadata, which is
/// dB below full scale: the Chime SDK maps -14 dB and louder to 1 and -42 dB to 0.
pub fn level(volume: Option<u32>) -> f32 {
    volume.map_or(0.0, |v| ((42.0 - v as f32) / 28.0).clamp(0.0, 1.0))
}

/// Someone's video in the meeting, from INDEX.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoSource {
    /// What to subscribe to.
    pub stream_id: u32,
    /// The publisher's tile (one group per attendee and kind of video).
    pub group_id: u32,
    pub attendee_id: String,
    pub external_user_id: String,
    pub max_bitrate_kbps: u32,
}

/// Names of INDEX codec capabilities (unknown values by number).
pub fn codec_names(caps: &[i32]) -> Vec<String> {
    caps.iter().map(|c| crate::proto::SdkVideoCodecCapability::try_from(*c).map_or_else(|_| format!("codec {c}"), |c| format!("{c:?}"))).collect()
}

/// The video sources of an INDEX frame (other kinds of media left out).
pub fn video_sources(index: &crate::proto::SdkIndexFrame) -> Vec<VideoSource> {
    index
        .sources
        .iter()
        .filter(|s| s.media_type == Some(crate::proto::SdkStreamMediaType::Video as i32))
        .filter_map(|s| {
            Some(VideoSource {
                stream_id: s.stream_id?,
                group_id: s.group_id.unwrap_or_default(),
                attendee_id: s.attendee_id.clone().unwrap_or_default(),
                external_user_id: s.external_user_id.clone().unwrap_or_default(),
                max_bitrate_kbps: s.max_bitrate_kbps.unwrap_or_default(),
            })
        })
        .collect()
}

/// The most videos we subscribe to (the server's limit may be lower).
pub const MAX_VIDEOS: usize = 9;

/// A picture from someone's video.
#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub attendee_id: String,
    pub stream_id: u32,
    pub width: usize,
    pub height: usize,
    /// Packed RGB.
    pub rgb: Arc<Vec<u8>>,
}

/// Which streams to receive, as the SDK's default policy picks them: per publisher (group)
/// the source with the highest bitrate, never our own (nor our own content share), at most
/// `limit`, in group order.
pub fn choose_streams(sources: &[VideoSource], self_attendee: &str, limit: usize) -> Vec<u32> {
    let mut best: BTreeMap<u32, &VideoSource> = BTreeMap::new();
    for s in sources.iter().filter(|s| !s.attendee_id.starts_with(self_attendee) || self_attendee.is_empty()) {
        let slot = best.entry(s.group_id).or_insert(s);
        if s.max_bitrate_kbps > slot.max_bitrate_kbps {
            *slot = s;
        }
    }
    best.values().take(limit).map(|s| s.stream_id).collect()
}

/// Receive slots (one per receive-only video section, 0 = unused) after `wanted` changed:
/// streams still wanted keep their section, the others free it, new ones take a free
/// section or a new one at the end (sections are never removed).
pub fn assign_slots(current: &[u32], wanted: &[u32]) -> Vec<u32> {
    let mut slots: Vec<u32> = current.iter().map(|s| if wanted.contains(s) { *s } else { 0 }).collect();
    for w in wanted {
        if !slots.contains(w) {
            match slots.iter_mut().find(|s| **s == 0) {
                Some(free) => *free = *w,
                None => slots.push(*w),
            }
        }
    }
    slots
}

/// SUBSCRIBE's `receive_stream_ids`: one per video section in SDP order — 0 for the camera's.
pub fn receive_ids(camera: bool, slots: &[u32]) -> Vec<u32> {
    camera.then_some(0).into_iter().chain(slots.iter().copied()).collect()
}

#[derive(Debug, Clone)]
pub enum SessionEvent {
    /// Media connected: audio flows.
    Connected,
    /// Who is in the huddle (keyed by audio stream id).
    Roster(Vec<Participant>),
    /// Speaking levels (0–1) of the attendees in the last audio metadata frame, by
    /// attendee id; attendees left out are silent.
    Levels(Vec<(String, f32)>),
    /// The videos the meeting offers (every INDEX frame; ours included).
    VideoSources(Vec<VideoSource>),
    /// The video codecs everyone in the meeting can receive (INDEX), when it changes.
    ReceiveCodecs(Vec<String>),
    /// The server accepted our camera under this stream id (SUBSCRIBE_ACK).
    SendingVideo { stream_id: u32 },
    Error(String),
    /// The media connection was lost; joining again (`Connected` follows when it worked).
    Reconnecting,
    /// The session ended (left, kicked or failed).
    Ended,
}

enum Command {
    Pcm(Vec<i16>),
    /// An H.264 access unit and the time it covers.
    Video(Vec<u8>, Duration),
    /// Turn the camera on or off.
    Camera(bool),
    Mute(bool),
    Leave,
}

/// How a session joins.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Join with the microphone muted.
    pub muted: bool,
    /// Offer a camera (fed through [`Session::video_sink`]).
    pub send_video: bool,
    /// Receive other people's video (a screen share session does not).
    pub receive_video: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { muted: true, send_video: false, receive_video: true }
    }
}

pub struct Session {
    commands: mpsc::UnboundedSender<Command>,
    keyframe: Arc<AtomicBool>,
    pub events: mpsc::UnboundedReceiver<SessionEvent>,
    /// The meeting's mix, 20 ms mono frames.
    pub audio: mpsc::UnboundedReceiver<Vec<i16>>,
    /// Pictures from other people's video.
    pub video: mpsc::UnboundedReceiver<VideoFrame>,
}

/// Clonable handle that feeds microphone frames into a session (e.g. from an audio thread).
#[derive(Clone)]
pub struct PcmSink(mpsc::UnboundedSender<Command>);

impl PcmSink {
    pub fn send(&self, frame: Vec<i16>) {
        let _ = self.0.send(Command::Pcm(frame));
    }
}

/// Clonable handle that feeds encoded video into a session (for one started with video).
#[derive(Clone)]
pub struct VideoSink(mpsc::UnboundedSender<Command>, Arc<AtomicBool>);

impl VideoSink {
    /// Queue an H.264 access unit (Annex-B) covering `duration`.
    pub fn send(&self, access_unit: Vec<u8>, duration: Duration) {
        let _ = self.0.send(Command::Video(access_unit, duration));
    }

    /// Whether the next frame should be a keyframe (media just connected, so frames sent
    /// before were dropped, or the server asked with a PLI/FIR); asking clears it.
    pub fn keyframe_wanted(&self) -> bool {
        self.1.swap(false, Ordering::Relaxed)
    }
}

impl Session {
    pub fn pcm_sink(&self) -> PcmSink {
        PcmSink(self.commands.clone())
    }

    pub fn video_sink(&self) -> VideoSink {
        VideoSink(self.commands.clone(), self.keyframe.clone())
    }

    pub fn start(creds: Credentials, options: Options) -> Session {
        let Options { muted, send_video, receive_video } = options;
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (event_tx, events) = mpsc::unbounded_channel();
        let (audio_tx, audio) = mpsc::unbounded_channel();
        let (video_tx, video) = mpsc::unbounded_channel();
        let keyframe = Arc::new(AtomicBool::new(false));
        let kf = keyframe.clone();
        tokio::spawn(async move {
            let shared = Shared { muted, send_video, receive_video, keyframe: kf, commands: command_rx, events: event_tx.clone(), audio: audio_tx, video: video_tx, drop_after: test_drop() };
            if let Err(e) = run(creds, shared).await {
                let _ = event_tx.send(SessionEvent::Error(e.to_string()));
            }
            let _ = event_tx.send(SessionEvent::Ended);
        });
        Session { commands, keyframe, events, audio, video }
    }

    /// Queue 20 ms of microphone audio (960 samples, 48 kHz mono).
    pub fn send_pcm(&self, frame: Vec<i16>) {
        let _ = self.commands.send(Command::Pcm(frame));
    }

    /// Turn the camera section on or off (feed it through [`Session::video_sink`]).
    pub fn set_camera(&self, on: bool) {
        let _ = self.commands.send(Command::Camera(on));
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
    expect_keeping(sig, kind, &mut Vec::new()).await
}

/// [`expect`], keeping the frames that came first (INDEX can arrive before the ACKs).
async fn expect_keeping(sig: &mut Signaling, kind: Type, earlier: &mut Vec<SdkSignalFrame>) -> Result<SdkSignalFrame> {
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
        earlier.push(f);
    }
}

/// How long a media connection may stay `disconnected` before it counts as lost (it can come
/// back by itself after a short network gap; `failed` comes only ~25 s later).
pub const DISCONNECTED_GRACE: Duration = Duration::from_secs(4);

/// Whether the media connection is lost: it failed, or has been disconnected for `disconnected_for`
/// at least [`DISCONNECTED_GRACE`].
pub fn media_lost(failed: bool, disconnected_for: Option<Duration>) -> bool {
    failed || disconnected_for.is_some_and(|d| d >= DISCONNECTED_GRACE)
}

/// Joining again after a lost connection: this many tries in a row that do not connect, then
/// the session ends.
pub const MAX_RECONNECT_TRIES: u32 = 3;

/// The wait before reconnect try `tries_failed` (0: the first), or None: give up.
pub fn reconnect_delay(tries_failed: u32) -> Option<Duration> {
    (tries_failed < MAX_RECONNECT_TRIES).then(|| if tries_failed == 0 { Duration::ZERO } else { Duration::from_secs(2u64 << (tries_failed - 1)) })
}

/// What a session keeps across the connections it makes (a reconnect is a new connection).
struct Shared {
    muted: bool,
    send_video: bool,
    receive_video: bool,
    keyframe: Arc<AtomicBool>,
    commands: mpsc::UnboundedReceiver<Command>,
    events: mpsc::UnboundedSender<SessionEvent>,
    audio: mpsc::UnboundedSender<Vec<i16>>,
    video: mpsc::UnboundedSender<VideoFrame>,
    /// Debug builds only, for tests: `KLACK_HUDDLE_DROP=SECS` drops the first connection this
    /// long after it connected, as if its media were lost.
    drop_after: Option<Duration>,
}

/// `KLACK_HUDDLE_DROP` (seconds), in debug builds.
fn test_drop() -> Option<Duration> {
    #[cfg(debug_assertions)]
    if let Some(secs) = std::env::var("KLACK_HUDDLE_DROP").ok().and_then(|v| v.trim().parse::<u64>().ok()) {
        return Some(Duration::from_secs(secs));
    }
    None
}

/// How one connection ended.
enum Outcome {
    /// We left, the server ended it, or the signaling closed: the session is over.
    Over,
    /// The media connection was lost: join again.
    Lost(String),
}

/// The session: connect, and when the media connection is lost join again (a new signaling
/// connection and a new peer connection with the same credentials), as the Chime SDK's
/// reconnect does — instead of dropping out of the huddle. A long call needs it: about 10 minutes
/// in, Chime's TURN server can answer the channel-binding refresh with 438 (stale nonce), and
/// webrtc-rs' TURN client (rtc-turn 0.21.1) then drops the binding instead of retrying, so the
/// relayed media stops (docs/protocol/huddles.md, VX5).
async fn run(creds: Credentials, mut shared: Shared) -> Result<()> {
    let mut ever_connected = false;
    let mut tries_failed = 0u32;
    loop {
        let mut connected = false;
        let result = connect(&creds, &mut shared, &mut connected).await;
        ever_connected |= connected;
        if connected {
            tries_failed = 0;
        }
        let why = match result {
            Ok(Outcome::Over) => return Ok(()),
            Ok(Outcome::Lost(why)) => why,
            // The first connection failing is an error (as before); while reconnecting, a try
            // that fails counts as one more failed try.
            Err(e) if !ever_connected => return Err(e),
            Err(e) => e.to_string(),
        };
        if !connected && ever_connected {
            tries_failed += 1;
        }
        let Some(delay) = reconnect_delay(tries_failed) else {
            return Err(anyhow!("huddle connection lost ({why}); reconnecting failed"));
        };
        tracing::warn!(%why, tries_failed, "huddle connection lost; joining again");
        let _ = shared.events.send(SessionEvent::Reconnecting);
        // A leave asked for meanwhile ends it; nothing else waits (microphone frames are stale).
        let wait = tokio::time::sleep(delay);
        tokio::pin!(wait);
        loop {
            tokio::select! {
                _ = &mut wait => break,
                cmd = shared.commands.recv() => match cmd {
                    Some(Command::Leave) | None => return Ok(()),
                    Some(cmd) => keep_for_next_connection(&mut shared, cmd),
                },
            }
        }
    }
}

/// A command that came while not connected: only the state it sets is kept (mute, camera);
/// audio and video frames are dropped.
fn keep_for_next_connection(shared: &mut Shared, cmd: Command) {
    match cmd {
        Command::Mute(m) => shared.muted = m,
        Command::Camera(on) => shared.send_video = on,
        Command::Pcm(_) | Command::Video(..) | Command::Leave => {}
    }
}

/// One connection: signaling, JOIN, media, SUBSCRIBE, then frames until it is over or lost.
/// `connected` is set once media flows.
async fn connect(creds: &Credentials, shared: &mut Shared, connected: &mut bool) -> Result<Outcome> {
    let Shared { muted, send_video, receive_video, keyframe, commands, events, audio, video, drop_after } = shared;
    let receive_video = *receive_video;
    let mut sig = Signaling::connect(creds).await?;
    sig.send(&signaling::join_frame(audio_session_id())).await?;
    let mut earlier = Vec::new();
    let ack = expect_keeping(&mut sig, Type::JoinAck, &mut earlier).await?;
    let limit = ack.joinack.as_ref().and_then(|a| a.video_subscription_limit).map_or(MAX_VIDEOS, |l| (l as usize).min(MAX_VIDEOS));
    let turn = ack.joinack.and_then(|a| a.turn_credentials).ok_or_else(|| anyhow!("JOIN_ACK without TURN credentials"))?;
    let (mut media, offer) = Media::offer(&turn, *send_video, keyframe.clone()).await?;
    // One entry per video section: just the camera's, which is 0.
    let receive = receive_ids(true, &[]);
    sig.send(&signaling::subscribe_frame(offer, &creds.meeting.media_placement.audio_host_url, &creds.attendee.attendee_id, *muted, *send_video, receive)).await?;
    let suback = expect_keeping(&mut sig, Type::SubscribeAck, &mut earlier).await?.suback.unwrap_or_default();
    // The last INDEX before SUBSCRIBE_ACK still says who is sending video.
    let mut sources = earlier.iter().rev().find_map(|f| f.index.as_ref()).map(video_sources).unwrap_or_default();
    if !sources.is_empty() {
        let _ = events.send(SessionEvent::VideoSources(sources.clone()));
    }
    tracing::info!(allocations = ?suback.allocations, tracks = ?suback.tracks, "SUBSCRIBE_ACK");
    if let Some(stream_id) = suback.allocations.iter().find(|a| a.track_label.as_deref() == Some(crate::media::VIDEO_TRACK_LABEL)).and_then(|a| a.stream_id) {
        let _ = events.send(SessionEvent::SendingVideo { stream_id });
    }
    let mut ssrc_streams: std::collections::HashMap<u32, u32> = suback.tracks.iter().filter_map(|t| Some((t.ssrc?, t.stream_id?))).collect();
    let answer = suback.sdp_answer.ok_or_else(|| anyhow!("SUBSCRIBE_ACK without an SDP answer"))?;
    media.accept(answer).await?;

    // Receiving video: one receive-only section per subscribed stream; a change means a new
    // offer in a new SUBSCRIBE (one at a time: a change meanwhile waits for its ACK).
    let me = creds.attendee.attendee_id.clone();
    let mut slots: Vec<u32> = Vec::new();
    let (mut subscribing, mut resubscribe) = (false, receive_video);
    // Subscribe again even if the slots did not change (the camera was switched).
    let mut camera_changed = false;
    let mut codecs: Vec<String> = Vec::new();

    let mut encoder = opus::Encoder::new(SAMPLE_RATE, opus::Channels::Mono, opus::Application::Voip)?;
    let mut decoder = opus::Decoder::new(SAMPLE_RATE, opus::Channels::Mono)?;
    let mut roster: BTreeMap<u32, Participant> = BTreeMap::new();
    let mut out = vec![0u8; 4000];
    let mut pcm = vec![0i16; FRAME_SAMPLES * 6];
    // Since when the media connection is `disconnected` (it may come back).
    let mut disconnected_since: Option<tokio::time::Instant> = None;
    let mut connected_since: Option<tokio::time::Instant> = None;
    // Frames queued while connecting are stale: drop them, keep what they set.
    while let Ok(cmd) = commands.try_recv() {
        match cmd {
            Command::Leave => {
                sig.send(&signaling::frame(Type::Leave)).await?;
                let _ = tokio::time::timeout(Duration::from_secs(3), expect(&mut sig, Type::LeaveAck)).await;
                media.close().await;
                sig.close().await;
                return Ok(Outcome::Over);
            }
            Command::Mute(m) if m != *muted => {
                *muted = m;
                sig.send(&signaling::mute_frame(m)).await?;
            }
            Command::Camera(on) if on != *send_video => {
                *send_video = on;
                media.set_camera(on).await?;
                camera_changed = true;
                keyframe.store(on, Ordering::Relaxed);
            }
            _ => {}
        }
    }
    let outcome = loop {
        if (resubscribe || camera_changed) && !subscribing {
            resubscribe = false;
            let wanted = if receive_video { assign_slots(&slots, &choose_streams(&sources, &me, limit)) } else { slots.clone() };
            if wanted != slots || camera_changed {
                camera_changed = false;
                slots = wanted;
                let offer = media.reoffer(&slots.iter().map(|s| *s != 0).collect::<Vec<_>>()).await?;
                tracing::info!(?slots, camera = *send_video, "subscribing again");
                sig.send(&signaling::subscribe_frame(offer, &creds.meeting.media_placement.audio_host_url, &me, *muted, *send_video, receive_ids(true, &slots))).await?;
                subscribing = true;
            }
        }
        let lost_at = disconnected_since.map(|t| t + DISCONNECTED_GRACE);
        let drop_at = connected_since.and_then(|t| Some(t + (*drop_after)?));
        tokio::select! {
            Some(f) = media.frames.recv() => {
                let stream_id = ssrc_streams.get(&f.ssrc).copied().unwrap_or_default();
                let attendee_id = sources.iter().find(|s| s.stream_id == stream_id).map(|s| s.attendee_id.clone()).unwrap_or_default();
                let _ = video.send(VideoFrame { attendee_id, stream_id, width: f.width, height: f.height, rgb: f.rgb });
            }
            changed = media.state.changed() => {
                if changed.is_err() { break Outcome::Over; }
                let state = *media.state.borrow();
                use webrtc::peer_connection::RTCPeerConnectionState as S;
                match state {
                    S::Connected => {
                        disconnected_since = None;
                        if !*connected {
                            *connected = true;
                            connected_since = Some(tokio::time::Instant::now());
                            keyframe.store(true, Ordering::Relaxed);
                            let _ = events.send(SessionEvent::Connected);
                        }
                    }
                    S::Disconnected => { disconnected_since.get_or_insert_with(tokio::time::Instant::now); }
                    // Lost before it ever connected: an error (the first try) or a failed try.
                    S::Failed | S::Closed if !*connected => return Err(anyhow!("media connection {state}")),
                    S::Failed | S::Closed => break Outcome::Lost(format!("media connection {state}")),
                    _ => {}
                }
            }
            _ = tokio::time::sleep_until(drop_at.unwrap_or_else(tokio::time::Instant::now)), if drop_at.is_some() => {
                *drop_after = None;
                tracing::warn!("KLACK_HUDDLE_DROP: dropping the connection");
                break Outcome::Lost("KLACK_HUDDLE_DROP".into());
            }
            _ = tokio::time::sleep_until(lost_at.unwrap_or_else(tokio::time::Instant::now)), if lost_at.is_some() => {
                if media_lost(false, disconnected_since.map(|t| t.elapsed())) {
                    break Outcome::Lost(format!("media connection disconnected for {} s", DISCONNECTED_GRACE.as_secs()));
                }
            }
            frame = sig.next() => {
                let Some(frame) = frame else { break Outcome::Over };
                let frame = frame?;
                if let Some(p) = &frame.ping_pong {
                    sig.send(&signaling::pong_frame(p.ping_id)).await?;
                }
                if let Some(info) = &frame.audio_stream_id_info {
                    apply_stream_info(&mut roster, info);
                    let _ = events.send(SessionEvent::Roster(roster.values().cloned().collect()));
                }
                if let Some(meta) = &frame.audio_metadata {
                    let levels: Vec<(String, f32)> = meta
                        .attendee_states
                        .iter()
                        .filter(|s| s.muted != Some(true))
                        .filter_map(|s| s.audio_stream_id.and_then(|id| roster.get(&id)).map(|p| (p.attendee_id.clone(), level(s.volume))))
                        .collect();
                    let _ = events.send(SessionEvent::Levels(levels));
                }
                if let Some(index) = &frame.index {
                    let mut names = codec_names(&index.supported_receive_codec_intersection);
                    names.sort();
                    if names != codecs {
                        codecs = names;
                        let _ = events.send(SessionEvent::ReceiveCodecs(codecs.clone()));
                    }
                    sources = video_sources(index);
                    let _ = events.send(SessionEvent::VideoSources(sources.clone()));
                    resubscribe = receive_video;
                }
                if let Some(ack) = frame.suback.clone().filter(|_| frame.r#type == Type::SubscribeAck as i32) {
                    subscribing = false;
                    tracing::info!(allocations = ?ack.allocations, tracks = ?ack.tracks, "SUBSCRIBE_ACK (again)");
                    if let Some(stream_id) = ack.allocations.iter().find(|a| a.track_label.as_deref() == Some(crate::media::VIDEO_TRACK_LABEL)).and_then(|a| a.stream_id).filter(|_| *send_video) {
                        let _ = events.send(SessionEvent::SendingVideo { stream_id });
                    }
                    ssrc_streams = ack.tracks.iter().filter_map(|t| Some((t.ssrc?, t.stream_id?))).collect();
                    match ack.sdp_answer {
                        Some(answer) => media.accept(answer).await?,
                        None => tracing::warn!("SUBSCRIBE_ACK without an answer"),
                    }
                }
                if let Some(s) = frame.audio_status.as_ref().filter(|_| frame.r#type == Type::AudioStatus as i32) {
                    tracing::info!(status = ?s.audio_status, "huddle audio status");
                }
                if let Some(e) = &frame.error {
                    return Err(anyhow!("signaling error {:?}: {}", e.status, e.description.clone().unwrap_or_default()));
                }
                if frame.r#type == Type::LeaveAck as i32 {
                    break Outcome::Over;
                }
            }
            payload = media.incoming.recv() => {
                let Some(payload) = payload else { break Outcome::Over };
                match decoder.decode(&payload, &mut pcm, false) {
                    Ok(n) => { let _ = audio.send(pcm[..n].to_vec()); }
                    Err(e) => tracing::debug!("opus decode: {e}"),
                }
            }
            cmd = commands.recv() => match cmd {
                Some(Command::Pcm(frame)) if !*muted && *connected && frame.len() == FRAME_SAMPLES => {
                    let n = encoder.encode(&frame, &mut out)?;
                    media.send_opus(out[..n].to_vec(), Duration::from_millis(20)).await?;
                }
                Some(Command::Pcm(_)) => {}
                Some(Command::Video(access_unit, duration)) if *connected => media.send_h264(access_unit, duration).await?,
                Some(Command::Video(..)) => {}
                Some(Command::Camera(on)) if on != *send_video => {
                    *send_video = on;
                    media.set_camera(on).await?;
                    camera_changed = true;
                    // The first picture after switching on must be a keyframe.
                    keyframe.store(on, Ordering::Relaxed);
                }
                Some(Command::Camera(_)) => {}
                Some(Command::Mute(m)) => {
                    *muted = m;
                    sig.send(&signaling::mute_frame(m)).await?;
                }
                Some(Command::Leave) | None => {
                    sig.send(&signaling::frame(Type::Leave)).await?;
                    // Wait briefly for LEAVE_ACK so the server drops us at once.
                    let _ = tokio::time::timeout(Duration::from_secs(3), expect(&mut sig, Type::LeaveAck)).await;
                    break Outcome::Over;
                }
            },
        }
    };
    media.close().await;
    sig.close().await;
    Ok(outcome)
}

/// Update the roster (keyed by audio stream id) from AUDIO_STREAM_ID_INFO, as the web SDK
/// reads it: an entry with an attendee id announces (or re-announces) that attendee, one with
/// `muted` updates the mute state, and one with neither means the stream's attendee left.
pub fn apply_stream_info(roster: &mut BTreeMap<u32, Participant>, info: &crate::proto::SdkAudioStreamIdInfoFrame) {
    for s in &info.streams {
        let Some(id) = s.audio_stream_id else { continue };
        if let Some(attendee) = &s.attendee_id {
            // A reconnect gets a new, higher stream id: forget the old one.
            roster.retain(|sid, p| *sid >= id || p.attendee_id != *attendee);
            let p = roster.entry(id).or_insert_with(|| Participant { attendee_id: attendee.clone(), external_user_id: String::new(), muted: false });
            p.external_user_id = s.external_user_id.clone().unwrap_or_else(|| attendee.clone());
        }
        if let Some(m) = s.muted {
            if let Some(p) = roster.get_mut(&id) {
                p.muted = m;
            }
        }
        if s.attendee_id.is_none() && s.muted.is_none() {
            roster.remove(&id);
        }
    }
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
    fn levels_like_the_sdk() {
        assert_eq!(level(None), 0.0);
        assert_eq!(level(Some(0)), 1.0);
        assert_eq!(level(Some(14)), 1.0);
        assert!((level(Some(28)) - 0.5).abs() < 1e-6);
        assert_eq!(level(Some(42)), 0.0);
        assert_eq!(level(Some(90)), 0.0);
    }

    #[test]
    fn roster_follows_stream_info() {
        use crate::proto::{SdkAudioStreamIdInfo as S, SdkAudioStreamIdInfoFrame as F};
        let join = |id: u32, att: &str| S { audio_stream_id: Some(id), attendee_id: Some(att.into()), external_user_id: Some(format!("T-R-{att}")), ..Default::default() };
        let mut r = BTreeMap::new();
        apply_stream_info(&mut r, &F { streams: vec![join(1, "a"), join(2, "b")] });
        assert_eq!(r.len(), 2);
        apply_stream_info(&mut r, &F { streams: vec![S { audio_stream_id: Some(2), muted: Some(true), ..Default::default() }] });
        assert!(r[&2].muted);
        // b leaves: an entry with only the stream id.
        apply_stream_info(&mut r, &F { streams: vec![S { audio_stream_id: Some(2), ..Default::default() }] });
        assert_eq!(r.keys().copied().collect::<Vec<_>>(), vec![1]);
        // a reconnects under stream 3: stream 1 goes away.
        apply_stream_info(&mut r, &F { streams: vec![join(3, "a")] });
        assert_eq!(r.keys().copied().collect::<Vec<_>>(), vec![3]);
    }

    #[test]
    fn video_subscriptions() {
        let src = |stream_id: u32, group_id: u32, attendee: &str, kbps: u32| VideoSource { stream_id, group_id, attendee_id: attendee.into(), external_user_id: String::new(), max_bitrate_kbps: kbps };
        let sources = vec![src(7, 3, "b", 300), src(8, 3, "b", 1200), src(4, 2, "me", 1000), src(9, 5, "c", 600), src(10, 6, "me#content", 900)];
        assert_eq!(choose_streams(&sources, "me", 9), vec![8, 9], "best per group, never our own");
        assert_eq!(choose_streams(&sources, "me", 1), vec![8]);
        // Slots keep their streams; freed ones are reused before new sections are added.
        assert_eq!(assign_slots(&[], &[8, 9]), vec![8, 9]);
        assert_eq!(assign_slots(&[8, 9], &[9]), vec![0, 9]);
        assert_eq!(assign_slots(&[0, 9], &[9, 11, 12]), vec![11, 9, 12]);
        assert_eq!(receive_ids(true, &[0, 9]), vec![0, 0, 9]);
        assert_eq!(receive_ids(false, &[5]), vec![5]);
    }

    #[test]
    fn a_lost_connection_is_one_that_failed_or_stays_disconnected() {
        let s = Duration::from_secs;
        assert!(media_lost(true, None));
        assert!(!media_lost(false, None), "connected");
        // Disconnected can come back by itself (a short network gap): wait a little first.
        assert!(!media_lost(false, Some(s(1))));
        assert!(!media_lost(false, Some(DISCONNECTED_GRACE - Duration::from_millis(1))));
        assert!(media_lost(false, Some(DISCONNECTED_GRACE)));
    }

    #[test]
    fn reconnects_are_bounded_and_back_off() {
        assert_eq!(reconnect_delay(0), Some(Duration::ZERO), "the first try at once");
        assert_eq!(reconnect_delay(1), Some(Duration::from_secs(2)));
        assert_eq!(reconnect_delay(2), Some(Duration::from_secs(4)));
        assert_eq!(reconnect_delay(MAX_RECONNECT_TRIES), None, "then the session ends");
    }

    #[test]
    fn codec_capability_names() {
        assert_eq!(codec_names(&[3, 1, 99]), vec!["H264ConstrainedBaselineProfile", "Vp8", "codec 99"]);
    }

    #[test]
    fn user_id_from_external_id() {
        let p = Participant { attendee_id: "a".into(), external_user_id: "T0TEST00001-R0CA8VAPS76-U0ANN000001".into(), muted: false };
        assert_eq!(p.user_id(), "U0ANN000001");
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

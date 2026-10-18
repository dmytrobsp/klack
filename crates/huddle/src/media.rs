//! WebRTC audio for a Chime meeting: a relay-only peer connection (Chime hands out TURN
//! credentials) with one Opus send/receive audio track.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use bytes::Bytes;
use rtc::peer_connection::configuration::media_engine::MIME_TYPE_OPUS;
use rtc::rtp_transceiver::rtp_sender::{RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters, RtpCodecKind};
use tokio::sync::{mpsc, watch};
use webrtc::media_stream::MediaStreamTrack;
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::media_stream::track_local::static_sample::TrackLocalStaticSample;
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};
use webrtc::peer_connection::{
    MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder, RTCIceGatheringState, RTCIceServer,
    RTCIceTransportPolicy, RTCPeerConnectionState, RTCSessionDescription, Registry, register_default_interceptors,
};

use crate::proto::SdkTurnCredentials;

/// Payload type we offer for Opus (the usual browser value).
pub const OPUS_PAYLOAD_TYPE: u8 = 111;

/// TURN servers we can use: Chime offers UDP and TLS-over-TCP; the stack relays over UDP.
pub fn udp_turn_uris(turn: &SdkTurnCredentials) -> Vec<String> {
    turn.uris.iter().filter(|u| u.starts_with("turn:") && !u.contains("transport=tcp")).cloned().collect()
}

struct Handler {
    gathered: watch::Sender<bool>,
    state: watch::Sender<RTCPeerConnectionState>,
    incoming: mpsc::UnboundedSender<Bytes>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            let _ = self.gathered.send(true);
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        tracing::info!("huddle media connection: {state}");
        let _ = self.state.send(state);
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        if track.kind().await != RtpCodecKind::Audio {
            return;
        }
        let tx = self.incoming.clone();
        tokio::spawn(async move {
            while let Some(evt) = track.poll().await {
                if let TrackRemoteEvent::OnRtpPacket(packet) = evt {
                    if tx.send(packet.payload.clone()).is_err() {
                        break;
                    }
                }
            }
        });
    }
}

/// The audio peer connection.
pub struct Media {
    pc: Arc<dyn PeerConnection>,
    track: Arc<TrackLocalStaticSample>,
    ssrc: u32,
    /// Opus payloads received from the meeting (the server's mix).
    pub incoming: mpsc::UnboundedReceiver<Bytes>,
    pub state: watch::Receiver<RTCPeerConnectionState>,
}

impl Media {
    /// Build the connection and return it with its SDP offer (after ICE gathering, so the
    /// offer carries the relay candidates).
    pub async fn offer(turn: &SdkTurnCredentials) -> Result<(Media, String)> {
        let codec = RTCRtpCodec { mime_type: MIME_TYPE_OPUS.to_owned(), clock_rate: 48000, channels: 2, sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(), rtcp_feedback: vec![] };
        let mut media_engine = MediaEngine::default();
        media_engine.register_codec(RTCRtpCodecParameters { rtp_codec: codec.clone(), payload_type: OPUS_PAYLOAD_TYPE, ..Default::default() }, RtpCodecKind::Audio)?;
        let registry = register_default_interceptors(Registry::new(), &mut media_engine)?;
        let uris = udp_turn_uris(turn);
        if uris.is_empty() {
            return Err(anyhow!("no UDP TURN server offered"));
        }
        let config = RTCConfigurationBuilder::new()
            .with_ice_servers(vec![RTCIceServer { urls: uris, username: turn.username.clone().unwrap_or_default(), credential: turn.password.clone().unwrap_or_default() }])
            .with_ice_transport_policy(RTCIceTransportPolicy::Relay)
            .build();
        let (gathered_tx, mut gathered) = watch::channel(false);
        let (state_tx, state) = watch::channel(RTCPeerConnectionState::New);
        let (incoming_tx, incoming) = mpsc::unbounded_channel();
        let handler = Arc::new(Handler { gathered: gathered_tx, state: state_tx, incoming: incoming_tx });
        let pc = PeerConnectionBuilder::new()
            .with_configuration(config)
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .with_handler(handler)
            .with_udp_addrs(vec!["0.0.0.0:0".to_string()])
            .build()
            .await?;
        let pc: Arc<dyn PeerConnection> = Arc::new(pc);

        let ssrc: u32 = rand_u32();
        let track = Arc::new(TrackLocalStaticSample::new(
            Instant::now(),
            MediaStreamTrack::new(
                "slacktui-audio".into(),
                "slacktui-audio-track".into(),
                "AmazonChimeExpressAudio".into(),
                RtpCodecKind::Audio,
                vec![RTCRtpEncodingParameters { rtp_coding_parameters: RTCRtpCodingParameters { ssrc: Some(ssrc), ..Default::default() }, codec, ..Default::default() }],
            ),
        )?);
        pc.add_track(Arc::clone(&track) as Arc<dyn TrackLocal>).await?;

        let offer = pc.create_offer(None).await?;
        pc.set_local_description(offer).await?;
        tokio::time::timeout(Duration::from_secs(10), gathered.wait_for(|g| *g)).await.map_err(|_| anyhow!("ICE gathering timed out"))??;
        let sdp = pc.local_description().await.ok_or_else(|| anyhow!("no local description"))?.sdp;
        Ok((Media { pc, track, ssrc, incoming, state }, sdp))
    }

    /// Apply the server's SDP answer (from SUBSCRIBE_ACK).
    pub async fn accept(&self, answer: String) -> Result<()> {
        self.pc.set_remote_description(RTCSessionDescription::answer(answer)?).await?;
        Ok(())
    }

    /// Send one Opus frame covering `duration` of audio.
    pub async fn send_opus(&self, frame: Vec<u8>, duration: Duration) -> Result<()> {
        let sample = rtc::media::Sample { data: Bytes::from(frame), timestamp: Instant::now(), duration, packet_timestamp: 0, prev_dropped_packets: 0, prev_padding_packets: 0 };
        self.track.write_sample(self.ssrc, OPUS_PAYLOAD_TYPE, &sample, &[]).await?;
        Ok(())
    }

    pub async fn close(&self) {
        let _ = self.pc.close().await;
    }
}

/// A random SSRC (no rand dependency: hash the time and the address of a local).
fn rand_u32() -> u32 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    h.finish() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_udp_turn() {
        let t = SdkTurnCredentials {
            uris: vec!["turn:a.chime.aws:3478?transport=udp".into(), "turns:a.chime.aws:443?transport=tcp".into(), "turn:b.chime.aws:3478?transport=tcp".into()],
            ..Default::default()
        };
        assert_eq!(udp_turn_uris(&t), vec!["turn:a.chime.aws:3478?transport=udp".to_string()]);
    }
}

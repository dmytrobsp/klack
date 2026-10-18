//! Chime meeting credentials (from Slack's `rooms.join`) and the signaling WebSocket.

use anyhow::{Context, Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::Value;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::proto::{self, SdkSignalFrame, sdk_signal_frame::Type};

/// The SDK version the Slack web client ships; the server sees it in the JOIN frame.
pub const CHIME_SDK_VERSION: &str = "3.32.0";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MediaPlacement {
    pub audio_host_url: String,
    pub signaling_url: String,
    #[serde(default)]
    pub turn_control_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Meeting {
    pub meeting_id: String,
    pub media_placement: MediaPlacement,
    #[serde(default)]
    pub media_region: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Attendee {
    pub attendee_id: String,
    pub external_user_id: String,
    pub join_token: String,
}

impl std::fmt::Debug for Attendee {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the join token.
        f.debug_struct("Attendee").field("attendee_id", &self.attendee_id).field("external_user_id", &self.external_user_id).finish_non_exhaustive()
    }
}

/// What Slack's `rooms.join` hands out for a huddle.
#[derive(Debug, Clone)]
pub struct Credentials {
    /// Slack room id (`R…`).
    pub room_id: String,
    pub meeting: Meeting,
    pub attendee: Attendee,
}

impl Credentials {
    /// From a `rooms.join` response: `call.call_id` and `call.free_willy.{meeting,attendee}`.
    pub fn from_rooms_join(v: &Value) -> Result<Credentials> {
        let call = &v["call"];
        let fw = &call["free_willy"];
        if fw.is_null() {
            return Err(anyhow!("rooms.join returned no Chime meeting (free_willy)"));
        }
        Ok(Credentials {
            room_id: call["call_id"].as_str().unwrap_or_default().to_string(),
            meeting: serde_json::from_value(fw["meeting"].clone()).context("meeting")?,
            attendee: serde_json::from_value(fw["attendee"].clone()).context("attendee")?,
        })
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// A frame of `kind` with the current timestamp; fill the payload field afterwards.
pub fn frame(kind: Type) -> SdkSignalFrame {
    SdkSignalFrame { timestamp_ms: now_ms(), r#type: kind as i32, ..Default::default() }
}

/// The JOIN frame the web SDK sends (audio only, no video).
pub fn join_frame(audio_session_id: u64) -> SdkSignalFrame {
    let mut f = frame(Type::Join);
    f.join = Some(proto::SdkJoinFrame {
        protocol_version: Some(2),
        flags: Some(proto::JOIN_FLAG_HAS_STREAM_UPDATE),
        client_details: Some(proto::SdkClientDetails {
            app_name: Some("slacktui".into()),
            app_version: Some(env!("CARGO_PKG_VERSION").into()),
            platform_name: Some("slacktui".into()),
            platform_version: Some(env!("CARGO_PKG_VERSION").into()),
            client_source: Some("amazon-chime-sdk-js".into()),
            chime_sdk_version: Some(CHIME_SDK_VERSION.into()),
            os_name: Some(std::env::consts::OS.into()),
            ..Default::default()
        }),
        audio_session_id: Some(audio_session_id),
        wants_compressed_sdp: Some(false),
        ..Default::default()
    });
    f
}

/// The SUBSCRIBE frame (what the web SDK sends): our audio, and with `send_video` our
/// camera too (stream 2, group 2, as the Chime SDK does); `receive_stream_ids`: one entry
/// per video section of the offer, 0 for the camera's.
pub fn subscribe_frame(sdp_offer: String, audio_host: &str, attendee_id: &str, muted: bool, send_video: bool, receive_stream_ids: Vec<u32>) -> SdkSignalFrame {
    let mut f = frame(Type::Subscribe);
    let mut send_streams = vec![proto::SdkStreamDescriptor {
        media_type: Some(proto::SdkStreamMediaType::Audio as i32),
        track_label: Some("AmazonChimeExpressAudio".into()),
        attendee_id: Some(attendee_id.into()),
        stream_id: Some(1),
        group_id: Some(1),
        framerate: Some(15),
        max_bitrate_kbps: Some(600),
        avg_bitrate_bps: Some(400_000),
        ..Default::default()
    }];
    if send_video {
        send_streams.push(proto::SdkStreamDescriptor {
            media_type: Some(proto::SdkStreamMediaType::Video as i32),
            track_label: Some(crate::media::VIDEO_TRACK_LABEL.into()),
            attendee_id: Some(attendee_id.into()),
            stream_id: Some(2),
            group_id: Some(2),
            framerate: Some(crate::video::TEST_FPS),
            max_bitrate_kbps: Some(VIDEO_MAX_KBPS),
            ..Default::default()
        });
    }
    f.sub = Some(proto::SdkSubscribeFrame {
        duplex: Some(if send_video { proto::SdkStreamServiceType::Duplex } else { proto::SdkStreamServiceType::Rx } as i32),
        send_streams,
        receive_stream_ids,
        sdp_offer: Some(sdp_offer),
        audio_host: Some(audio_host.into()),
        audio_checkin: Some(false),
        audio_muted: Some(muted),
        ..Default::default()
    });
    f
}

/// The camera's bitrate cap we announce (the encoder aims lower).
pub const VIDEO_MAX_KBPS: u32 = 1000;

/// PONG for a server PING.
pub fn pong_frame(ping_id: u32) -> SdkSignalFrame {
    let mut f = frame(Type::PingPong);
    f.ping_pong = Some(proto::SdkPingPongFrame { r#type: proto::SdkPingPongType::Pong as i32, ping_id });
    f
}

/// AUDIO_CONTROL: tell the server we are (un)muted.
pub fn mute_frame(muted: bool) -> SdkSignalFrame {
    let mut f = frame(Type::AudioControl);
    f.audio_control = Some(proto::SdkAudioControlFrame { muted: Some(muted) });
    f
}

pub type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The signaling connection.
pub struct Signaling {
    ws: Socket,
}

impl Signaling {
    pub async fn connect(creds: &Credentials) -> Result<Signaling> {
        let url = format!("{}?X-Chime-Control-Protocol-Version=3&X-Amzn-Chime-Send-Close-On-Error=1", creds.meeting.media_placement.signaling_url);
        let mut req = url.into_client_request()?;
        let protocols = format!("_aws_wt_session, {}", creds.attendee.join_token);
        req.headers_mut().insert("Sec-WebSocket-Protocol", HeaderValue::from_str(&protocols)?);
        let (ws, _) = tokio_tungstenite::connect_async(req).await.context("signaling websocket")?;
        Ok(Signaling { ws })
    }

    pub async fn send(&mut self, frame: &SdkSignalFrame) -> Result<()> {
        self.ws.send(WsMessage::Binary(proto::encode_frame(frame).into())).await?;
        Ok(())
    }

    /// Next signaling frame; None when the socket closes.
    pub async fn next(&mut self) -> Option<Result<SdkSignalFrame>> {
        loop {
            match self.ws.next().await? {
                Ok(WsMessage::Binary(data)) => return Some(proto::decode_frame(&data).map_err(Into::into)),
                Ok(WsMessage::Close(reason)) => {
                    tracing::info!("signaling closed: {reason:?}");
                    return None;
                }
                Ok(_) => continue,
                Err(e) => return Some(Err(e.into())),
            }
        }
    }

    pub async fn close(mut self) {
        let _ = self.ws.close(None).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_from_rooms_join() {
        let v = serde_json::json!({"call":{"call_id":"R1","free_willy":{
            "meeting":{"MeetingId":"m-1","MediaRegion":"eu-central-1","MediaPlacement":{"AudioHostUrl":"a.chime.aws:3478","SignalingUrl":"wss://s.chime.aws/control/m-1","TurnControlUrl":"https://t"}},
            "attendee":{"AttendeeId":"att-1","ExternalUserId":"T-R1-U","JoinToken":"secret"}}}});
        let c = Credentials::from_rooms_join(&v).unwrap();
        assert_eq!(c.room_id, "R1");
        assert_eq!(c.meeting.media_placement.audio_host_url, "a.chime.aws:3478");
        assert!(!format!("{c:?}").contains("secret"), "the join token must not appear in Debug output");
        assert!(Credentials::from_rooms_join(&serde_json::json!({"call":{}})).is_err());
    }

    #[test]
    fn join_frame_fields() {
        let f = join_frame(7);
        let j = f.join.unwrap();
        assert_eq!(j.protocol_version, Some(2));
        assert_eq!(j.flags, Some(proto::JOIN_FLAG_HAS_STREAM_UPDATE));
        assert_eq!(j.client_details.unwrap().chime_sdk_version.as_deref(), Some(CHIME_SDK_VERSION));
    }
}

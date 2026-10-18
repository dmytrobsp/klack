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

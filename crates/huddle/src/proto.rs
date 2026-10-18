//! The subset of the Chime SDK signaling protocol (`SignalingProtocol.proto`, amazon-chime-sdk-js
//! v3.32.0, proto2) that audio huddles need. Field tags match the upstream file; prost skips
//! fields we do not declare.

#![allow(clippy::derive_partial_eq_without_eq)]

/// Prefix byte of every signaling WebSocket message (`FRAME_TYPE_RTC`).
pub const FRAME_TYPE_RTC: u8 = 5;

#[derive(Clone, PartialEq, prost::Message)]
pub struct SdkSignalFrame {
    #[prost(uint64, required, tag = "1")]
    pub timestamp_ms: u64,
    #[prost(enumeration = "sdk_signal_frame::Type", required, tag = "2")]
    pub r#type: i32,
    #[prost(message, optional, tag = "3")]
    pub error: Option<SdkErrorFrame>,
    #[prost(message, optional, tag = "4")]
    pub join: Option<SdkJoinFrame>,
    #[prost(message, optional, tag = "5")]
    pub joinack: Option<SdkJoinAckFrame>,
    #[prost(message, optional, tag = "6")]
    pub sub: Option<SdkSubscribeFrame>,
    #[prost(message, optional, tag = "7")]
    pub suback: Option<SdkSubscribeAckFrame>,
    #[prost(message, optional, tag = "8")]
    pub index: Option<SdkIndexFrame>,
    #[prost(message, optional, tag = "11")]
    pub leave: Option<SdkLeaveFrame>,
    #[prost(message, optional, tag = "12")]
    pub leave_ack: Option<SdkLeaveAckFrame>,
    #[prost(message, optional, tag = "17")]
    pub audio_control: Option<SdkAudioControlFrame>,
    #[prost(message, optional, tag = "18")]
    pub audio_metadata: Option<SdkAudioMetadataFrame>,
    #[prost(message, optional, tag = "19")]
    pub audio_stream_id_info: Option<SdkAudioStreamIdInfoFrame>,
    #[prost(message, optional, tag = "20")]
    pub ping_pong: Option<SdkPingPongFrame>,
    #[prost(message, optional, tag = "21")]
    pub audio_status: Option<SdkAudioStatusFrame>,
}

pub mod sdk_signal_frame {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
    #[repr(i32)]
    pub enum Type {
        Join = 1,
        JoinAck = 2,
        Subscribe = 3,
        SubscribeAck = 4,
        Index = 5,
        Pause = 7,
        Resume = 8,
        Leave = 9,
        LeaveAck = 10,
        Bitrates = 13,
        AudioControl = 16,
        AudioMetadata = 17,
        AudioStreamIdInfo = 18,
        PingPong = 19,
        AudioStatus = 20,
        ClientMetric = 21,
        DataMessage = 22,
        RemoteVideoUpdate = 24,
        PrimaryMeetingJoin = 25,
        PrimaryMeetingJoinAck = 26,
        PrimaryMeetingLeave = 27,
        Notification = 34,
        MeetingSessionTiming = 43,
    }
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SdkErrorFrame {
    #[prost(uint32, optional, tag = "1")]
    pub status: Option<u32>,
    #[prost(string, optional, tag = "2")]
    pub description: Option<String>,
}

/// Bits of `SdkJoinFrame::flags`.
pub const JOIN_FLAG_HAS_STREAM_UPDATE: u32 = 2;

#[derive(Clone, PartialEq, prost::Message)]
pub struct SdkClientDetails {
    #[prost(string, optional, tag = "1")]
    pub app_name: Option<String>,
    #[prost(string, optional, tag = "2")]
    pub app_version: Option<String>,
    #[prost(string, optional, tag = "3")]
    pub device_model: Option<String>,
    #[prost(string, optional, tag = "4")]
    pub device_make: Option<String>,
    #[prost(string, optional, tag = "5")]
    pub platform_name: Option<String>,
    #[prost(string, optional, tag = "6")]
    pub platform_version: Option<String>,
    #[prost(string, optional, tag = "7")]
    pub client_source: Option<String>,
    #[prost(string, optional, tag = "8")]
    pub chime_sdk_version: Option<String>,
    #[prost(string, optional, tag = "9")]
    pub client_utc_offset: Option<String>,
    #[prost(string, optional, tag = "10")]
    pub os_name: Option<String>,
    #[prost(string, optional, tag = "11")]
    pub os_version: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum SdkServerSideNetworkAdaption {
    Default = 1,
    None = 2,
    BandwidthProbing = 3,
    BandwidthProbingAndVideoQualityAdaption = 4,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SdkJoinFrame {
    #[prost(uint32, optional, tag = "1", default = "2")]
    pub protocol_version: Option<u32>,
    #[prost(uint32, optional, tag = "2")]
    pub max_num_of_videos: Option<u32>,
    #[prost(uint32, optional, tag = "3")]
    pub flags: Option<u32>,
    #[prost(message, optional, tag = "4")]
    pub client_details: Option<SdkClientDetails>,
    #[prost(uint64, optional, tag = "6")]
    pub audio_session_id: Option<u64>,
    #[prost(bool, optional, tag = "7")]
    pub wants_compressed_sdp: Option<bool>,
    #[prost(enumeration = "SdkServerSideNetworkAdaption", optional, tag = "10")]
    pub server_side_network_adaption: Option<i32>,
    #[prost(enumeration = "SdkServerSideNetworkAdaption", repeated, packed = "false", tag = "11")]
    pub supported_server_side_network_adaptions: Vec<i32>,
    #[prost(bool, optional, tag = "12")]
    pub wants_all_temporal_layers_in_index: Option<bool>,
    #[prost(bool, optional, tag = "13")]
    pub disable_periodic_keyframe_request_on_content_sender: Option<bool>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SdkJoinAckFrame {
    #[prost(message, optional, tag = "1")]
    pub turn_credentials: Option<SdkTurnCredentials>,
    #[prost(uint32, optional, tag = "2")]
    pub video_subscription_limit: Option<u32>,
    #[prost(bool, optional, tag = "3")]
    pub wants_compressed_sdp: Option<bool>,
    #[prost(enumeration = "SdkServerSideNetworkAdaption", optional, tag = "4")]
    pub default_server_side_network_adaption: Option<i32>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SdkLeaveFrame {}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SdkLeaveAckFrame {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum SdkStreamServiceType {
    Rx = 1,
    Tx = 2,
    Duplex = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum SdkStreamMediaType {
    Audio = 1,
    Video = 2,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SdkSubscribeFrame {
    #[prost(enumeration = "SdkStreamServiceType", optional, tag = "1", default = "Rx")]
    pub duplex: Option<i32>,
    #[prost(message, repeated, tag = "2")]
    pub send_streams: Vec<SdkStreamDescriptor>,
    #[prost(uint32, repeated, packed = "false", tag = "3")]
    pub receive_stream_ids: Vec<u32>,
    #[prost(string, optional, tag = "4")]
    pub sdp_offer: Option<String>,
    #[prost(string, optional, tag = "5")]
    pub audio_host: Option<String>,
    #[prost(bool, optional, tag = "6")]
    pub audio_checkin: Option<bool>,
    #[prost(bool, optional, tag = "7")]
    pub audio_muted: Option<bool>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SdkSubscribeAckFrame {
    #[prost(enumeration = "SdkStreamServiceType", optional, tag = "1")]
    pub duplex: Option<i32>,
    #[prost(message, repeated, tag = "2")]
    pub allocations: Vec<SdkStreamAllocation>,
    #[prost(string, optional, tag = "3")]
    pub sdp_answer: Option<String>,
    #[prost(message, repeated, tag = "4")]
    pub tracks: Vec<SdkTrackMapping>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SdkIndexFrame {
    #[prost(bool, optional, tag = "1")]
    pub at_capacity: Option<bool>,
    #[prost(message, repeated, tag = "2")]
    pub sources: Vec<SdkStreamDescriptor>,
    #[prost(uint32, repeated, packed = "false", tag = "3")]
    pub paused_at_source_ids: Vec<u32>,
    #[prost(uint32, optional, tag = "4")]
    pub num_participants: Option<u32>,
    /// Codecs everyone in the meeting can receive; senders keep to these.
    #[prost(enumeration = "SdkVideoCodecCapability", repeated, packed = "false", tag = "5")]
    pub supported_receive_codec_intersection: Vec<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum SdkVideoCodecCapability {
    Vp8 = 1,
    H264BaselineProfile = 2,
    H264ConstrainedBaselineProfile = 3,
    H264MainProfile = 4,
    H264HighProfile = 5,
    H264ConstrainedHighProfile = 6,
    Vp9Profile0 = 8,
    Av1MainProfile = 11,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SdkStreamDescriptor {
    #[prost(uint32, optional, tag = "1")]
    pub stream_id: Option<u32>,
    #[prost(uint32, optional, tag = "2")]
    pub framerate: Option<u32>,
    #[prost(uint32, optional, tag = "3")]
    pub max_bitrate_kbps: Option<u32>,
    #[prost(string, optional, tag = "4")]
    pub track_label: Option<String>,
    #[prost(uint32, optional, tag = "6")]
    pub group_id: Option<u32>,
    #[prost(uint32, optional, tag = "7")]
    pub avg_bitrate_bps: Option<u32>,
    #[prost(string, optional, tag = "8")]
    pub attendee_id: Option<String>,
    #[prost(enumeration = "SdkStreamMediaType", optional, tag = "9")]
    pub media_type: Option<i32>,
    #[prost(string, optional, tag = "10")]
    pub external_user_id: Option<String>,
    #[prost(uint32, optional, tag = "11")]
    pub width: Option<u32>,
    #[prost(uint32, optional, tag = "12")]
    pub height: Option<u32>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SdkStreamAllocation {
    #[prost(string, optional, tag = "1")]
    pub track_label: Option<String>,
    #[prost(uint32, optional, tag = "2")]
    pub stream_id: Option<u32>,
    #[prost(uint32, optional, tag = "3")]
    pub group_id: Option<u32>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SdkTrackMapping {
    #[prost(uint32, optional, tag = "1")]
    pub stream_id: Option<u32>,
    #[prost(uint32, optional, tag = "2")]
    pub ssrc: Option<u32>,
    #[prost(string, optional, tag = "3")]
    pub track_label: Option<String>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SdkAudioControlFrame {
    #[prost(bool, optional, tag = "1")]
    pub muted: Option<bool>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SdkAudioMetadataFrame {
    #[prost(message, repeated, tag = "1")]
    pub attendee_states: Vec<SdkAudioAttendeeState>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SdkAudioAttendeeState {
    #[prost(uint32, optional, tag = "1")]
    pub audio_stream_id: Option<u32>,
    #[prost(uint32, optional, tag = "2")]
    pub volume: Option<u32>,
    #[prost(bool, optional, tag = "3")]
    pub muted: Option<bool>,
    #[prost(uint32, optional, tag = "4")]
    pub signal_strength: Option<u32>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SdkAudioStreamIdInfoFrame {
    #[prost(message, repeated, tag = "1")]
    pub streams: Vec<SdkAudioStreamIdInfo>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SdkAudioStreamIdInfo {
    #[prost(uint32, optional, tag = "1")]
    pub audio_stream_id: Option<u32>,
    #[prost(string, optional, tag = "2")]
    pub attendee_id: Option<String>,
    #[prost(bool, optional, tag = "3")]
    pub muted: Option<bool>,
    #[prost(string, optional, tag = "4")]
    pub external_user_id: Option<String>,
    #[prost(bool, optional, tag = "5")]
    pub dropped: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum SdkPingPongType {
    Ping = 1,
    Pong = 2,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SdkPingPongFrame {
    #[prost(enumeration = "SdkPingPongType", required, tag = "1")]
    pub r#type: i32,
    #[prost(uint32, required, tag = "2")]
    pub ping_id: u32,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SdkAudioStatusFrame {
    #[prost(uint32, optional, tag = "1")]
    pub audio_status: Option<u32>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SdkTurnCredentials {
    #[prost(string, optional, tag = "1")]
    pub username: Option<String>,
    #[prost(string, optional, tag = "2")]
    pub password: Option<String>,
    #[prost(uint32, optional, tag = "3")]
    pub ttl: Option<u32>,
    #[prost(string, repeated, tag = "4")]
    pub uris: Vec<String>,
}

/// Encode a frame for the WebSocket: the `FRAME_TYPE_RTC` byte, then the protobuf.
pub fn encode_frame(frame: &SdkSignalFrame) -> Vec<u8> {
    let mut out = vec![FRAME_TYPE_RTC];
    prost::Message::encode(frame, &mut out).expect("Vec has room");
    out
}

/// Decode a WebSocket message (prefix byte, then the protobuf).
pub fn decode_frame(data: &[u8]) -> Result<SdkSignalFrame, prost::DecodeError> {
    <SdkSignalFrame as prost::Message>::decode(data.get(1..).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_frame_round_trip() {
        let frame = SdkSignalFrame {
            timestamp_ms: 1,
            r#type: sdk_signal_frame::Type::Join as i32,
            join: Some(SdkJoinFrame { protocol_version: Some(2), flags: Some(JOIN_FLAG_HAS_STREAM_UPDATE), audio_session_id: Some(42), ..Default::default() }),
            ..Default::default()
        };
        let bytes = encode_frame(&frame);
        assert_eq!(bytes[0], FRAME_TYPE_RTC);
        // timestamp (field 1, varint) then type (field 2, varint = 1).
        assert_eq!(&bytes[1..5], &[0x08, 0x01, 0x10, 0x01]);
        assert_eq!(decode_frame(&bytes).unwrap(), frame);
    }
}

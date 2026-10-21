# Huddles: how the Slack web client does them

Source: static reading of the web client bundles served for `app.slack.com/client/<team>`
(`client-boot.*.min.js`, `gantry-v2-async-client-boot-shared.*`, `gantry-v2-async-client-boot-vendors.*`,
`gantry-v2-async-amazon-chime-sdk-js-experimental.*`), downloaded 2026-10-18. The CDN serves them
gzip-compressed; decompress before searching. Labels: OBSERVED (seen in code), INFERRED.

## Media stack

- OBSERVED: the client ships the **Amazon Chime SDK for JavaScript** as an async chunk
  (`amazon-chime-sdk-js-experimental`).
- OBSERVED: a call/room API response is turned into Chime credentials by
  `handleCallAPIResponse`: if the response has `free_willy`, it yields
  `{meeting: free_willy.meeting, attendee: free_willy.attendee}` — the shapes of Chime's
  `CreateMeeting` / `CreateAttendee` (`MeetingId`, `MediaPlacement{AudioHostUrl, SignalingUrl,
  TurnControlUrl, …}`, `AttendeeId`, `JoinToken`). Without `free_willy` it is the legacy
  "screenhero" calls server (`token, token_expiry, hostname, port, turn_auth{username,password}, turn_ports`).

## Slack API

| Method | Args (OBSERVED) | Use |
|---|---|---|
| `rooms.join` | `id?` (room id), `channel_id?` | Join (or start, INFERRED when only `channel_id`) a huddle. Response `call` → `handleCallAPIResponse(call)`; room id = `call.call_id`. |
| `rooms.leave` | `channel_id, call_id, attendee_id, reason` | Leave. Benign failures are ignored by the client. |
| `screenhero.rooms.join` | `room, use_room_scoped_token: true, regions: ""` | Legacy calls (call ids, not huddles). |
| `rooms.request` | `app, channel` | Third-party conference apps (Zoom etc.), **not** huddles. |
| `rooms.notifyMember`, `huddles.knock`, `huddles.knockResponse`, `huddles.get`, `huddles.history`, `huddles.readiness`, `huddles.update`, `huddles.recording.*`, `rooms.startTranscription` | — | Invites, knocking, metadata, recording, transcripts. |

## Plan for klack (INFERRED, to be verified step by step)

1. `rooms.join` with `channel_id` → Chime meeting + attendee (never print `JoinToken`).
2. Chime signaling: WebSocket to `MediaPlacement.SignalingUrl` with protobuf `SdkSignalFrame`s
   (JOIN → JOIN_ACK, SUBSCRIBE with an SDP offer → SUBSCRIBE_ACK with the answer, INDEX for the roster).
   The protobuf schema is in the Chime SDK chunk.
3. WebRTC audio (ICE via Chime TURN, DTLS-SRTP, Opus) with a Rust WebRTC stack; cpal for devices.
4. Verify by joining the same huddle with both test accounts and passing a tone from A to B.

## Verified against the live API (2026-10-18)

- OBSERVED: `rooms.join channel_id=<C…>` on a channel without a huddle **starts** one: response
  `{call: {call_id: "R…", free_willy: {meeting, attendee}}, huddle: {id, channels, thread_root_ts, …}, canvas: {…}}`.
  `meeting.MediaPlacement` has `SignalingUrl` (`wss://signal.….chime.aws/control/<MeetingId>`),
  `AudioHostUrl` (`<id>.k.m3.<region>.app.chime.aws:3478`), `TurnControlUrl`, screen-share URLs, `EventIngestionUrl`.
  `attendee` has `AttendeeId`, `ExternalUserId` (`<team>-<room>-<user>`), `JoinToken` (secret).
- OBSERVED: `rooms.leave` with the client's own args (`reason=user_initiated`) answers `feature_not_enabled`
  in this workspace; the web client treats that as benign. Participation follows the Chime attendee.
- OBSERVED: `huddles.get huddle_id=R…` returns the huddle (participants, has_ended, …).

## Chime signaling (SDK v3.32.0, from the bundled SDK + the public `SignalingProtocol.proto` at that tag)

- WebSocket URL: `<SignalingUrl>?X-Chime-Control-Protocol-Version=3&X-Amzn-Chime-Send-Close-On-Error=1`,
  subprotocols `["_aws_wt_session", <JoinToken>]`.
- Every binary message is one byte `5` (FRAME_TYPE_RTC) + a protobuf `SdkSignalFrame`.
- JOIN: `protocol_version=2`, `flags=HAS_STREAM_UPDATE(2)`, `client_details{platform_name, platform_version,
  client_source, chime_sdk_version="3.32.0", client_utc_offset, os_name…}`, random `audio_session_id`.
- SUBSCRIBE (audio): `duplex=RX`, `sdp_offer`, `audio_host=<AudioHostUrl>`, `audio_checkin=false`, `audio_muted`,
  one send stream `{media_type=AUDIO, track_label="AmazonChimeExpressAudio", attendee_id, stream_id=1,
  group_id=1, framerate=15, max_bitrate_kbps=600, avg_bitrate_bps=400000}`; SUBSCRIBE_ACK carries `sdp_answer`.
- JOIN_ACK may carry `turn_credentials{username,password,ttl,uris}`; otherwise `TurnControlUrl`.
- OBSERVED (klack `ctl huddle-signal`, 2026-10-18): with the frames above the real Chime service answers
  PING (id 1) → JOIN_ACK (2 TURN uris, ttl 300, video limit 25) → INDEX (num_participants 1) and LEAVE → LEAVE_ACK.
  The server pings; the client must answer PONG with the same id.
- OBSERVED (klack `ctl huddle-media`, 2026-10-18): WebRTC audio works with webrtc-rs 0.21 using only the
  UDP TURN uri from JOIN_ACK (`RTCIceTransportPolicy::Relay`), an Opus/48000/2 sendrecv track labelled
  `AmazonChimeExpressAudio`, and SUBSCRIBE as above; SUBSCRIBE_ACK's `sdp_answer` is accepted as is.
  Two attendees (A sending a 440 Hz sine, B recording) in one huddle: B's recording has the tone at −20 dB in
  the 440 Hz band vs −53 dB at 1 kHz. The server mix excludes your own audio. AUDIO_STREAM_ID_INFO gives the
  roster (`external_user_id` = `<team>-<room>-<user>`).

## Huddles in threads (2026-10-18)

The message menu "Start huddle in thread" dispatches the same join thunk with `threadTs`; the
"Join a calls / huddle room" fetcher sends `rooms.join` with `{id?, channel_id, thread_ts, regions,
multidevice}`. Verified live: `rooms.join channel_id=D… thread_ts=<message ts>` starts a huddle whose
`sh_room_*` events carry `thread_root_ts` = that ts, and Slack posts "A huddle started" into the thread.
Channel huddles report `thread_root_ts` too (their own "A huddle started" message).

## Video (2026-10-18; Chime SDK v3.32.0 source + live)

From the SDK source (`DefaultSignalingClient.subscribe`, `DefaultVideoStreamIndex`, `DefaultTransceiverController`,
`SubscribeAndReceiveSubscribeAckTask`, `AllHighestVideoBandwidthPolicy`):
- Transceivers: audio (mid 0), then the camera (mid 1): `inactive` until a camera is on, then **sendrecv**; one
  `recvonly` video section per subscribed remote stream after that.
- SUBSCRIBE with a camera: `duplex=DUPLEX(3)` and a second send stream `{media_type=VIDEO, track_label=
  "AmazonChimeExpressVideo", stream_id=2, group_id=2, framerate=15, max_bitrate_kbps≈1500, attendee_id}`.
- `receive_stream_ids`: one entry per `m=video` section in SDP order — the stream id for a recvonly
  section, **0** for the camera (send or inactive) section.
- INDEX `sources`: video sources of the others (`stream_id`, `group_id` = publisher's tile, `attendee_id`,
  `external_user_id`, `max_bitrate_kbps`); subscribe by **stream_id**, the highest-bitrate source per group.
- SUBSCRIBE_ACK `tracks` (stream_id, ssrc, track_label) map received tracks; the SDK matches a remote
  track by its MediaStream id, which by convention is `v_<mid>`. `allocations[0]` is our camera's stream.
- No codec field: the server reads codecs from the SDP; the SDK prefers VP9, H.264 CB, VP8. Keyframes: RTCP
  PLI/FIR only (no signaling frame). OBSERVED (C1): when someone subscribes to our camera or share the
  server sends us a PLI at once and another ~3 s later; answering them gets the first picture to the new
  receiver ~0.25 s after it connects (without: up to the 2 s keyframe interval).

OBSERVED (klack `ctl huddle-media --video-test`): an offer with an H.264 (42e01f, packetization-mode=1)
sendrecv section at mid 1 is answered sendrecv H.264; SUBSCRIBE_ACK allocates `{AmazonChimeExpressVideo,
stream_id 1, group_id 1}`; the other attendee's INDEX then lists `{stream_id 1, group_id 1, attendee_id B,
max_bitrate_kbps 1000}`; the sender's own INDEX leaves itself out. INDEX can arrive **before** SUBSCRIBE_ACK.

## Screen share (2026-10-18)

- OBSERVED (Slack client-boot bundle): `isScreenshare(e){return e?.includes("#content")}` — a huddle screen share
  is a Chime **content share**. SDK (`DefaultContentShareController.createContentShareMeetingSessionConfigure`):
  a second session with `attendeeId + "#content"`, `joinToken + "#content"`, the same external user id, and
  `NoVideoDownlinkBandwidthPolicy` (it receives no video).
- OBSERVED (klack `ctl huddle-media --share-test`): such a session joins with the same rooms.join credentials
  plus the suffix; others' INDEX lists `{attendee_id "<id>#content", external_user_id <same as the person>}`, and
  the content attendee may also appear in AUDIO_STREAM_ID_INFO (muted).

## Codecs with real clients (2026-10-18)

- OBSERVED: INDEX field 5 `supported_receive_codec_intersection` lists all eight codecs while only full clients are
  in the meeting, and narrows to `H264_CONSTRAINED_BASELINE_PROFILE` once a client offering only H.264 CB (klack)
  is subscribed. A real Slack web client (Chrome 155, fake camera) then sends H.264: klack decoded 324 pictures
  (720×720) of Chrome's fake camera, ~3 s after subscribing. The web client also shows klack's H.264 camera.
  So VP8/VP9 decoding is not needed while klack is in the call.

## Invitations (ringing), OBSERVED 2026-10-19

- When someone starts a huddle in a DM with you, RTM sends `{"type":"huddle_invite","call_id":"R…",
  "channel_id":"D…","sender_user_id":"U…","is_guest":false,"team_id":…,"event_ts":…,"free_willy":{…}}`
  (`free_willy` = the Chime meeting and *your* attendee with its JoinToken — secret, never log it), right
  with the "A huddle started" message, before `sh_room_join`.
- `{"type":"huddle_invite_cancel","channel_id":"D…"}` ends it: when the huddle ends (~7 s after the caller
  left) or when you decline. While the caller stays, Slack does not stop the invitation by itself (none
  within 60 s) — the client decides how long to ring.
- Decline (web client `respondToHuddleInvite`): `rooms.inviteResponse response=decline channel_id=D…
  room_id=R…` → `{"ok":true}`, then `huddle_invite_cancel`. Other responses: `joining_soon`; accepting is
  just `rooms.join`.

## Huddle lists: live and recent (2026-10-21, VW6)

Research: `slack-rea/notes/huddles-list.md` §1–2. Verified live on the test workspace with klack's
`ctl huddles --shape` (prints keys, types and lengths only), while B was alone in a channel huddle
(`ctl huddle-media`, synthetic media).

- **Flannel** (the edge API): `api.getFlannelHttpUrl include_external_workspaces=true` → `{ok, url, ttl_seconds}`,
  `url` = `https://edgeapi.slack.com/cache/<team or org>`. Calls are **POST** `<url>/<method>` with a JSON body
  `{token, …args}` and the `d` cookie (GET answers 405); answers carry `ok`. klack keeps the url for `ttl_seconds`
  and sends the session only to a `https://*.slack.com` / `*.slack-gov.com` url.
- OBSERVED: `huddles/list {}` → `{ok, huddles: [object]}`, and `huddles/info {channel_ids: [≤100]}` →
  `{ok, huddles: [object]}` with the **same object** (A, who was not in the huddle, saw it):
  `active_members` [user ids], `dropped_members` [], `call_id` (R…, 11), `channel_id` (11), `created_by`,
  `background_id`, `expiration` (int), `start_date` (int s), `updated` (int), `is_private`, `is_scheduled` (bool),
  `knocks` {}, `last_invite_status_by_user` {}, `pending_invitees` {}, `locale` (5), `name` (empty),
  `thread_root_ts`, `recording {can_record_summary, is_paused, recording_user, summary, transcript}`.
  No `room_id` (the research note's guess). With no huddle running both answer `{ok, huddles: []}`.
  klack: `members` = `active_members` − `dropped_members` (as the web client counts them).
- OBSERVED: `huddles.history` items as in the research note (`id, channels [1], channel_type, created_by,
  date_start, date_end, has_ended, name, thread_root_ts, huddle_link, participant_history, participants,
  access_type_by_user, last_invite_status_by_user, pending_invitees, attached_file_ids, background_id,
  external_unique_id, prototypes, recording`; `missed_participants` only when someone missed it), and
  `response_metadata.next_cursor` (80 chars while more pages follow).
- OBSERVED: the huddle B was alone in (125 s) was **not** in B's `huddles.history` while it ran nor ~1.5 min after it
  ended, though `huddles.get` had it (`has_ended` true, one person in `participant_history`). INFERRED: huddles
  nobody else joined are left out of the history.
- Unknown: whether a running huddle with two or more people is in `huddles.history` (with `has_ended` false) — klack
  keeps `ended` so a list can leave such items out.
- OBSERVED (2026-10-21, VW7, `ctl listen` keys only): RTM `sh_room_join` / `sh_room_update` / `sh_room_leave` are
  `{type, user, huddle {channel_id}, room {…}, ts, event_ts}`, `room` with the 37 keys of a `huddle_thread` message's
  room, `date_start` (int s, set) among them. A DM huddle B started and A never answered (B alone) did not show in A's
  `huddles.history`, not even with `missed_huddles_only=true`, though older missed DM huddles are there — the same
  rule as above. `ended_huddle_update` / `huddle_start` did not arrive at A for those solo huddles (A was in neither).

## Your own huddle state ("In a huddle"), 2026-10-21 (VW8)

Research: `slack-rea/notes/huddles-list.md` §4, re-read in the web client bundles served on 2026-10-21
(`client-boot.*`: the huddle orchestrator and the older `HuddleStateManager`); live shapes on the test workspace.

- OBSERVED (code): once the huddle is connected (`STARTED`) the client starts a timer that calls, right away and
  then every 240 s ±10 % (random each time), `users.profile.set` with `profile = JSON {"huddle_state_expiration_ts":
  <now in unix seconds, with a fraction> + 300}` and reason `increment-huddle-state-expiration-ts` (the client's
  `reason` is the `_x_reason` argument elsewhere; INFERRED here). Nothing else of the profile: never
  `huddle_state`, never the status. Each tick checks the user pref **`huddles_set_status`** (the older manager
  checks it once when the call starts); off → no write. Errors are logged and the timer goes on.
- OBSERVED (code): leaving (`ENDING`, or an error) only cancels the timer: no profile write on leave. (The leave
  itself also sends `rooms.leave … reason=user_initiated`, not part of this.)
- OBSERVED (live): `huddles_set_status` is a boolean in `client.userBoot` `prefs` and in `users.prefs.get` `prefs`.
- OBSERVED (live, `users.info` of B read by A every 30 s, shapes only): B joined a channel huddle with
  `ctl huddle-media` (no renewal). Within seconds `huddle_state` = `in_a_huddle`, `huddle_state_expiration_ts` about
  22 minutes ahead (fixed, counting down), `huddle_state_call_id` a string, `huddle_state_channel_id` null (a
  channel huddle). Within ~40 s of B leaving: `default_unset`, expiration 0, no call id. So the **server sets the
  state on join and clears it on leave**; the client's renewal only moves the expiry (to 5 minutes ahead), and
  without it someone in a long huddle drops out of "In a huddle" after about 22 minutes.
- OBSERVED (live, klack's renewal): `users.profile.set` with that form answers ok; while the server's own expiry is
  later than now+300 the expiry read back stays the server's (unchanged). Not seen: what a renewal does once the
  server's expiry comes within 5 minutes (the test huddles on the test workspace ended after ~10 minutes, before
  that point). Slack's client sends it regardless, and so does klack.
- klack: the same renewal while a call is connected and not leaving, with the pref from boot / `pref_change`
  (off until Slack says on); `Client::set_huddle_state` refuses any field but `huddle_state*`. A fractional
  `huddle_state_expiration_ts` (what a renewal sends) is read as whole seconds, so it cannot break a profile.

# slacktui — design

A native Slack client for the terminal, written in Rust. It is Linux-first, and macOS
and Windows stay buildable. It talks to the same endpoints as Slack's web/desktop
client, using a browser-session token (`xoxc-…`) plus the `d` cookie (see
`../notes/`).

## Crates

| Crate | Role |
|---|---|
| `crates/slack-core` | Session/auth import, Web API client, RTM websocket, data model, mrkdwn/rich_text parsing. No terminal code. |
| `crates/huddle` | Huddles: Chime SDK signaling (prost subset of `SignalingProtocol.proto`), WebRTC audio (webrtc-rs, Opus), audio devices (cpal: microphone, speaker, resampling, levels, volume) and microphone processing (gain, speaking gate, RNNoise noise suppression via `nnnoiseless`); video (`video.rs`: H.264 via `openh264` built from source, RTP access-unit assembly, a synthetic test pattern), cameras (`camera.rs`: the `ffmpeg` tool, a camera only by explicit name) and screen sharing (`screen.rs`: the screen-cast portal via `ashpd` + GStreamer `pipewiresrc`, Linux only); video in/out shares one peer connection (camera section at mid 1, receive-only sections after it, renegotiated with a new SUBSCRIBE). |
| `crates/slacktui` | Binary: TUI (ratatui + crossterm), media (images, video and audio), notifications and sounds, huddle UI (call bar; `huddle_view.rs`: tiles, stage, scrolling, select/maximize; `call.rs`: devices, camera, screen share), preferences, keymap, themes, i18n, and the `ctl` scripting CLI used for end-to-end tests. |

## Local files

All next to the profiles in the config directory (`~/.config/slacktui/` on Linux):
`<profile>/session.json` (0600), `keys.toml` (key bindings, `keymap.rs`), `settings.toml` (local
preferences, `settings.rs`), `themes/<name>.toml` (`theme.rs`), `locales/<code>.toml` (`i18n.rs`).
Slack-side preferences (notification levels, keywords, VIPs, sections, stars, status) stay on Slack;
the reverse-engineered endpoints are documented in `../notes/` (sections, activity, notifications, huddles).

## Portability rules

- **Terminal:** crossterm, which works on Linux, macOS and Windows. Graphics go through `ratatui-image`: kitty/sixel/iTerm2 protocols, with a half-block fallback.
- **Audio:** cpal (ALSA/PipeWire, CoreAudio, WASAPI). **Video decoding:** the `ffmpeg`/`ffprobe` command-line tools on PATH (decoded frames and PCM come back over pipes), so nothing links against the ffmpeg libraries and every platform only needs the ffmpeg binaries installed.
- **Syntax highlighting:** syntect with its pure-Rust regex engine (`default-fancy`), so there is no C dependency (no Oniguruma).
- **Notifications:** notify-rust (D-Bus, macOS NSUserNotification, Windows toast).
- **Paths:** `directories`. Session files are created with owner-only permissions (0600 on unix).
- **Optional dependencies:** importing a session from RSlack needs SQLite (bundled). No other code may depend on Linux-only APIs unless it is behind `cfg`.

## Auth

`slacktui login` creates a session in one of two ways:
1. `--import-rslack [PROFILE]`: read the `d` cookie from RSlack's WebKit cookie DB, then fetch the `xoxc` token from a server-rendered workspace page (`/customize/emoji` embeds `"api_token"`).
2. `--workspace URL` with the browser's `d` cookie (`--cookie`, `$SLACKTUI_COOKIE_D` or a prompt): the same token fetch, then `auth.test`. This is the way in on macOS and Windows.

(The desktop magic-link flow, `auth.loginMagicBulk`, was planned but is not implemented.)

Sessions are stored at `~/.config/slacktui/<profile>/session.json` (0600). Tokens are never logged.

## Data flow

- **Boot:** `client.userBoot` provides channels, IMs and prefs. `users.list` and `conversations.info` fill in the rest.
- **Live updates:** `rtm.connect` opens a websocket that delivers messages, reactions, typing, read markers and presence. Pings every 30 s; reconnects with backoff.
- **History:** `conversations.history` / `conversations.replies`, paged on demand.
- **Sending:** `chat.postMessage` takes the composer's mrkdwn as `text` (Slack renders it), plus `thread_ts` for replies. Edits and deletes use `chat.update` / `chat.delete`; reactions use `reactions.add/remove`.
- **Files:** uploads use `files.getUploadURLExternal`, then a PUT, then `files.completeUploadExternal`. Downloads fetch `url_private` with the session cookie and token.

## Milestones (tracked in PROGRESS.md)

1. Core: session import, API client, RTM, `ctl whoami/post/history`.
2. TUI shell: sidebar (channels, DMs, unread markers), message pane, composer, keyboard and mouse navigation.
3. Messaging: rendering of mrkdwn/rich_text/blocks/attachments, edit/delete/react, threads pane, @user, #channel and :emoji: autocomplete, formatting shortcuts.
4. Notifications: mentions, DMs and keywords, honouring DND and muted channels.
5. Media: inline images (kitty graphics), image viewer, file upload/download, video playback with audio.
6. Huddles: reverse engineer the web client's huddle signalling via RSlack, then implement audio huddles (join/leave/mute) in Rust.
7. Extras: search, channel info/members, pins, typing indicators, presence, status, emoji list, unread jump.
8. Docs, packaging and portability review.

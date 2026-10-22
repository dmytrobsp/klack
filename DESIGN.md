# ttyak — design

A native Slack client for the terminal, written in Rust. It is Linux-first, and macOS
and Windows stay buildable. It talks to the same endpoints as Slack's web/desktop
client, using a browser-session token (`xoxc-…`) plus the `d` cookie (see
`docs/protocol/`).

## Crates

| Crate | Role |
|---|---|
| `crates/slack-core` | Session/auth import, Web API client, RTM websocket, data model, mrkdwn/rich_text parsing. No terminal code. |
| `crates/huddle` | Huddles: Chime SDK signaling (prost subset of `SignalingProtocol.proto`), WebRTC audio (webrtc-rs, Opus), audio devices (cpal: microphone, speaker, resampling, levels, volume) and microphone processing (echo cancellation — WebRTC's AEC3 via the pure-Rust `sonora`, fed with the meeting audio as it is played — gain, speaking gate, RNNoise noise suppression via `nnnoiseless`); video (`video.rs`: H.264 via `openh264` built from source, RTP access-unit assembly, a synthetic test pattern), cameras (`camera.rs`: the `ffmpeg` tool, a camera only by explicit name) and screen sharing (`screen.rs`: in a Wayland session the screen-cast portal via `ashpd` + GStreamer `pipewiresrc`; elsewhere the whole screen through `ffmpeg` — `x11grab` (tried against Xvfb), AVFoundation and `gdigrab` (untested)); video in/out shares one peer connection (camera section at mid 1, receive-only sections after it, renegotiated with a new SUBSCRIBE). |
| `crates/ttyak` | Binary: TUI (ratatui + crossterm), media (images, video and audio), notifications and sounds, huddle UI (call bar; `huddle_view.rs`: tiles, stage, scrolling, select/maximize; `call.rs`: devices, camera, screen share), preferences, keymap, themes, i18n, and the `ctl` scripting CLI used for end-to-end tests. |

## Local files

All next to the profiles in the config directory (`~/.config/ttyak/` on Linux):
`<profile>/session.json` (0600), `keys.toml` (key bindings, `keymap.rs`), `settings.toml` (local
preferences, `settings.rs`), `themes/<name>.toml` (`theme.rs`), `locales/<code>.toml` (`i18n.rs`).
Slack-side preferences (notification levels, keywords, VIPs, sections, stars, status) stay on Slack;
the reverse-engineered endpoints are documented in `docs/protocol/` (sign-in, sections, activity, notifications, snippets, huddles).

## Portability rules

- **Terminal:** crossterm, which works on Linux, macOS and Windows. Graphics go through `ratatui-image`: kitty/sixel/iTerm2 protocols, with a half-block fallback.
- **Audio:** cpal (ALSA/PipeWire, CoreAudio, WASAPI). **Video decoding:** the `ffmpeg`/`ffprobe` command-line tools on PATH (decoded frames and PCM come back over pipes), so nothing links against the ffmpeg libraries and every platform only needs the ffmpeg binaries installed.
- **Syntax highlighting:** syntect with its pure-Rust regex engine (`default-fancy`), so there is no C dependency (no Oniguruma).
- **Notifications:** notify-rust (D-Bus, macOS NSUserNotification, Windows toast).
- **Paths:** `directories`. Session files are created with owner-only permissions (0600 on unix).
- **No Linux-only APIs** outside `cfg`-gated code.

## Auth

`ttyak auth login` gets the browser's `d` cookie — from a browser profile signed in to Slack, from the browser
profiles it watches while you sign in with a link it prints, from an emailed code that ttyak confirms itself
(`signin.confirmCode`; only sending the code needs a browser's captcha), or by hand (`ttyak auth login
--workspace URL` with `--cookie`, `$TTYAK_COOKIE_D` or a prompt); with it the `xoxc` token is fetched from a server-rendered workspace page
(`/customize/emoji` embeds `"api_token"`), then checked with `auth.test` (`docs/protocol/auth.md`).

(The desktop magic-link flow, `auth.loginMagicBulk`, was planned but is not implemented.)

Sessions are stored at `~/.config/ttyak/<profile>/session.json` (0600). Tokens are never logged.

## Data flow

- **Boot:** `client.userBoot` provides channels, IMs and prefs. `users.list` and `conversations.info` fill in the rest.
- **Live updates:** a websocket that delivers messages, reactions, typing, read markers, presence and profile changes: Slack's own gateway as the web client opens it (`client.getWebSocketURL` + `flannel=3`, the token in the address, which is checked to be Slack's own `wss` host and never logged), else `rtm.connect`'s (a legacy connection: no profile changes of other people). Pings every 20 s; reconnects with backoff.
- **History:** `conversations.history` / `conversations.replies`, paged on demand.
- **Sending:** `chat.postMessage` takes the composer's mrkdwn as `text` (Slack renders it), plus `thread_ts` for replies. Edits and deletes use `chat.update` / `chat.delete`; reactions use `reactions.add/remove`.
- **Files:** uploads use `files.getUploadURLExternal`, then a PUT, then `files.completeUploadExternal`. Downloads fetch `url_private` with the session cookie and token.

## Milestones (tracked in PROGRESS.md)

1. Core: session import, API client, RTM, `ctl whoami/post/history`.
2. TUI shell: sidebar (channels, DMs, unread markers), message pane, composer, keyboard and mouse navigation.
3. Messaging: rendering of mrkdwn/rich_text/blocks/attachments, edit/delete/react, threads pane, @user, #channel and :emoji: autocomplete, formatting shortcuts.
4. Notifications: mentions, DMs and keywords, honouring DND and muted channels.
5. Media: inline images (kitty graphics), image viewer, file upload/download, video playback with audio.
6. Huddles: reverse engineer the web client's huddle signalling, then implement audio huddles (join/leave/mute) in Rust.
7. Extras: search, channel info/members, pins, typing indicators, presence, status, emoji list, unread jump.
8. Docs, packaging and portability review.

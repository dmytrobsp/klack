# slacktui

A Slack client for the terminal, written in Rust (ratatui + crossterm). Messaging with
threads, reactions, autocomplete and formatting; inline images, an image viewer and video with
sound; desktop notifications; search; and **huddles with audio**.

It talks to Slack the way the web client does: a browser session (`xoxc-…` token plus the `d`
cookie). It is not an official Slack app.

## Requirements

- A recent stable Rust toolchain (edition 2024).
- **libopus** (huddles): `apt install libopus-dev` · `brew install opus` · vcpkg on Windows.
- **ALSA headers** on Linux (audio): `apt install libasound2-dev`.
- **ffmpeg / ffprobe on PATH** at runtime, for video playback only.
- A terminal with graphics for real images (kitty, WezTerm, Ghostty, iTerm2, or any sixel
  terminal); others get half-block images.

```sh
cargo build --release
./target/release/slacktui login --workspace acme.slack.com   # prompts for the `d` cookie
./target/release/slacktui
```

## Signing in

- **From a browser:** sign in to Slack in your browser, copy the value of the `d` cookie for
  `slack.com` (devtools → Application → Cookies), then
  `slacktui login --workspace acme.slack.com` and paste it at the prompt (or set
  `SLACKTUI_COOKIE_D`). slacktui fetches the token itself and checks it with `auth.test`.
- **From RSlack** (the companion Linux desktop shell): `slacktui login --import-rslack default`.

Sessions are saved per profile in `~/.config/slacktui/<profile>/session.json` (owner-only
permissions). Use `--profile NAME` to keep several accounts apart. Signing in to more workspaces
with the same profile adds them to it: inside the app, `Alt+1 … Alt+9` switches between them
and `/workspace [name]` lists them or switches by name.

## Keys

Press **F1** (or `?`) in the app for the full list. The essentials:

| Key | |
|---|---|
| Tab / Shift+Tab | move between sidebar, messages, composer, thread, reply box |
| Alt+↑ / Alt+↓, Ctrl+K | previous / next conversation, jump to any conversation or person (`#` lists every channel, including ones you are not in) |
| Ctrl+F | search messages |
| F5 | activity: mentions, reactions to your messages, thread replies |
| `/status [:emoji:] [text] [for 30m\|2h\|today]` · `/status clear` | set or clear your status |
| `/away` · `/active` · `/dnd 30m` · `/dnd off` | appear away / active · pause or resume notifications |
| `*` · `m` (sidebar) | star / unstar · move to a section (or a new one); `/section rename <name>` · `/section delete` |
| F2 | conversation details: about, members, pins |
| F3 / F4 | join or leave the huddle / mute or unmute |
| Enter (on a message) | open its thread |
| r · e · d · y · Y · o · s · P | react · edit · delete · copy text · copy link · open image/video/snippet · save files · pin |
| u | profile card of the author (←/→: people the message mentions); Enter messages them, h starts a huddle |
| Enter · Alt+Enter | send · new line |
| @ # : | mention, channel, emoji autocomplete |
| `/upload <path> [text]` | upload a file |
| `/join [#channel]` · `/leave` | join (default: the channel you are previewing) · leave the current channel |
| `/create <name>` · `/create-private <name>` | create a channel |
| `/dm @a [@b …]` | open a DM, or a group DM with up to 8 people |
| `/snippet [lang] [title]` + lines | post the lines after the command (Alt+Enter) as a text snippet |
| Ctrl+Q | quit |

### Rebinding keys

Put your own bindings in `~/.config/slacktui/keys.toml` (macOS: `~/Library/Application Support/io.github.rslack.slacktui/`,
Windows: `%APPDATA%\rslack\slacktui\config\`). Each table is a context and each entry replaces an action's
default keys; F1 lists every context and action name, and shows the keys in effect.

```toml
[global]
jump_to = ["ctrl+p", "ctrl+k"]
[messages]
react = "R"
pin = []            # unbound
```

The same can be done in the preferences (F6 → Keys): Enter, then press the new key (the
leader first for a chord); Delete unbinds, `r` restores the default. Keys are written like `ctrl+k`, `alt+up`, `shift+tab`, `G`, `f5`, `space`, `pageup`. Set
`leader = "ctrl+b"` (at the top) to use chords such as `"<leader> k"`; while a chord is
being typed the status bar shows it. Plain
characters cannot be bound where you type (global, composer, search fields). Problems are shown
in the status bar at startup, and the default stays in place for any entry that has one.

## Huddles

F3 joins (or starts) the huddle of the conversation you are in, and `H` on a message starts (or
joins) a huddle in that message's thread; you join **muted**, and the
microphone is only opened while you are unmuted (F4). The call bar shows who is there, who is
speaking and who is muted. Someone starting a huddle in a DM with you sends a desktop
notification.

Under the hood a huddle is an Amazon Chime SDK meeting: `rooms.join` hands out the meeting
credentials, `crates/huddle` speaks Chime's signaling protocol and runs WebRTC audio (webrtc-rs,
Opus). See `../notes/huddles.md` for how this was worked out.

Pick audio devices with `SLACKTUI_AUDIO_INPUT` / `SLACKTUI_AUDIO_OUTPUT` (part of a device id or
name from `slacktui ctl audio-devices`). A device that is named but missing is an error; there is
no silent fallback.

No video, screen sharing, echo cancellation or noise suppression yet.

## Other settings

| Variable | |
|---|---|
| (F6 or `/prefs`) | preferences: compact list, thumbnails, profile pictures, emoji as names, 12/24 h clock, date format, desktop notifications, microphone and speaker — saved in `settings.toml` next to `keys.toml` |
| `SLACKTUI_LOG` | log filter, e.g. `slacktui=debug` (log file: `~/.cache/slacktui/<profile>/slacktui.log`) |
| `SLACKTUI_DOWNLOAD_DIR` | where `s` saves attachments (default: your downloads folder) |
| `SLACKTUI_AVATARS=0` | no profile pictures next to names (they are also off on terminals without real graphics) |

### Translations

The interface can be translated: `slacktui ctl locale-template > ~/.config/slacktui/locales/de.toml`,
fill in the right-hand sides, then choose `de` in F6 → Language & region. Only the help, the
preferences, overlay titles and the status-bar hints go through translation so far.

## Scripting

`slacktui ctl …` exposes the API for automation and tests: `whoami`, `channels`,
`history`, `replies`, `post`, `react`, `pin`, `upload`, `search`, `listen`, `audio-devices`,
`huddle-media` and a few research probes (`api`, `get`, `huddle-*`) that redact secrets.

## Layout

- `crates/slack-core` — session/auth, Web API client, RTM websocket, message formatting.
- `crates/huddle` — Chime signaling (protobuf), WebRTC audio, Opus, audio devices.
- `crates/slacktui` — the TUI and the `ctl` CLI.

Status and verification notes: `PROGRESS.md`. Design: `DESIGN.md`.

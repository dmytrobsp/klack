# klack

A Slack client for the terminal, written in Rust (ratatui + crossterm). Messaging with
threads, reactions, autocomplete and formatting; inline images, an image viewer and video with
sound; desktop notifications; search; and **huddles with audio**.

klack was called slacktui until October 2026; on first start it moves that name's folders
(`~/.config/slacktui`, `~/.cache/slacktui`, …) to klack's.

It talks to Slack the way the web client does: a browser session (`xoxc-…` token plus the `d`
cookie). It is not an official Slack app.

## Install

Ready-made builds are on the [releases page](https://github.com/dmytrobsp/klack/releases): unpack
the archive and put `klack` (`klack.exe`) somewhere on your PATH. None of them is signed.

| Download | Runs on | Needs |
|---|---|---|
| `x86_64-linux.tar.gz` | glibc 2.34 or newer (Ubuntu 22.04, Debian 12, Fedora 35, RHEL 9 and later) | `libasound.so.2` (ALSA), `libstdc++` — on every desktop Linux |
| `aarch64-macos.tar.gz` | Apple Silicon Macs, macOS 11 or newer | nothing (system frameworks only); unsigned: `xattr -d com.apple.quarantine klack` once, or allow it in System Settings → Privacy & Security |
| `x86_64-macos.tar.gz` | Intel Macs, macOS 11 or newer | as above |
| `x86_64-windows.zip` | 64-bit Windows 10 / 11 | nothing (Windows system DLLs only, no MinGW runtime); unsigned, so SmartScreen may ask once |

Optional everywhere: `ffmpeg` / `ffprobe` on PATH for video, cameras and screen sharing; a terminal
with graphics for real pictures (see below). To build it yourself instead, read on.

## Requirements

To build: a recent stable Rust toolchain (edition 2024; [rustup](https://rustup.rs)), a C/C++
compiler, `pkg-config`, **cmake** (the bundled Opus codec is built with it) and the **ALSA headers**
on Linux. On a fresh system that is (each line checked with a clean checkout in a container,
`tools/container-build.sh`):

| System | Packages |
|---|---|
| Debian 13 / Ubuntu 24.04 | `apt install build-essential pkg-config cmake libasound2-dev git curl` |
| Fedora | `dnf install gcc gcc-c++ make pkgconf-pkg-config cmake alsa-lib-devel git` |
| Arch | `pacman -S base-devel cmake alsa-lib git` |
| macOS | Xcode command-line tools, then `brew install cmake` |

To run: `libasound.so.2` (ALSA's library, on every desktop Linux) and the C++ runtime. Optional:
**ffmpeg / ffprobe on PATH** for video playback, cameras and (outside Wayland) screen sharing; a
D-Bus notification service for desktop notifications; a terminal with graphics for real images
(kitty, WezTerm, Ghostty, iTerm2, foot or any sixel terminal; others get half-block images). Without a
sound card huddles still work (silently, with a note in the call bar); without system CA
certificates klack uses the Mozilla roots it carries.

```sh
cargo build --release
./target/release/klack login --workspace acme.slack.com   # prompts for the `d` cookie
./target/release/klack
```

Windows from Linux: `rustup target add x86_64-pc-windows-gnu`, `apt install mingw-w64`, then
`cargo build --release --target x86_64-pc-windows-gnu`. The result is one `klack.exe` that needs no
MinGW DLLs (`crates/klack/build.rs` links OpenH264's C++ runtime statically, `.cargo/config.toml` the C
runtime).

## The screen

The sidebar on the left (Threads, sections, starred conversations, channels, direct messages — the most
recently active first; channels and DMs without a message for 30 days are left out unless starred, unread
or open, F6 → Appearance; Ctrl+K still finds them and shows them while open; what you have not read is
bold with a bar in the first column, muted conversations are dimmed and not counted), the
messages in the middle with a reply pane for threads on the right (in a narrow terminal the thread
takes the messages' place until you close it), the composer below, and a status
bar at the bottom: a pill saying where your keys go now (SIDEBAR, MESSAGES, COMPOSE, THREAD, REPLY,
HUDDLE, or the open dialog: JUMP, SEARCH, PREFS, …), where you are (`#social › Thread`), unread
counts and messages — or, while a dialog is open, its keys — then F1 for all keys and whether klack
is connected. A thumb on a pane's right border shows where you are when it scrolls. A conversation
opened with unread messages has a red "new" line above the first of them; while you read further
up, the bottom border counts the newer messages below ("↓ 3 newer"). With two or more
workspaces in the profile a rail on the far left has a tile for each — its logo where it has
one and the terminal draws pictures, else its initials and number in a colour of its own — with
the shown one marked and, below the others, what is waiting there (`@3`, or `•` for anything
unread); a click on a tile switches, like Alt+1…9.

## Signing in

- **In a browser window (the easy way):** `klack login --workspace acme.slack.com` opens Chrome,
  Chromium, Edge or Brave (an installed one or a flatpak; `KLACK_BROWSER` picks another) on the
  workspace's sign-in page, with a temporary profile of its own. Sign in there however Slack offers
  (password, Google / SSO, 2FA, an emailed code); klack notices, takes the session, closes the window
  and deletes that profile. Your own browser profile is never read or changed. Ctrl+C cancels.
- **From the browser you already use:** `klack login --workspace acme.slack.com --from-browser
  firefox` (or `chrome`) takes the session from a browser you are signed in to Slack with. Its cookie
  database is copied and read, never changed; Chrome's (and Chromium's, Edge's, Brave's) cookies are
  decrypted with its key from the system keyring (Linux) or the Keychain (macOS, which asks once).
  `--browser-profile DIR` picks one profile (or a browser's folder of profiles); otherwise every profile
  is tried. When several sign in to the workspace as different people, klack lists them (the browser's
  profile name and the account) and asks which; without a terminal to ask in it takes the one used
  most recently. Chrome
  on Windows cannot be read this way (app-bound encryption); Firefox can.
- **By hand:** with `--no-browser`, or when no such browser is found, klack asks for the value of
  the `d` cookie for `slack.com` (sign in in any browser, then devtools → Application → Cookies);
  `KLACK_COOKIE_D` or `--cookie` give it directly. Either way klack fetches the token itself and
  checks it with `auth.test`. (There is no sign-in by emailed code inside the terminal: Slack sends
  the code only after a captcha, which needs a browser — the window above does it.)

Sessions are saved per profile in `~/.config/klack/<profile>/session.json` (owner-only
permissions). Use `--profile NAME` to keep several accounts apart. Profiles separate accounts only:
the preferences (`settings.toml`, everything in F6), `keys.toml`, `themes/` and `locales/` are shared
by all profiles; each profile has its own sessions and its own log/cache folder. To tell workspaces
apart, give each a theme of its own (F6 → Appearance → Theme for this workspace). Signing in to more workspaces
with the same profile adds them to it: inside the app, `Alt+1 … Alt+9` switches between them
and `/workspace [name]` lists them or switches by name. The status bar notes unread messages in the
others ("Alt+2 @3": three unread DMs or mentions there; "Alt+2 •": something unread), checked every 30 s,
and their desktop notifications still arrive (titled with the workspace); clicking one switches there.

## Keys

Press **F1** (or `?`) in the app for the full list. The essentials:

What you type is kept per conversation and per thread: leave a half-written message (or an
edit in progress) and it is back when you return, until you send it; the sidebar marks such
conversations with ✎. Drafts also survive quitting, switching workspace and crashes (they are saved two seconds after you stop typing): they are kept per
workspace account in `drafts/<team>-<user>.json` under the local data folder
(`~/.local/share/klack` on Linux), readable by you only, and the file goes once no draft is left.

| Key | |
|---|---|
| Tab / Shift+Tab | move between sidebar, messages, composer, thread, reply box |
| Alt+↑ / Alt+↓, Ctrl+K | previous / next conversation, jump to any conversation or person — unread first, then the most recently active (`#` lists every channel, including ones you are not in) |
| Ctrl+F | search messages (more results load as you move down the list) |
| mouse | click to select, wheel to scroll (also the help and the preferences; in lists like Ctrl+K, Ctrl+T, F5 and search it moves the selection), hover shading, right-click for a context menu (messages, sidebar entries); a thumb on a pane's right border shows where you are when it scrolls |
| F5 | activity: mentions, reactions to your messages, thread replies |
| Ctrl+T | threads: every thread you are in, unread ones first (marked with a bar and "2 new"), each with its parent and its first unread (or newest) reply; Enter opens the thread at that reply, `r` loads again, more load at the end of the list. The **Threads** row atop the sidebar opens it too: it turns bold with a bar and the number of unread replies when threads you follow get new ones |
| `/status [:emoji:] [text] [for 30m\|2h\|today]` · `/status clear` | set or clear your status |
| `/away` · `/active` · `/dnd 30m` · `/dnd off` | appear away / active · pause or resume notifications |
| any other `/command` | run by Slack, as its own clients do (`/remind`, `/giphy`, `/shrug`, apps' commands); an answer only you see is marked "only visible to you"; an unknown one says so; a leading space sends the line as text |
| `*` · `m` (sidebar) | star / unstar · move to a section (or a new one); `/section rename <name>` · `/section delete` (each section's name heads a rule, with a blank row between sections — none in compact mode) |
| `Space` / `z` (sidebar), click a header | fold / unfold the section: folded, only "▸ name" shows, with the number of its conversations with unread messages and a badge for mentions; Enter on it unfolds too, and going to a conversation inside (Ctrl+K, search, a notification) unfolds it. Kept per workspace account in `sidebar/<team>-<user>.json` under the local data folder |
| F2 | conversation details: about, members, pins |
| F3 / F4 / F7 / F8 / F9 / F10 | join or leave the huddle / mute or unmute / open the huddle's thread / huddle view / camera on or off / share your screen or stop |
| F6 | preferences (also `/prefs`): availability, appearance, messages, region, notifications, audio, keys |
| Enter (on a message) | open its thread |
| m · Menu key · Shift+F10 (on a message) | its context menu, as a right-click opens it |
| r · e · d · y · Y · o · s · P | react · edit · delete · copy text · copy link · open what the message holds — a link in your browser, a picture, a video (Space pauses, ← / → jump 10 s), a snippet; with several, a list to pick from · save files (a dialog: Space / `a` pick which, Tab changes the folder for this save, Enter saves) · pin |
| u | profile card of the author (←/→: people the message mentions); Enter messages them, h starts a huddle |
| Enter · Alt+Enter | send · new line |
| Ctrl+V | paste from the clipboard: text is typed in; a picture (a screenshot) or files copied in a file manager are attached above the box (✕, or Backspace in an empty box, removes one) and Enter sends them with your text — or on their own — as one message. Your terminal's own paste (Ctrl+Shift+V) only carries text, and over SSH there is no clipboard to read |
| `/` (first in the box) · @ # : | command (klack's own and Slack's, with what they take; a command typed out in full runs on Enter) and its fixed choices (`/dnd 30m`…`off`, `/status clear` and `… for today`, `/section rename\|delete`, `/workspace` names, `/snippet` languages), mention, channel, emoji autocomplete |
| `/upload <path> [text]` | upload a file (the path completes as you type: Tab goes into a folder; `~` is your home; names with spaces are quoted for you) |
| `/join [#channel]` · `/leave` | join (default: the channel you are previewing — Ctrl+K finds channels you are not in; a previewed one is fetched again every 15 s, F6 → Messages & media) · leave the current channel |
| `/create <name>` · `/create-private <name>` | create a channel |
| `/dm @a [@b …]` | open a DM, or a group DM with up to 8 people |
| `/snippet [lang] [title]` + lines | post the lines after the command (Alt+Enter) as a text snippet |
| Ctrl+Q | quit |

### Rebinding keys

Put your own bindings in `~/.config/klack/keys.toml` (macOS: `~/Library/Application Support/klack/`,
Windows: `%APPDATA%\klack\config\`). Each table is a context and each entry replaces an action's
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
microphone is only opened while you are unmuted (F4). The call bar at the bottom shows that you are
still in the huddle — where, how many are there, your microphone (🎙 live with your level, or 🔇
muted), your camera and a screen share — and the picture and name of whoever is talking, or talked
last, instead of everyone; with its keys underneath, and it takes more rows rather than cutting
anything off when the window is narrow. F8 (or a click on the call bar) shows the huddle view in place of
the messages: a tile per participant with their picture or video — the border lights up while
they speak, 🔇 in the title when muted — that shrinks as people join and scrolls (mouse wheel,
"▲/▼ N more") when they no longer fit; a screen share takes the stage with everyone else in a
strip. Arrows select a tile, Enter (or a second click) maximizes it, Esc shows all tiles again
and then leaves the tiles; F8 again, or opening a conversation, brings the messages back. Someone starting a huddle in a DM with you sends a
desktop notification.

Clicking a desktop notification (Linux desktops with an XDG notification server) opens its
conversation in klack — a thread reply opens the thread with the reply selected — for 30
minutes after it was shown. The terminal window is not raised (a terminal program cannot do
that portably).

Under the hood a huddle is an Amazon Chime SDK meeting: `rooms.join` hands out the meeting
credentials, `crates/huddle` speaks Chime's signaling protocol and runs WebRTC audio (webrtc-rs,
Opus). See `docs/protocol/huddles.md` for how this was worked out.

Pick audio devices with `KLACK_AUDIO_INPUT` / `KLACK_AUDIO_OUTPUT` (part of a device id or
name from `klack ctl audio-devices`). A device that is named but missing is an error; there is
no silent fallback.

F6 → Huddles has the huddle sounds (at the notification sound volume; each can be turned off): a soft
rising arpeggio when you join, and while you are in a huddle a short "pop" when someone joins and a
falling pair when someone leaves. A huddle invitation (someone starts one in a DM with you) rings — a
soft trill every few seconds, with "📞 X invites you to a huddle" in the status bar — until you join (F3,
from any conversation), decline (Shift+F3), the huddle ends, or it has rung for the set time (30 s); then,
if "Then decline it on Slack" is on, it is declined for you. When you have been the only one in a
huddle for a while (300 s by default, adjustable), a quiet melody loops until someone joins or you leave.

F6 → Audio also has the huddle volume, the notification sound volume, a microphone gain, a
speaking threshold (quieter microphone input is sent as silence, held open 300 ms after you
stop so words are not cut), noise suppression (RNNoise via `nnnoiseless`) and echo cancellation
(on by default: WebRTC's AEC3 via the pure-Rust `sonora`, so others do not hear themselves from your
speakers; it drops the microphone above 16 kHz, which speech does not need). `+`/`-` (or Enter)
step the levels, and a huddle you are in follows at once.

Other people's cameras play in their tiles in the huddle view (F8); a screen share gets its own
"🖥 <name>'s screen" tile ("🖥 <name> is sharing" in the call bar).

Your camera: choose it in F6 → Audio & video → Camera (until you do, it can never switch on), then
F9 in a huddle switches it on and off; you see yourself in your tile. Capture goes through the
`ffmpeg` tool (V4L2 on Linux; AVFoundation/DirectShow on macOS/Windows, where cameras are listed
with ffmpeg's `-list_devices`, are written but have never been run).

Video is H.264 (OpenH264, built from source — note that H.264 is patent-encumbered when you
distribute binaries). For tests, `klack ctl huddle-media <channel> --video-test` joins with a
synthetic camera (a moving test pattern), `--share-test` shares it as a screen, `--camera NAME`
sends a real camera (`ctl cameras` lists them), and `--record-video DIR` saves what arrives.

Your screen: F10 in a huddle starts sharing, F10 again stops. Others see it as your screen share;
you see it in a "🖥 Your screen" tile. In a Wayland session the desktop asks what to share (a
screen or a window, through the screen-cast portal, read with GStreamer's `pipewiresrc` —
`gst-launch-1.0` and the GStreamer PipeWire plugin must be installed). Elsewhere there is no picker
and F10 shares the **whole** screen through `ffmpeg`: `x11grab` in an X11 session, the main screen
via AVFoundation on macOS (the terminal needs the Screen Recording permission), the desktop via
`gdigrab` on Windows. The Wayland portal and X11 (against a virtual Xvfb display) have been tried;
on Windows only from an SSH session, where `gdigrab` cannot capture (no desktop); macOS is untested.

## Other settings

| Variable | |
|---|---|
| (F6 or `/prefs`) | preferences: theme, compact list, thumbnails, profile pictures, emoji as names, custom emoji as pictures, 12/24 h clock, date format, time zone, language, desktop notifications and sound, microphone and speaker, volumes, microphone gain, speaking threshold, noise suppression, echo cancellation — saved in `settings.toml` next to `keys.toml` |
| `KLACK_LOG` | log filter, e.g. `klack=debug` (log file: `~/.cache/klack/<profile>/klack.log`) |
| `KLACK_DOWNLOAD_DIR` | where `s` saves attachments first, over F6 → Messages & media → Save files to (default: your downloads folder) |
| `KLACK_OPEN_WITH` | the program `o` opens links with, given the address (default: the system's opener — `xdg-open`, `open`, Windows' URL handler); only web and mail links are opened |
| `KLACK_AVATARS=0` | initials instead of profile pictures next to names (as on terminals without real graphics; F6 → Messages & media → Show profile pictures turns both off, Two-row profile pictures makes them bigger) |
| `KLACK_GRAPHICS` | how pictures are drawn: `kitty`, `sixel`, `iterm2` or `halfblocks` (default: asked from the terminal; also F6 → Messages & media → Draw pictures with) |

### Themes

F6 → Appearance → Colour theme opens a picker of dark, light, terminal and your own themes,
each with a few of its colours: type to filter, ↑/↓ show each one on the whole screen at once,
Enter keeps it, Esc goes back (a theme file that does not load says why). A theme is
a TOML file in `themes/` next to `settings.toml`: `base = "dark"` plus any colours to change
(`"#ff8800"`, ANSI names such as `"light-red"`, or 0–255). Start from
`klack ctl theme-template dark > sunset.toml`, edit it, and install it (checked first) with
`klack ctl theme-import sunset.toml`; share themes by passing the file around.

To tell workspaces apart at a glance, F6 → Appearance → Theme for this workspace gives the
workspace you are in a theme of its own (the same picker, with "(colour theme)" for none);
`settings.toml` keeps it under `[appearance.workspace_themes]` by workspace id.

### Translations

The interface can be translated: `klack ctl locale-template > ~/.config/klack/locales/de.toml`,
fill in the right-hand sides, then choose `de` in F6 → Language & region. Only the help, the
preferences, overlay titles and the status-bar hints go through translation so far.

## Scripting

`klack ctl …` exposes the API for automation and tests: `whoami`, `channels`,
`history`, `replies`, `post`, `react`, `pin`, `upload`, `search`, `listen`, `audio-devices`,
`huddle-media` and a few research probes (`api`, `get`, `huddle-*`) that redact secrets.

## Layout

- `crates/slack-core` — session/auth, Web API client, RTM websocket, message formatting.
- `crates/huddle` — Chime signaling (protobuf), WebRTC audio, Opus, audio devices.
- `crates/klack` — the TUI and the `ctl` CLI.

Status and verification notes: `PROGRESS.md`. Design: `DESIGN.md`.

## Licence

klack is licensed under either of the [Apache License, Version 2.0](LICENSE-APACHE) or the
[MIT licence](LICENSE-MIT), at your option. Unless you say otherwise, any contribution you submit
for inclusion in klack is licensed the same way, without any additional terms or conditions.

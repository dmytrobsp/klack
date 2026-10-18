# slacktui — progress

Canonical status. The plan and architecture are in `DESIGN.md`. The reverse-engineering
background is in `../REPORT.md` and `../notes/`.

**Verification levels**
- **LIVE**: exercised in a real kitty window against the `rslack-testbed` workspace and checked by
  screenshot and/or by reading back as the other account.
- **UNIT**: covered by `cargo test` only.
- **NONE**: written but not exercised.

## Status (branch `master`; every step is one commit)

| Area | State | Verified | Commit |
|---|---|---|---|
| slack-core: session import from RSlack, Web API client, RTM websocket, mrkdwn/rich_text parser | done | LIVE (both accounts; RTM events received) + UNIT (8) | 883df8d |
| `slacktui login --import-rslack`, `slacktui ctl whoami/channels/history/post/react/upload/listen` | done | LIVE | 21c50f0 |
| Terminal setup/teardown (panic-safe), event loop | done | LIVE | 28c8361, 65d2f49 |
| Sidebar: channels/DMs, unread and mention badges, keyboard and mouse | done | LIVE | 75fd9a1 |
| Message pane: history, grouping, formatting, emoji, files (as text), reactions, thread counts, day separators, older-page loading | done | LIVE (history/formatting); older-page loading NONE (not enough history in the testbed) | 0e88600 |
| Composer: multi-line, word motions, Alt+B/I/S/C formatting, paste, Enter send | done | LIVE (sent a two-line message, read back as B) | a4f4af2 |
| Live updates (RTM): messages, reactions, read markers, presence, typing indicator | done | LIVE (B's message and reaction appeared instantly; badge cleared) | 104b768 |
| Live updates: edits, deletes, thread replies, typing display | done | UNIT only — NOT seen live | 104b768 |
| Message selection (↑/↓, j/k, g/G, click), Tab focus cycle | done | LIVE | 9936455 |
| Reaction picker (`r`): search, favourites, custom emoji, optimistic toggle | done | LIVE (🚀 added, no double count from the RTM echo; 🚀 and 👍 later read back by B via `ctl history`) | 5ab8258 |
| Edit own messages: `e` on a selected message or ↑ in an empty composer; mentions/channels/@here back to composer mentions; Enter = `chat.update`, Esc cancels; optimistic with rollback | done | LIVE (edited a message with a mention, `&` and `<…>`; B's `ctl history` read back the right `<@U…>`/`&amp;`/`&lt;` text; `e` path and Esc cancel checked by screenshot) + UNIT (round trip) | d2cdb2f |
| Delete own messages: `d` → y/n popup → `chat.delete`; optimistic with rollback; refuses others' messages | done | LIVE (`n` keeps it, `y` removes it, B's history no longer has it; `d` on B's message shows a refusal) | e415bdf |
| Copy: `y` = message as plain text, `Y` = permalink (`chat.getPermalink`). arboard (with Wayland data-control), OSC 52 fallback; on quit the copy is handed to the terminal via OSC 52 so it survives | done | LIVE (`wl-paste` read back the text with `@Dmytro`/`&`/`<…>` resolved, and the `…/archives/D…/p…` link; after quit the clipboard still held the text, with the terminal left open) + UNIT (base64) | 64dffaa |
| Threads pane (view): Enter on a message opens its thread on the right (`conversations.replies`, loaded once, then kept live over RTM); parent, "N replies" rule, replies; ↑↓/jk/g/G selection, `r` react, `y`/`Y` copy, mouse click/scroll; Esc closes; closes on channel switch; Tab cycle includes it | done | LIVE (B's existing reply loaded; two replies from B appeared live; 👍 added from the thread showed on both copies of the parent; `y` copied a reply) + UNIT (thread layout, copies across timeline/thread) | fd103c4 |
| Threads pane (reply): own composer in the thread pane (`i` from the thread, Tab order sidebar → messages → composer → thread → reply); Enter posts with `thread_ts`; Alt+A toggles "also send to channel" (`reply_broadcast`); ↑ in the empty reply box / `e` / `d` work on own replies; composer keys shared via `Composer::handle_key` | done | LIVE (reply read back by B with `ctl replies`; reply edited from the thread; broadcast reply shown in both panes and in B's channel history; broadcast deleted from the thread pane, gone for B; reply count stayed exact throughout) + UNIT (key outcomes) | d0c1620 |
| `ctl replies <chan> <ts>`; `ctl history`/`replies` print reactions | done | LIVE | d0c1620 |
| Autocomplete in both composers: `@user` (display/handle/real name; prefix, then word, then substring) plus `@here/@channel/@everyone`, `#channel`, `:emoji:` (picker ranking, custom emoji); popup above the composer; ↑↓ move, Tab/Enter insert, Esc dismisses until the token changes; mentions are sent as `<@U…>`/`<#C…>`/`<!here>` | done | LIVE (sent `hi @Dmytro Dudnyk see #social it works :tada:` — B read back `<@U0C9YM9J3RR> … <#C0CA4NBM998> … :tada:`; `@dmytro.d` completed in the thread box and arrived as `<@U…>`; Esc dismissal checked) + UNIT (ranking, specials, channels, emoji) | 2d609d0 |
| Desktop notifications (notify-rust): DMs/group DMs, mentions incl. @here/@channel/@everyone, replies in threads you started or replied to; quiet for own/system messages, muted channels, DND (snooze + daily schedule, live `dnd_updated`), and the conversation/thread on screen while the terminal is focused (focus-change reporting) | done | LIVE (D-Bus `Notify` calls captured with `dbus-monitor`: DM elsewhere → "Dmytro"; #social without mention → none; with mention → "Dmytro in #social"; DM on screen + focused → none; same but unfocused → notified; reply in own thread → "Dmytro (thread)") + UNIT (decision rules, DND windows) | 56bda27 |
| Huddles, research + signaling: web client reverse engineered (`../notes/huddles.md`): huddles are Amazon Chime SDK meetings; `rooms.join channel_id=…` returns the Chime meeting/attendee (`free_willy`). New crate `crates/huddle`: hand-written prost subset of the Chime `SignalingProtocol.proto` (v3.32.0), credentials parsing (join token never printed), signaling WebSocket (`_aws_wt_session` subprotocol, 0x05-prefixed frames). `ctl huddle-join/huddle-leave/huddle-signal/api/get` for research (secrets redacted; `get` only sends the session to `*.slack.com`) | signaling done; media next | LIVE (`ctl huddle-signal social`: PING, JOIN_ACK with TURN credentials, INDEX with 1 participant, LEAVE → LEAVE_ACK against the real Chime service) + UNIT (frame encoding, credentials, JOIN fields) | see git log |
| Sign-in without RSlack: `slacktui login --workspace <url>` with the browser's `d` cookie (`--cookie`, `$SLACKTUI_COOKIE_D` or a prompt) → token fetch + `auth.test` | done | LIVE (throwaway profile signed in as A, session file 0600, `ctl whoami` OK, profile removed) | ee87331 |
| Packaging and portability: README (requirements, sign-in, keys, huddles, settings), release profile (thin LTO, stripped: 33 MB), zero compiler warnings. Portability reviewed by reading: the only Unix API (0600 session files) is `cfg(unix)`-guarded; audio/notifications/clipboard/paths use cross-platform crates; video needs ffmpeg on PATH; huddles need libopus | done | Release build runs (`ctl whoami`). **macOS/Windows builds NOT attempted** (no cross toolchains/SDKs here) | ee87331 |
| Huddle invites: a desktop notification when someone else starts a huddle in a DM / group DM ("X started a huddle · Press F3 to join"); not for channels, muted conversations, DND, or a huddle you are in | done | LIVE (B started a huddle in the DM → D-Bus `Notify` "Dmytro started a huddle" captured; DM showed 🎧1) + UNIT (rules, start vs join) | 8d57bb0 |
| Audio device selection: `SLACKTUI_AUDIO_INPUT` / `SLACKTUI_AUDIO_OUTPUT` match a device id or name (case-insensitive part); a named device that is missing is an error, never a fallback; `ctl audio-devices` lists `<id> — <name>` | done | LIVE (listing; `SLACKTUI_AUDIO_OUTPUT=nonexistent-device` → call bar "no sound: no output device matching…", no playback stream created) + UNIT (matching) | a708e3b |
| Huddles in the TUI: F3 joins/starts the current conversation's huddle (`rooms.join`) or leaves; joins **muted** (the microphone is opened only while unmuted); F4 mute/unmute (AUDIO_CONTROL); call bar with participants, speaking 🔊 and muted 🔇; the meeting mix plays on the default output (cpal, resampled); sidebar shows `🎧N` on conversations with an active huddle (live `sh_room_join/leave/update`) | done | LIVE (B started a huddle in #social with a 440 Hz tone → `🎧1` appeared in A's sidebar; F3 joined: call bar listed B speaking and A muted; slacktui's own PipeWire playback stream carried the tone, −29.8 dB at 440 Hz vs −77.5 dB at 2 kHz; B's roster showed A `muted: true`; F3 left and B's roster dropped A) + UNIT (resampler, mixdown, roster rules, huddle tracking). **Microphone capture NOT verified live** (not recording the user's room); the send path itself is verified with `ctl huddle-media --tone`. | 4be3870 |
| Huddles, media: `huddle::media` (webrtc-rs 0.21, TURN relay over UDP, Opus 48 kHz mono via libopus) and `huddle::session` (one task: JOIN → JOIN_ACK → SDP offer → SUBSCRIBE → SUBSCRIBE_ACK → audio; answers PINGs; roster from AUDIO_STREAM_ID_INFO; mute via AUDIO_CONTROL; LEAVE). `ctl huddle-media <chan> [secs] [--tone HZ] [--record FILE]`. rustls provider now chosen explicitly in `main` (ring), since webrtc compiles in a second provider | done | LIVE (A sent a 440 Hz sine, B recorded in the same huddle: −20 dB at 440 Hz vs −53 dB at 1 kHz, clean spectrogram; both saw each other in the roster; media "Connected" via TURN) + UNIT (Opus round trip, frames, TURN uri filter) | see git log |
| Presence: `presence_sub` for all people once connected and the user list is loaded (again after reconnects); sidebar DM dots and F2 member/profile presence follow it | done | LIVE (A shown active; B's dot turned green when B connected over RTM with `ctl listen`) | see git log |
| Pins: `P` pins/unpins the selected message (optimistic, `pins.add/remove`); "📌 pinned" marker; live `pin_added/pin_removed`; `ctl pin [--remove]`; `ctl history` shows `[pinned]` | done | LIVE (A pinned and unpinned "notify test 7" — B's history followed; B's `ctl pin` showed up live in A's TUI; F2 Pins listed both) + UNIT (pin events) | c0e60e3 |
| Conversation details (F2): About (type, topic, purpose, created, muted; for DMs the person's name, handle, title, status, time zone, presence), Members (Enter opens a DM), Pins (Enter jumps to the message); ←/→ tabs | done | LIVE (#social About and Members; Enter on B opened the DM; B's profile in the DM's About) + UNIT (tabs, pins parsing); Pins tab seen live with two pins | 1673151 |
| Search (Ctrl+F): `search.messages`, two-line results (who · where · when · "in thread", then text); Enter on a hit switches conversation, selects the message, and for replies opens the thread (parent from the permalink's `thread_ts`) with the reply selected; `ctl search` prints matches | done | LIVE (top-level hit in #social selected after the jump; reply hit opened the DM thread with the reply selected) + UNIT (hit parsing incl. thread replies) | 11b64c9 |
| Quick switcher (Ctrl+K): channels, DMs and group DMs ranked by name/handle (unread first on an empty query); people without a DM appear when searching and open one via `conversations.open` | done | LIVE (unread #social listed first, `soc` filtered to it, Enter jumped there and marked it read) + UNIT (ranking, new-DM entries). Opening a new DM not exercised live (both test accounts already have DMs). | d1c314e |
| Help overlay: F1 anywhere, `?` outside the composers; every key grouped by area; any key closes; status bar shortened to the essentials + "F1 help" | done | LIVE (screenshot of the overlay; closes on a key) | 4095abd |
| Video with sound: `o` on a message with a video downloads it (Slack's `mp4` transcode, else the original) to a temp file and plays it full-window; `ffmpeg` (runtime dependency on PATH) decodes 12 fps RGB frames and f32 PCM at the output device's rate over pipes; frames drawn with ratatui-image, audio via cpal on its own thread; Space/p pauses (stream corked), Esc/q stops (processes killed, temp file removed) | done | LIVE (B's 8 s testsrc clip: frames advance while playing and freeze while paused; slacktui's own PipeWire stream recorded with `pw-record` carries the 440 Hz tone, −40.6 dB in the 440 Hz band vs −90 dB at 2 kHz; stream corked on pause; ffmpeg gone, temp file deleted and stream removed after Esc) + UNIT (probe parsing, frame fitting, frame schedule) | e973856 |
| Files: `s` saves a message's attachments to the downloads folder (`$SLACKTUI_DOWNLOAD_DIR` overrides; never overwrites, ` (n)` suffix; remote names cannot escape the folder); `/upload <path> [message]` in either composer uploads a local file (quoted paths, `~`), into the thread from the reply box | done | LIVE (saved B's PNG: pixel-identical to the original, Slack strips PNG text chunks; uploaded a PNG with a message — B's history has it, it appeared live with its thumbnail) + UNIT (unique names, path traversal, command parsing) | f55d51a |
| Image viewer: `o` on a message with images opens a full-window view of the large version (`thumb_1024`, else the original), fitted without upscaling and re-encoded on resize; ←/→ or h/l between the message's images; Esc/q closes | done | LIVE (B's PNG shown complete and centred; thumbnail intact after closing) + UNIT (stepping, stale results ignored) | 0721cdd |
| Inline image thumbnails: terminal graphics detected at startup (`Picker::from_query_stdio`, half-block fallback); rows reserved from `original_w/h` (≤ 36×10 cells, no upscaling) so nothing jumps; thumbnails fetched lazily when on screen (`thumb_*` with the session cookie), decoded/encoded off the UI task, cached per file; drawn with `SlicedImage` so partly scrolled images are clipped | done | LIVE (kitty: B's 800×500 PNG shown under its file line; clipped correctly after scrolling; no remnants after switching conversation) + UNIT (sizing, reserved rows) | a113b72 |

Tests: `cargo test` → slack-core 8, huddle 11, slacktui 56, all passing. `cargo build` has no errors.
Remaining dead-code warnings: `Composer::cursor/is_multiline`, `Sidebar::select_id`, `Workspace::team_id`.

## Next (in order, each as a small, separately verified step)

The original roadmap is done. On 2026-10-18 the user asked for the missing features below, in this
order (agreed after an audit; run autonomously, one verified commit per step). Where a Slack endpoint is
not a documented public method, confirm it from the web client bundles first (see `../notes/huddles.md`
for how the bundles are fetched and searched).

Easy (standard API or a small client endpoint):
1. Channels: browse (incl. channels you are not in), join, leave, create (public/private) — `conversations.list/join/leave/create`; Ctrl+K should also find non-member channels.
2. Group chats: start an unnamed group DM with several people (`conversations.open users=a,b,c`); "named" group chats = private channels (item 1).
3. Your own messages clearly marked (e.g. a coloured gutter bar / distinct name style).
4. Profile cards for any member (from a message author, F2 Members, mentions): name, title, status, local time, presence, "message"/"huddle" actions.
5. Text snippets: show snippet files inline (fetch the text, syntax highlight), create one from the composer.
6. Favourites (starred channels) and channel sections (folders) in the sidebar — find the web client's endpoints (`users.channelSections.*` or similar) and read/write them.
7. Activity feed (mentions, reactions to your messages, thread replies) — find the endpoint (`activity.feed`?).
8. Availability: set status (emoji + text, expiry), away/active, pause notifications (DND snooze).
9. Starting a huddle in a thread — check in the client whether this exists as its own flow.

Medium:
10. Profile pictures in the message list (small kitty/sixel images, cached).
11. Multiple workspaces: switch inside the app (profile already stores several).
12. Preferences menu: availability, notifications (per-channel all/mentions/nothing, keywords), VIP, appearance (theme/colours, compact mode), messages & media (inline images/video on/off, emoji style), language & region (time zone, 12/24 h, date format; i18n scaffolding), audio & video (devices). Save to a config file; Slack-side prefs via `users.prefs.*`.

Hard (huddles video; the user's real camera/screen must not be captured in tests without them):
13. View other participants' webcam and screen share (receive video over WebRTC, decode, draw like the video player).
14. Send your own webcam, then screen share (camera capture, VP8/H.264 encode; Wayland needs the screen-cast portal + PipeWire).

## Known gaps / not verified

- Older-history paging, and the edit/delete/thread-reply RTM paths, have never been exercised live.
- The test workspace has very little history, so scrolling performance with large channels is unknown.
- Edit/delete rollback (`Net::Restore`) has not been exercised against a real API failure.
- The RTM `message_changed`/`message_deleted` paths for *other people's* edits are still UNIT only (there is no `ctl edit`/`ctl delete` yet to drive them from B).
- Thread drafts are not kept: closing the pane or switching channel clears the reply box.
- The reply box title clips "(Alt+A)" when the pane is narrow.
- Typing notifications are not sent from the reply box with `thread_ts` (they go to the channel).
- Status-bar messages (errors, refusals) stay until the next one replaces them; they are never cleared on a timer.
- The message being edited is not highlighted while the composer has focus.
- The OSC 52 copy path (no system clipboard, e.g. over SSH) has only been exercised as the quit hand-off, not as the primary path.
- The clipboard hand-off needs the terminal to outlive slacktui: with `kitty -e slacktui` the whole kitty instance exits and the copy is lost.
- The thread pane has no day separators, so a thread spanning several days shows only times.
- `@here/@channel/@everyone` are offered in DMs too.
- Video: no seeking; audio/video sync is by wall clock (both start together, no drift correction); each frame is a new kitty image (kitty evicts old ones by its storage quota); videos in the thread pane play the same way but were not tested there; needs `ffmpeg`/`ffprobe` on PATH (a clear error otherwise).
- Search: only the first 50 matches (no paging); a hit older than the loaded history is not scrolled to (the conversation opens, nothing is selected).
- Microphone: the capture pipeline (`MicConverter`: mixdown, resampling, 20 ms framing) is unit-tested with a synthetic 44.1 kHz stereo tone; opening the device is not verified live. A live attempt with `PIPEWIRE_NODE` pointing at a temporary null sink did **not** route the capture there — the default microphone (the user's headset) was used for ~12 s into the private test huddle; the recording was deleted. Do not run microphone tests without explicit device selection.
- Huddles: roster departures follow the SDK rule (an AUDIO_STREAM_ID_INFO entry with neither attendee id nor muted = left), found when B kept A after A had left; no echo cancellation or noise suppression on the microphone; no video or screen share.
- `/upload` reads the whole file into memory and shows no progress; other `/commands` are not recognised (they are sent as text).
- Thumbnails: only tested in kitty (sixel/iTerm2/half-block paths untested); not tested inside the thread pane; a failed thumbnail leaves its reserved rows blank (no error text); GIFs show their first frame only.
- Notifications: clicking one does nothing (no jump to the conversation); per-channel notification preferences (all/mentions/nothing) and keyword alerts are not honoured; muted channels come from boot prefs only (no live pref updates); the daily DND schedule can be off by an hour across a DST change.
- A freshly opened kitty window does not report focus-out until it has had one focus-in, so right after launch an unfocused slacktui may think it is focused (harness quirk; normal use is unaffected once the window has been focused).
- Autocomplete does not complete on a mouse click.

## Bug log

- Day separators and the thread "N replies" rule were attributed to the next message, so selecting it highlighted the rule too. Found by screenshot after a search jump; tests now assert they belong to no message.

- The `WideSafe` filter (previous fix) measured the raw cell symbol, so a kitty image cell carrying the image transmission escape (thousands of characters, forced width 1) looked thousands of columns wide and the rest of its row was dropped: the viewer image lost its top row. Found by screenshot; now uses `Cell::cell_width()`, which honours forced widths. Test `forced_width_cells_cover_nothing`.

- VS16 emoji (❤️) shifted the rest of their row by one column (`[❤  1]`) and left stale characters after scrolling or switching conversation. Reproduced in isolation with a two-frame ratatui program in kitty. Cause: ratatui-core 0.1.2 re-emits the emoji's trailing cell right after it; the crossterm backend sees an adjacent cell and skips the cursor move, but the terminal had already advanced two columns. Fixed with a backend wrapper (`terminal::WideSafe`) that drops writes into columns covered by the wide character just written. Test `covered_columns_are_dropped`; the same live scenario is clean now.

- Notifications never fired: `dnd.info` reports the *next start* and *next end* separately (today 00:00 and tomorrow 09:00 for a daily 00:00–09:00 schedule), and reading them as one span made DND look active for 33 h. Found with a debug log of each notification decision; now read as a daily schedule. Test `dnd_windows` uses the real values.

- `message_replied` carries the parent *without* reactions/files, and replacing the parent wiped its reactions (👍 vanished after any reply). Confirmed by capturing A's RTM events with `ctl listen`; now only the thread fields are merged. `message_changed` likewise keeps reactions, reply counts and files when the event omits them (a missing `reply_count` reset it to 0, caught by the new test). Test `message_replied_keeps_reactions`.

- Live thread replies were counted twice (`💬 3 replies` for 2): the reply event bumped `reply_count` and `message_replied` also carried the server count. Found by screenshot; a reply is now counted only if newer than the parent's `latest_reply`. Test `reply_counted_once_with_message_replied`.
- " (edited)" was appended after wrapping and got cut off in narrow panes. Found by screenshot in the thread pane; now wraps to its own line. Test `edited_mark_wraps_instead_of_overflowing`.

- Duplicate day separator after system messages (`channel_join`). Found by screenshot; fixed in 9936455 and guarded by the test `one_day_separator_around_system_messages`.
- Status-bar unread counts were computed only at boot. Found by screenshot; now computed per frame (104b768).

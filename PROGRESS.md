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
| Inline image thumbnails: terminal graphics detected at startup (`Picker::from_query_stdio`, half-block fallback); rows reserved from `original_w/h` (≤ 36×10 cells, no upscaling) so nothing jumps; thumbnails fetched lazily when on screen (`thumb_*` with the session cookie), decoded/encoded off the UI task, cached per file; drawn with `SlicedImage` so partly scrolled images are clipped | done | LIVE (kitty: B's 800×500 PNG shown under its file line; clipped correctly after scrolling; no remnants after switching conversation) + UNIT (sizing, reserved rows) | a113b72 |

Tests: `cargo test` → slack-core 8, slacktui 41, all passing. `cargo build` has no errors.
Remaining dead-code warnings: `Composer::cursor/is_multiline`, `Sidebar::select_id`, `Workspace::team_id`.

## Next (in order, each as a small, separately verified step)

1. Images, part 2: a full-size image viewer (Enter/`o` on an image), then upload from a path.
2. Video playback with sound (ffmpeg-next → frames via ratatui-image; cpal audio).
3. Huddles: reverse engineer the signalling first (observe the web client inside RSlack), then audio.
4. Search, channel info/members, pins, quick switcher (Ctrl+K), help overlay.

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
- Key hints for `e`/`d`/`r`/`y`/`Y` are not shown anywhere yet (help overlay is in "Next").
- `@here/@channel/@everyone` are offered in DMs too.
- Thumbnails: only tested in kitty (sixel/iTerm2/half-block paths untested); not tested inside the thread pane; a failed thumbnail leaves its reserved rows blank (no error text); GIFs show their first frame only.
- Notifications: clicking one does nothing (no jump to the conversation); per-channel notification preferences (all/mentions/nothing) and keyword alerts are not honoured; muted channels come from boot prefs only (no live pref updates); the daily DND schedule can be off by an hour across a DST change.
- A freshly opened kitty window does not report focus-out until it has had one focus-in, so right after launch an unfocused slacktui may think it is focused (harness quirk; normal use is unaffected once the window has been focused).
- Autocomplete does not complete on a mouse click.

## Bug log

- VS16 emoji (❤️) shifted the rest of their row by one column (`[❤  1]`) and left stale characters after scrolling or switching conversation. Reproduced in isolation with a two-frame ratatui program in kitty. Cause: ratatui-core 0.1.2 re-emits the emoji's trailing cell right after it; the crossterm backend sees an adjacent cell and skips the cursor move, but the terminal had already advanced two columns. Fixed with a backend wrapper (`terminal::WideSafe`) that drops writes into columns covered by the wide character just written. Test `covered_columns_are_dropped`; the same live scenario is clean now.

- Notifications never fired: `dnd.info` reports the *next start* and *next end* separately (today 00:00 and tomorrow 09:00 for a daily 00:00–09:00 schedule), and reading them as one span made DND look active for 33 h. Found with a debug log of each notification decision; now read as a daily schedule. Test `dnd_windows` uses the real values.

- `message_replied` carries the parent *without* reactions/files, and replacing the parent wiped its reactions (👍 vanished after any reply). Confirmed by capturing A's RTM events with `ctl listen`; now only the thread fields are merged. `message_changed` likewise keeps reactions, reply counts and files when the event omits them (a missing `reply_count` reset it to 0, caught by the new test). Test `message_replied_keeps_reactions`.

- Live thread replies were counted twice (`💬 3 replies` for 2): the reply event bumped `reply_count` and `message_replied` also carried the server count. Found by screenshot; a reply is now counted only if newer than the parent's `latest_reply`. Test `reply_counted_once_with_message_replied`.
- " (edited)" was appended after wrapping and got cut off in narrow panes. Found by screenshot in the thread pane; now wraps to its own line. Test `edited_mark_wraps_instead_of_overflowing`.

- Duplicate day separator after system messages (`channel_join`). Found by screenshot; fixed in 9936455 and guarded by the test `one_day_separator_around_system_messages`.
- Status-bar unread counts were computed only at boot. Found by screenshot; now computed per frame (104b768).

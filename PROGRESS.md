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
| Reaction picker (`r`): search, favourites, custom emoji, optimistic toggle | done | LIVE (🚀 added, no double count from the RTM echo). Server-side state NOT read back independently. | 5ab8258 |
| Edit own messages: `e` on a selected message or ↑ in an empty composer; mentions/channels/@here back to composer mentions; Enter = `chat.update`, Esc cancels; optimistic with rollback | done | LIVE (edited a message with a mention, `&` and `<…>`; B's `ctl history` read back the right `<@U…>`/`&amp;`/`&lt;` text; `e` path and Esc cancel checked by screenshot) + UNIT (round trip) | d2cdb2f |
| Delete own messages: `d` → y/n popup → `chat.delete`; optimistic with rollback; refuses others' messages | done | LIVE (`n` keeps it, `y` removes it, B's history no longer has it; `d` on B's message shows a refusal) | see git log |

Tests: `cargo test` → slack-core 8, slacktui 25, all passing. `cargo build` has no errors.
There are dead-code warnings for the planned autocomplete API (`Composer::trigger/complete`).

## Next (in order, each as a small, separately verified step)

1. Copy message text (`y`, via arboard) and the permalink.
2. Threads pane: Enter on a message opens its replies (`conversations.replies`). The composer then
   targets the thread (`thread_ts`); the thread state map already exists (`Workspace::threads`).
3. Autocomplete popup for `@user`, `#channel` and `:emoji:` (composer `trigger()`/`complete()` already exist and are tested).
4. Desktop notifications (notify-rust) for DMs, mentions and @here, honouring DND and muted channels and the terminal focus events.
5. Images inline (ratatui-image, kitty protocol; `File::preview_url`), an image viewer, upload from a path.
6. Video playback with sound (ffmpeg-next → frames via ratatui-image; cpal audio).
7. Huddles: reverse engineer the signalling first (observe the web client inside RSlack), then audio.
8. Search, channel info/members, pins, quick switcher (Ctrl+K), help overlay.

## Known gaps / not verified

- Older-history paging, and the edit/delete/thread-reply RTM paths, have never been exercised live.
- The test workspace has very little history, so scrolling performance with large channels is unknown.
- The reaction picker's server-side effect is inferred from the absence of an API error plus the RTM echo.
- Edit/delete rollback (`Net::Restore`) has not been exercised against a real API failure.
- The RTM `message_changed`/`message_deleted` paths for *other people's* edits are still UNIT only (there is no `ctl edit`/`ctl delete` yet to drive them from B).
- Status-bar messages (errors, refusals) stay until the next one replaces them; they are never cleared on a timer.
- The message being edited is not highlighted while the composer has focus.
- Reaction chips with VS16 emoji (❤️) sometimes render with an extra blank cell (`[❤  1]`); seen in screenshots, not investigated.

## Bug log

- Duplicate day separator after system messages (`channel_join`). Found by screenshot; fixed in 9936455 and guarded by the test `one_day_separator_around_system_messages`.
- Status-bar unread counts were computed only at boot. Found by screenshot; now computed per frame (104b768).

# Pause / schedule and the second round of fixes (queue V, part 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** Pausing notifications and the notification schedule as designed (approved 2026-10-20), plus the user's
second list of problems found in v0.6.0.

**Architecture:** Slack calls in `crates/slack-core/src/api.rs`; DND state in `crates/klack/src/tui/state.rs`
(`Dnd`); commands in `commands.rs`; F6 rows in `prefs.rs`; keys in `keymap.rs`; the rest in the existing TUI modules.

**Tech Stack:** Rust, ratatui 0.30, crossterm 0.29, Slack web API (`dnd.*`, `users.prefs.*`).

**Spec:** `docs/superpowers/specs/2026-10-20-pause-and-schedule-design.md` (Tasks 1–4); the user's report, quoted per
task (Tasks 5–14); keys and modes: `docs/superpowers/specs/2026-10-19-keymap-rework-design.md`.

## Global Constraints

- The project's step routine (build without warnings, tests green, live check in the kitty test window, PROGRESS row
  with honest LIVE/UNIT/NOT and "(this commit)", one commit with the `Co-Authored-By` trailer; the controller pushes).
- DND writes only on the two rslack-testbed test accounts (user's permission 2026-10-20): snapshot their `dnd_*`
  prefs (`users.prefs.get`) and `dnd.info` before, restore exactly after. Never on the user's own `test` profile.
- No `cargo fmt`; no sound on the user's speakers; never the real mic/camera/screen; the user's ~/.config/klack files
  backed up and restored.

## Review Focus

- Resume while BOTH a snooze and a scheduled quiet period are active — notifications must really be on afterwards.
- A schedule whose quiet period crosses midnight and the weekday boundary (Fri 17:00 → Mon 09:00 with weekdays).
- The account's time zone differing from klack's (labels, "until" times).
- Restarting klack while a thread was open in a conversation that no longer exists / was left.
- Hover rows while a list scrolls (wheel) — the shaded row follows the pointer, not a stale index.

---

### Task 1 (V7): Slack calls and state for pause and schedule

- [ ] Research on test account B (read prefs; then write and restore): confirm `dnd.endDnd` ends the current
  scheduled quiet period without changing `dnd_enabled`/hours (check `users.prefs.get` and `dnd.info` before/after);
  confirm `users.prefs.set` (or the call Slack's web app uses — inspect with `ctl api`) can write `dnd_days`,
  `dnd_start_hour`/`dnd_end_hour` and the per-day `dnd_enabled_<day>` / `dnd_before_<day>` / `dnd_after_<day>`,
  and what `dnd_enabled_<day>` values exist (seen: "partial"). Record findings in the report and in the spec's
  "Research so far".
- [ ] `slack-core`: `Client::end_dnd()`, `Client::dnd_schedule() -> Result<Schedule>`,
  `Client::set_dnd_schedule(&Schedule) -> Result<()>` with `pub enum ScheduleDays { Off, EveryDay, Weekdays, Custom }`
  and `pub struct Schedule { days, range: Option<(String, String)>, per_day: [DayHours; 7] }` (shape as research
  dictates; names may change — tell the controller). Unit tests parse a recorded `users.prefs.get` fixture (ids
  removed) and build the `users.prefs.set` parameters.
- [ ] `state::Dnd` gains what the status line needs: `until: Option<f64>` = end of whatever is active now (snooze end
  or scheduled period end), from `dnd.info` (`snooze_endtime`, `next_dnd_start_ts`/`next_dnd_end_ts`). Test with
  fixtures: snooze only, scheduled only, both, neither.
- [ ] Close (V7, `feat: Slack calls for pausing and the notification schedule (V7)`).

### Task 2 (V8): `/dnd` toggles; resume ends both; `␣an` / `␣aN`

- [ ] Tests: `commands::parse("/dnd")` → `Command::Dnd(DndAction::Toggle)`; `/dnd off` → `Resume`; `/dnd 2h` →
  `Pause(120)`; resume calls `endSnooze` when snoozed and `endDnd` when in a scheduled period (pure decision
  function over `Dnd`); the autocomplete entry for `/dnd` reads "pause notifications for 30 minutes" when on,
  "resume notifications" when off; keymap: `␣an` = `pause_notifications` (toggle), `␣aN` = new `pause_for`
  (prefill `/dnd `), `␣ap` gone; which-key label of `␣an` follows the state.
- [ ] Implement per the spec's goals 1–3 (and its decision "if endDnd turned the schedule off… say notifications resume
  at 09:00"). F6's old Pause row is replaced in Task 3, not here.
- [ ] Live (test accounts; restore): in scheduled quiet `/dnd off` → 🔕 gone, notifications on (dnd.info); `␣an` →
  paused 30 min; `␣an` → resumed; `␣aN` → `/dnd ` in the box.
- [ ] Close (V8).

### Task 3 (V9): "🔕 until 09:00" and F6's Notifications row

- [ ] Tests: `profile::availability` shows `🔕 until 09:00` for a snooze and for a scheduled period alike,
  `🔕 until Mon 09:00` when not today, in the clock format setting; the status line's shortest form keeps `🔕`
  (status.rs); F6 row "Notifications": "on" / "paused until 09:00"; Enter toggles like `␣an`; `+`/`-` step the pause
  (30 min, 1 h, 2 h, 8 h) while paused (replaces `Field::Pause`/`PAUSES`; update `availability_becomes_commands`).
- [ ] Implement; live (test account in its scheduled period, then outside): texts as above.
- [ ] Close (V9).

### Task 4 (V10): Editing the schedule (F6 and `/dnd schedule`)

- [ ] Tests: F6 → Availability rows "Notification schedule" (off / every day / weekdays / custom), "Receive from … to …"
  for every day / weekdays, per-weekday rows for custom (range / all day / none per research); edits produce the
  exact `set_dnd_schedule` change; rows name the Slack zone when it differs from klack's; custom read-only fallback if
  Task 1 found it unwritable. `/dnd schedule off|every day HH:MM-HH:MM|weekdays HH:MM-HH:MM` parse + autocomplete.
- [ ] Implement; live on test account B: set weekdays 10:00-18:00 receive hours, see Slack's prefs change, set a custom
  day, then restore the snapshot exactly.
- [ ] Close (V10).

### Task 5 (V11): Keys `␣fc`, clear the message box

User: "<leader>f# is inconvenient, default keymap for "browse channels" should be <leader>fc"; "we should come up with a
shortcut that clears compose box".

- [ ] Tests: `␣fc` → BrowseChannels (`␣f#` gone); composer action `clear` ("clear the box"), defaults `<leader> x` and
  `<leader> backspace` (so Ctrl+Space x or Ctrl+Space Backspace while typing — the second asked by the user); clearing is final — no recovery (user, 2026-10-20), ↑ in the empty box keeps "edit your last message"; an edit in
  progress is cancelled like Esc instead (never clears someone's edit silently) — Claude's calls.
- [ ] Close (V11).

### Task 6 (V12): Status notes clear on the next command key

User: "messages linger too long on a status bar … pressing Esc, Enter or basically anything that counts as a direct
command or a start of a command (for example just <leader>) should clear the message."

- [ ] Test (pure helper): a key that resolves to an Action or starts a sequence clears `app.status` before it acts
  (the action may set a new note); plain typing in INSERT and mouse moves don't. The ringing invitation isn't a note
  and stays.
- [ ] Close (V12).

### Task 7 (V13): Away shows; your presence dot

User: "<leader>aa - turns on "away" mode. green dot stays green, only "live" message goes away".

- [ ] Find why: likely the status line's short info drops the availability ("away") and the green `●` is the
  connection, read as presence. Claude's call: the info's dot becomes **your presence** as in Slack — green `●`
  active, dim `○` away, `z` added while notifications are paused; the connection shows only as text while not
  connected ("connecting…"); the short and shortest forms keep the dot. Confirm the root cause first; if it is a
  different bug (presence not updated after `␣aa`), fix that and report.
- [ ] Tests on status.rs / availability; live: `␣aa` → `○`, `␣aa` → `●` (restore presence).
- [ ] Close (V13).

### Task 8 (V14): Start in NORMAL; restore an open thread

User: "on app launch user lands immediately into insert mode focused on a compose box instead of normal mode focused on
a message (or thread) panel"; "if app was closed with a thread panel open on relaunch it is not restored".

- [ ] Tests: the start focus is `Focus::Messages` with the newest message selected (Thread with newest reply when a
  thread is restored); the last-conversation file also records the open thread (`channel/thread_ts`), read back at
  start; a thread that can't be loaded is dropped quietly.
- [ ] Close (V14).

### Task 9 (V15): Hover in dialogs

User: "activity, threads and reactions (possibly other) windows do not respect mouse hover events"; "search window
doesnt respect mouse hover events"; details too (Task 10).

- [ ] Tests: each list's `row_at` (V5/V6) reused for hover; the hovered row is shaded like the sidebar's hover (no
  selection change) in jump, search, threads, activity, reaction picker, preferences (rows, categories), menu (it
  already moves the selection — keep), mover, save, details (Task 10 adds its rows). Hover clears when the pointer
  leaves or the dialog closes / the terminal loses focus.
- [ ] Close (V15).

### Task 10 (V16): Conversation details: clicks and hover

User: "conversation detail window doesnt respect mouse events, neither hover nor clicks".

- [ ] Tests: `Details::tab_at` / `row_at`; a click on a tab switches; a click on a member / pin acts as Enter (profile
  card / go to the pin), as the list dialogs do (V5 Ruling 1); hover shading as Task 9.
- [ ] Close (V16).

### Task 11 (V17): The huddle view and the bars

User: "there's 0 spacing between status bar and huddle bar"; "huddle view contains redundant control prompts. they
should be removed"; "status bar when on huddle view doesnt display `<leader> actions` help prompt - the most useful one".

- [ ] Tests: one empty row between the huddle bar and the status line (layout); the huddle view draws no key-hint line
  (`Keys`/`controls` removed if unused); `hints_for(Huddle)` includes the leader "actions" hint like NORMAL panes.
- [ ] Close (V17).

### Task 12 (V18): A thread opened from the huddle view

User: "if user goes opens thread panel from a huddle view and then closes it via Esc - it closes the huddle view too".

- [ ] Reproduce (huddle view → `␣ht` or Enter on the thread → Esc); test: closing that thread returns to the huddle
  view (focus Huddle, view still shown).
- [ ] Close (V18).

### Task 13 (V19): Screen share fills its tile

User: "screen sharing doesnt occupy the entire tile in huddle view. thus maximizing the tile does absolutely nothing and
just increases the tile size, but not the video frame size".

- [ ] Find why (video.rs / huddle_view.rs scaling: a fixed decode size? aspect fit?); test the fit computation (frame
  size → cells for a tile); the frame scales to the tile keeping its aspect, maximized included. Live with a synthetic
  share from B (never the real screen — see the video test rules memory).
- [ ] Close (V19).

### Task 14 (V20): Search shows files

User: "when searching for files/images/videos/snippets by name via search - search window entries just displays author,
channel and date. no file name, no content".

- [ ] Find what `search.messages` returns for file hits (and whether `search.files`/`search.all` is needed); test the
  hit's line: `📎 name` (title), and the snippet's first line or the message text when there is one.
- [ ] Close (V20).

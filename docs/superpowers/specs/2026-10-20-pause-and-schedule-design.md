# Pausing notifications and the notification schedule — design

Brainstormed with the user on 2026-10-20. Marks: **(user)** said or chosen by the user; **(shown)** Claude's
proposal the user saw and accepted; **(Claude's call)** decided while writing, not yet seen. Part of queue V.

## Why

The user saw 🔕 in the status bar, typed `/dnd off`, and nothing happened: klack's `/dnd off` only ends a
*snooze* (`dnd.endSnooze`), but their quiet time came from Slack's *notification schedule* (weekdays
17:00–09:00), which klack can neither end nor change. "we need to come up with a way to clearly communicate both to
the user and for him to manipulate both. right now app doesnt provide the user a way to manipulate his schedule."

Two Slack concepts, kept separate in the code, merged for the user where Slack merges them:

- **Snooze** — "pause notifications for 30 minutes / until …", started by the user.
- **Schedule** — recurring quiet hours (every day, weekdays, or per-day custom hours). Slack can also end the
  *current* scheduled quiet period early without changing the schedule.

## Goals

1. **`/dnd` behaves like Slack's** **(user)**: plain `/dnd` toggles; its autocomplete label states what it will do
   now — "pause notifications for 30 minutes" when notifications are on, "resume notifications" when they're off —
   never "toggle". Plain `/dnd` while on pauses for 30 minutes. `/dnd 30m|2h|1d|N` and `/dnd off` keep working.
2. **Resume ends whatever silences you** (Claude's call, fixes the user's bug): `/dnd off`, plain `/dnd` while off,
   `␣an` while off and F6's resume all end a snooze (`dnd.endSnooze`) **and** the current scheduled quiet period
   (`dnd.endDnd`), leaving the schedule itself untouched.
3. **Keys** **(user)**: `␣an` acts at once — pause 30 minutes, or resume when paused — labelled by what it will do
   (which-key shows "pause notifications (30 min)" or "resume notifications"); `␣aN` puts `/dnd ` into the message
   box (INSERT) to type any period. (Replaces U6's `␣ap`; the keys.toml action name `pause_notifications` stays
   for `␣an`'s action — Claude's call, keeps users' files working; `␣aN` is a new action.)
4. **Showing it** **(user: like Slack, don't say which kind; Claude's idea shown: say until when)**: the status line's
   availability shows `🔕 until 09:00` (the end of whatever is active: the snooze end or the scheduled period's
   end; a weekday name when it isn't today, e.g. `🔕 until Mon 09:00`). In the status line's short form it is `🔕`.
   No "snoozed" vs "scheduled" wording.
5. **The schedule is editable** **(user)**, including Slack's per-day custom hours **(user: "allow user to set custom
   … just as original does")**, worded the way Slack's current app words it — the hours you **receive**
   notifications in, paused outside them (Claude's call: "just as original does"; Slack's prefs store both forms,
   see Research):
   - F6 → Availability gets: **Notification schedule** (off / every day / weekdays / custom); for every day and
     weekdays one **Receive from … to …** range; for custom one row per weekday (Mon … Sun), each a range, "all
     day" or "none" — whatever Slack's per-day states really are. Saved to Slack at once, like the other
     Availability rows (Claude's call on the layout).
   - Times are shown and typed in the Slack account's time zone (that's what Slack stores and applies); when it
     differs from klack's zone (F6 → Language & region) the rows say which zone, e.g. "(Asia/Jerusalem)". No
     conversion — Claude's call (conversion could move per-day ranges across days).
   - A command for keyboard users (Claude's call): `/dnd schedule off`, `/dnd schedule every day 22:00-08:00`,
     `/dnd schedule weekdays 17:00-09:00`. Per-day custom hours are edited in F6 only (a command for seven ranges
     would be unreadable) — Claude's call.
   - Times in 24-hour HH:MM in the input; shown in the user's chosen clock format (F6 → Language & region).

## Non-goals

- Distinguishing snooze from schedule in the status line **(user)**.
- Per-conversation mute (separate queue V item) and notification levels (exist already).
- A dialog/wizard for the schedule — F6 rows are enough (Claude's call).

## Anti-goals

- Overwriting a schedule the user set in another Slack client with something less precise (e.g. collapsing a
  per-day schedule to one range) — klack writes exactly what the user edited.
- Resume that silently leaves notifications off (today's bug).

## What exists (v0.6.0 + V2–V6)

- `slack-core/src/api.rs`: `snooze(Option<u64>)` → `dnd.setSnooze` / `dnd.endSnooze`; `set_presence`;
  `call_raw` for any method. `ctl api …` can call anything for research.
- TUI: `commands.rs` `/dnd 30m|2h|1d|N|off` (`Command::Dnd(Option<u64>)`), `/status`, `/away`, `/active`; F6 →
  Availability rows Status, Appear away, Pause notifications (cycles 0/30/60/120/480); `refresh_dnd` (event_loop.rs)
  keeps the DND state; status line availability via `profile::availability`; U6's `␣a` group (`s` status, `c`
  clear, `a` away, `p` → `/dnd ` prefill).

## Research so far (2026-10-20, read-only on the test account B)

`users.prefs.get` has both forms: legacy `dnd_enabled`, `dnd_start_hour` "17:00", `dnd_end_hour` "9:00",
`dnd_days` "weekdays"; and per-day `dnd_enabled_<day>` ("partial" seen), `dnd_before_<day>` "8:00",
`dnd_after_<day>` "22:00" (receive-hours form), `dnd_weekdays_off_allday`. `dnd.info` returns `dnd_enabled`,
`next_dnd_start_ts`, `next_dnd_end_ts` (here 09:00 — matching the legacy 17:00–9:00 weekday window while
`dnd_days` = weekdays), `snooze_enabled`. The account's zone is Asia/Jerusalem (+3).

**Confirmed by writing on B (V7, 2026-10-20 ~01:00, restored exactly — diff clean):**

- `dnd.endDnd` ends the current scheduled quiet period (dnd.info moved from Tue 00:00–09:00 to Tue 17:00–Wed
  09:00) and changes no pref. It also ends a running snooze. Outside quiet hours it answers ok and changes nothing.
  Any later `users.prefs.set` of a schedule pref brings the ended period back (dnd.info recomputed).
- `users.prefs.set` takes `prefs` = a JSON object of several prefs in one call (or `name`/`value`). Slack checks the
  values and **ignores the whole write while still answering ok** when one is not taken (a bad time, an unknown
  value) — so klack checks times first and reads the prefs back.
- `dnd_days`: `every_day`, `weekdays`, `custom` (anything else ignored). **Slack applies the legacy pair for every
  day and weekdays** (quiet `dnd_start_hour` → `dnd_end_hour`; dnd.info end 7:05 / 9:00 / 10:00 followed it) and the
  **per-day form for custom** (dnd.info end 8:00 = `dnd_before_tuesday`). In weekdays, Saturday and Sunday have no
  quiet hours.
- Per day (custom): `dnd_enabled_<day>` is `partial` (notifications from `dnd_before_<day>` to `dnd_after_<day>`,
  paused outside — Tue 0:30–1:30 gave quiet from Tue 01:30 to Wed 08:00), `all_day` (**paused all day**: Tue 00:00
  → Wed 08:00) or `disabled` (**notifications all day**, no quiet hours: next start moved to Wed 00:00). Nothing else
  is taken. An inverted range (before 22:00, after 8:00) is accepted.
- Off: `dnd_enabled` false is taken **only together with `dnd_days` = `every_day`** (alone or with weekdays/custom it
  is ignored); setting `dnd_days` weekdays/custom turns `dnd_enabled` back on. dnd.info then says
  `dnd_enabled` false (the `next_*` values are meaningless: 1, or some time).
- Times: `H:MM`, any minute; Slack stores "09:00" as "9:00"; "25:00" ignored.
- `dnd_weekdays_off_allday` is writable (bool); with it true dnd.info showed start = end = Wed 00:00 — meaning
  unclear, klack leaves it alone.
- dnd.info with a snooze: `snooze_enabled`, `snooze_endtime`, `snooze_remaining`. When the snooze is the next quiet
  time (no schedule, or before the next period) Slack also reports it as `next_dnd_start_ts`/`next_dnd_end_ts`, and
  `dnd_enabled` true even with the schedule off; inside a scheduled period the `next_*` stay the period's.

## Decisions on the builder check's questions (Claude's calls)

- **If per-day custom hours can't be written** with klack's session (users.prefs.set) or not without loss: custom is
  shown read-only ("custom — edit in Slack"), every day / weekdays stay editable; never collapse a custom schedule.
- **If `dnd.endDnd` turns the schedule off or changes it** instead of only ending the current quiet period: resume
  during scheduled quiet ends a snooze if any and otherwise says "notifications resume at 09:00 (schedule) —
  change the schedule in F6"; it never edits the schedule behind the user's back.
- **F6's "Pause notifications" row** becomes **Notifications**: "on" or "paused until 09:00" (the same text as the
  status line, any cause); Enter toggles like `␣an` (pause 30 min / resume); `+`/`-` step the pause length
  (30 min, 1 h, 2 h, 8 h) while paused.
- The status line's shortest form gains `🔕` (it shows no availability today).

## Left to the builder (with research first)

- **Slack's API for the schedule** must be confirmed against the real API on the test workspace before coding
  (read-only first: `users.prefs.get`/`dnd.info` on the test accounts; write only on a test account, restoring its
  schedule — weekdays 17:00–09:00 today — afterwards). Expected fields (unverified): `dnd_enabled`,
  `dnd_start_hour`, `dnd_end_hour`, `dnd_days` (`every_day` / `weekdays` / `custom`) and some per-day custom form;
  whatever Slack really uses wins.
- Where the "until" time comes from (`dnd.info`: `next_dnd_end_ts`, `snooze_endtime`) and refreshing it when it
  passes.
- The autocomplete label's mechanism (the list is built from a static table today).
- Step order of the plan.

## Constraints

- The project's step routine (small verified steps, tests, live check on the test workspace, PROGRESS, commit,
  push after review). The user's own accounts are never written; test accounts are restored.

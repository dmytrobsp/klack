# Threads, Activity and Huddles views; people's statuses — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Threads and Activity become main-pane views with full message context, a new Huddles view lists live and
recent huddles, people's status emoji show next to their names, and the message hover shade covers the whole row.

**Architecture:** `App` (private struct in `crates/klack/src/tui/event_loop.rs`, "EL") gains a `view: View` field that
the main-pane draw cascade (EL ~5472-5512) and key routing consult before the conversation branch; Threads/Activity
move from popups (`Option<Threads>`, `Option<Activity>`, drawn as modals) to that view. The Threads feed reuses
`MessagePane`'s line builder (`build_lines_range`, messages.rs:749) per block so messages look exactly as in a
conversation. Live huddles come from a new Flannel helper in slack-core plus the RTM `sh_room_*` events klack already
applies to `ws.huddles`; recent huddles from `huddles.history`.

**Tech Stack:** Rust, ratatui 0.30, crossterm 0.29, ratatui-image, reqwest, tokio; tests with ratatui `TestBackend` and
inline `serde_json::json!` reply shapes.

**Spec:** `docs/superpowers/specs/2026-10-21-threads-activity-huddles-design.md` (research:
`~/projects/slack-rea/notes/huddles-list.md`, `notes/activity.md`). Code map from a read of the tree: see each task.

## Global Constraints

- No new crate dependencies. No `cargo fmt`. Never build `target/release`. `cargo build` + `cargo test` green before
  every commit.
- Privacy: no real names, ids, emails, tokens or work-workspace content in code, tests, docs or PROGRESS (say A / B /
  "the test workspace"). Reply shapes in tests are made up from recorded *shapes*, never real values.
- Live checks only on rslack-testbed: `--profile default` (A), `--profile b` (B), in a fake HOME with every sound off;
  never `--profile test`; own kitty window per task; never desktop screenshots (`kitty @ get-text`); never a real
  mic/camera/screen (test huddles use `klack ctl --profile b huddle-media … --share-test`/synthetic media only).
- Everything keeps working: keyboard-first + mouse, NORMAL/INSERT/DIALOG modes, which-key, keys.toml overrides
  (`keymap.rs` table), F6 help for new settings, themes (`theme::p()` tokens), i18n (`i18n::tr` on every new string),
  narrow terminals (`app.narrow`), pictures (all graphics modes), OSC 8 links, `↪` app links, the ctl CLI.
- Ctrl+T opens Threads, Ctrl+A opens Activity (the views); Esc from a view returns to the conversation you were in;
  the key of the view you are in refreshes it; at start klack restores the last conversation, never a view.
- Thread read-marking in the feed: after a block has been fully on screen for **2 seconds** (a setting,
  `[threads] read_after_secs = 2`, like `preview_refresh_secs`); not while scrolling past.
- Activity tab types: All = `at_user,at_user_group,at_channel,at_everyone,keyword,message_reaction,thread_v2,dm,mpdm`;
  DMs = `dm,mpdm,bot_dm_bundle` (never `bot_dm`: it fails the whole request); Mentions =
  `at_user,at_user_group,at_channel,at_everyone,keyword`; Threads = `thread_v2`. Mode `chrono_reads_and_unreads`.
- Huddle badge = number of live huddles you can join (member, not muted, ≥1 participant, not the one you are in).
- Own huddle state: do exactly what the official client does (gate on the `huddles_set_status` pref, same fields,
  ~300 s expiry refreshed ~every 240 s) — never touch status emoji/text.
- Each task: one PROGRESS.md row inside the table (`| VW<n> — … | done | UNIT: … / LIVE: … / NOT: … | (this commit) |`,
  just above the `Tests:` line; update its klack count), README updated for anything a user sees.

## Review Focus

1. A feed/Activity item whose message can't be fetched (deleted, no access, `message_at` → `None`) shows "message not
   available" in its place — never a blank block, a panic, or a stuck "Loading…". (VW4, VW5 tests)
2. Leaving a view or scrolling before 2 s never marks a thread read; a block taller than the pane is marked once its
   unread replies have all been on screen for 2 s. (VW4 test)
3. Status expiry: `status_expiration` 0 = never expires (show); a past expiry hides the emoji without an event; an
   `in_a_huddle` state past `huddle_state_expiration_ts` shows nothing. (VW2 test)
4. Live huddles after an RTM reconnect or an `ended_huddle_update`: the list and the badge are refetched/cleared, no
   stale "live" huddle survives `has_ended`. (VW7 test)
5. Narrow terminals (`app.narrow`, ~80 columns) and a collapsed sidebar: the views and the three sidebar rows fit
   without wrapping garbage; the Activity tab bar truncates. (VW3, VW5, VW7 TestBackend at 80×24)

---

### Task 1 (VW1): The hover shade covers the whole message row

**Files:**
- Modify: `crates/klack/src/tui/messages.rs` (hover painting ~:346-353; the AA2 edge-to-edge fill ~:423-441)
- Test: `crates/klack/src/tui/messages.rs` (tests module)

**Interfaces:** Produces nothing new; `MessagePane::hover: Option<String>` keeps its meaning.

- [ ] **Step 1: Failing test** `the_hover_fills_the_whole_row` beside `the_selection_fills_the_whole_row` (:1156): two
  messages, `pane.hover = Some(<short one>)`, nothing selected → every row of the hovered message has
  `bg == p.hover` at `x = 1` and `x = 38` (40-wide backend), `!= p.hover` at the border (39); the other message's rows
  have no `p.hover`. Second case: the same message selected *and* hovered → `p.selection` wins on every row.
- [ ] **Step 2:** `cargo test -p klack the_hover_fills_the_whole_row` → FAIL.
- [ ] **Step 3:** Replace the span `patch_style` hover with a buffer row fill using `theme::p().hover` for rows whose
  owner is `hover` (and not selected/editing), done in the same pre-render pass as AA2's selection fill; hovered rows
  with a loaded picture reset the picture's cells to `Color::Reset` like AA2.
- [ ] **Step 4:** `cargo test` (whole workspace) → PASS. Live: fake home, A, move the mouse over a short message
  (`kitty @ get-text --ansi`: the hover background reaches the right inner edge), also in the thread pane.
- [ ] **Step 5: Commit** "klack: the hover shade covers the whole message row (VW1)" + PROGRESS row.

### Task 2 (VW2): People's status emoji next to their names

**Files:**
- Modify: `crates/slack-core/src/types.rs` (`UserProfile` :105), `crates/klack/src/tui/state.rs` (new fn near
  `user_name` :410), `crates/klack/src/tui/sidebar.rs` (`conversation_line` :397-432), `crates/klack/src/tui/messages.rs`
  (author header in `build_lines_range` :798-818), the DM conversation header (where the title of a DM is drawn —
  `title` passed to `MessagePane::draw` from EL ~5508), the profile popup stays (profile.rs).
- Test: types.rs (serde), state.rs, sidebar.rs, messages.rs tests.

**Interfaces:**
- Produces: `UserProfile` fields `huddle_state: String` (`#[serde(default)]`), `huddle_state_expiration_ts: i64`
  (`#[serde(default)]`); `Workspace::status_of(&self, user: &str, now: i64) -> Option<Status>` with
  `pub struct Status { pub emoji: String /* name without colons, or "headphones" for in-a-huddle */, pub text: String, pub in_huddle: bool }`.
  Rules: custom `status_emoji` non-empty and (`status_expiration` is `None`/0 or > now) → that emoji + text; else
  `huddle_state == "in_a_huddle"` and `huddle_state_expiration_ts > now` → `headphones`, text "In a huddle",
  `in_huddle: true`; else `None`.
- Produces: `fn status_span(ws: &Workspace, user: &str, now: i64) -> Option<String>` in `text.rs` returning the emoji
  through `text::emoji_cell` (custom emoji pictures where the pane supports them; the sidebar is a plain Paragraph, so
  there `text::emoji` — a glyph or `:name:` — is used).
- Consumes: `ws.users` (state.rs:114), `user_change` already replaces the whole `User` (live.rs:310).

- [ ] **Step 1: Failing tests**
  - types.rs `profile_reads_huddle_state`: JSON with `"huddle_state":"in_a_huddle","huddle_state_expiration_ts":1000`
    → fields set; without them → `""`, `0`.
  - state.rs `status_rules`: (a) emoji `palm_tree`, text "Vacationing", expiration 0 → Some(palm_tree); (b) expiration
    500, now 600 → None; (c) no emoji, `in_a_huddle`, expiry 700, now 600 → Some(headphones, in_huddle); now 800 → None;
    (d) emoji + in_a_huddle → the emoji wins.
  - sidebar.rs `dm_row_shows_the_status_emoji`: a DM whose user has `palm_tree` → the drawn row contains the glyph right
    after the name; a long name is truncated so the emoji still fits (width budget `room` :422).
  - messages.rs `author_line_shows_the_status_emoji`: the header line contains name, then the emoji, then the time.
- [ ] **Step 2:** run them → FAIL.
- [ ] **Step 3:** Implement as the Interfaces say. In the message header, put the status before the time span and run the
  header through the same `emoji_cells` pass `message_body` uses so custom emoji become pictures (slot line offsets!).
  The status **text** shows in a DM's pane title ("Name — 🌴 Vacationing") and as a hover note over a name in the
  sidebar (reuse the existing hover-note mechanism used for links, Z's hover note).
  "now" = the wall clock; the expiry is re-checked each draw (no event needed).
- [ ] **Step 4:** `cargo test` → PASS. Live: B sets a status with an expiry (`klack ctl --profile b api users.profile.set
  profile='{"status_emoji":":palm_tree:","status_text":"Vacationing","status_expiration":<now+120>}'`), A sees 🌴 in the
  sidebar and on B's messages, it disappears after expiry; B clears it afterwards.
- [ ] **Step 5: Commit** "klack: people's status emoji next to their names (VW2)" + README (Statuses) + PROGRESS row.

### Task 3 (VW3): Views in the main pane; Activity row in the sidebar

**Files:**
- Modify: `crates/klack/src/tui/event_loop.rs` (`App` :132-294, `draw` :5407/5472-5512, `overlay()` ~:2630, `mode_of`
  :5640, `mode_label` :5657, key routing :4148-4179, mouse `click_row` ~:4350 / `hover_dialog` :2508 / `wheel_in_popup`
  ~:2410 / `dialog_area` :2540 / `close_dialog` :2576, `sync_current` :369, Back :1014, `run_global` :4708-4709),
  `sidebar.rs` (`Item` :12, `rebuild` :115, `is_stop` :147, `hover_at` :256, `draw` :298), `threads.rs` / `activity.rs`
  (`draw` geometry), `keymap.rs` (contexts/labels; which-key :1463).
- Test: sidebar.rs, threads.rs, activity.rs, EL tests.

**Interfaces:**
- Produces: in EL `enum View { Conversation, Threads, Activity, Huddles }` and `App.view: View` (default
  `Conversation`); `fn open_view(&mut self, v: View)` (loads its data like `load_threads(false)` / `open_activity()`
  today; when already in `v` → refresh); `fn close_view(&mut self)` (back to `View::Conversation`, focus Messages).
  `App.threads: Threads` and `App.activity: Activity` become plain fields (no `Option`) owned by the view.
- Produces: `sidebar::Item::{Threads, Activity}` rows at the top in that order (VW7 adds `Item::Huddles` after
  Activity); `Sidebar::view_at(col,row) -> Option<Item>`; `Sidebar::on_view() -> Option<Item>` replacing `on_threads()`.
  The Activity row carries an unread mark when the feed has unread items (same `unread_mark` as Threads; the count of
  unread items fetched with the All tab's types when the workspace starts and on Refresh). `View::Huddles` exists from
  VW3 but nothing opens it until VW7.
- Produces: `Threads::draw(&mut self, f, area, ws)` and `Activity::draw(...)` draw into the given main-pane area (no
  centred modal, no `clear_modal`); `row_at`/`hover_at` keep working off `list/first/shown`.
- Contexts: `Context::Threads` / `Context::Activity` stay (keys.toml compatibility) but no longer force `Mode::Dialog`
  — the mode is NORMAL while a view has focus; status-line pills "THREADS" / "ACTIVITY" stay.

- [ ] **Step 1: Failing tests**
  - sidebar.rs `views_on_top`: after `rebuild`, items start `[Threads, Activity, Gap|Header…]`; ↓ from Threads stops on
    Activity; `view_at` maps their drawn rows.
  - EL test `ctrl_t_opens_the_threads_view_and_esc_goes_back`: an App with `current = Some("C1")`; `run_global(Threads)`
    → `view == Threads`, `mode() == Normal`; Back → `view == Conversation`, `current == Some("C1")`; `run_global(Threads)`
    twice → still `Threads` (refresh, a new `ThreadView` request).
  - threads.rs/activity.rs: existing `a_click_maps_to_…` and `…fills_the_whole_row` tests pass against a full-area draw
    (TestBackend 100×30, list spans the area minus the view's title row).
- [ ] **Step 2:** run → FAIL.
- [ ] **Step 3:** Implement: the draw cascade gets a branch before the MessagePane one (like `huddle_view`: hide the
  messages pane, `composer_area = Default` — except Threads keeps the composer, see VW4); key routing sends keys to the
  view's context when `view != Conversation` and focus is on the main pane; mouse handlers use the view's area instead
  of the dialog area; Enter on an item behaves as today (`open_thread_item`/`open_activity_item` → `open_hit`) and sets
  `view = Conversation`; `sync_current` treats all view rows like `Item::Threads` today. Startup never restores a view.
- [ ] **Step 4:** `cargo test` → PASS. Live (fake home, A): Ctrl+T, Ctrl+A, sidebar rows by keys and mouse, Esc back to
  the same conversation, narrow terminal (`kitty @ set-font-size` to get ~80 columns).
- [ ] **Step 5: Commit** "klack: Threads and Activity are views in the main pane; an Activity row (VW3)" + README +
  PROGRESS row.

### Task 4 (VW4): The Threads feed

**Files:**
- Create: `crates/klack/src/tui/feed.rs` (block layout + read tracking, pure, testable)
- Modify: `threads.rs` (draw the feed through `feed.rs`), `messages.rs` (expose what the feed needs from
  `build_lines_range` — no copy of its logic), EL (composer target, read-marking call, Enter/`i`), `settings.rs` +
  `prefs.rs` + `prefs_help.rs` (`[threads] read_after_secs`, default 2, steps `[0, 1, 2, 3, 5, 10]`, 0 = only when
  opened).
- Test: feed.rs, threads.rs, EL.

**Interfaces:**
- Consumes: `threads::Entry{channel, root, unread, latest}` (threads.rs:18), `MessagePane`/`build_lines_range`
  (messages.rs:749), `Here` (applink.rs:299), `mark_thread_read` (EL; `subscriptions.thread.mark`).
- Produces: `pub struct Block { pub channel: String, pub thread_ts: String, pub rows: Range<usize>, pub unread_rows: Range<usize> }`;
  `pub fn layout(here: &Here, entries: &[Entry], width: u16, replies_shown: usize) -> (Vec<Line<'static>>, Vec<Option<(String, String)>> /* row → (channel, message ts) */, Vec<Block>)`;
  `pub struct ReadClock { … }` with `fn tick(&mut self, visible: Range<usize>, blocks: &[Block], now: Instant, after: Duration) -> Vec<(String, String)>`
  returning the threads to mark read now (each once).
- Produces (EL): the composer's reply target `App.feed_reply: Option<(String /*channel*/, String /*thread_ts*/)>`;
  the composer title reads `tr("Reply in {where} thread")`.

- [ ] **Step 1: Failing tests**
  - feed.rs `block_has_header_root_more_and_last_replies`: an Entry with root + 5 replies, `replies_shown = 2` → lines in
    order: header (`#channel` + people), root's lines, "3 more replies", the last 2 replies' lines, a blank separator;
    every message line's owner is its `(channel, ts)`; the header/"more" lines own nothing.
  - feed.rs `the_new_line_sits_before_the_first_unread_reply`.
  - feed.rs `read_after_two_seconds_on_screen`: block fully visible from t0 → `tick` at t0+1.9 s returns nothing, at
    t0+2.0 s returns it, at t0+3 s nothing again; scrolled away at t0+1 s and back → the clock restarts; a block taller
    than the pane is returned once its `unread_rows` were visible for 2 s; `after = 0` → never returned.
  - feed.rs `missing_message_says_so`: an Entry whose root text is unavailable (message fetch `None`) → a dim
    "message not available" line, not an empty block.
  - EL `i_replies_to_the_selected_thread`: view Threads, a message of entry (C1, T1) selected, `Type` → composer focused,
    `feed_reply == Some(("C1","T1"))`; sending posts with `thread_ts = T1`.
- [ ] **Step 2:** run → FAIL.
- [ ] **Step 3:** Implement. Selection moves message by message (↑/↓/mouse) with AA2's full-row highlight; Enter on any
  message opens the whole thread at it (`open_thread_at`); the shared composer under the feed replies to the selected
  message's thread; after sending, the block refreshes. `replies_shown` = 3 (controller's call; long roots: cut after 12
  lines with "… more" — Enter shows all). Mark read through `ReadClock` + `mark_thread_read`, then drop the block's
  unread marks locally.
- [ ] **Step 4:** `cargo test` → PASS. Live: B replies in a thread A follows; A's feed shows it with the new line; after
  2 s on screen the sidebar Threads mark clears and (checked with B's view or a refetch) the thread is read on Slack; `i`
  reply from the feed lands in the thread.
- [ ] **Step 5: Commit** "klack: the Threads view is a feed with the messages in full (VW4)" + README + PROGRESS row.

### Task 5 (VW5): The Activity view

**Files:**
- Modify: `activity.rs` (tabs, parse `dm` items, layout), `crates/slack-core/src/api.rs` (`activity_feed` :310 takes
  types), EL (tab keys, unread filter, Enter jumps), `keymap.rs` (Activity block :608: `NextTab` Tab, `PrevTab`
  Shift+Tab, `Tab1`…`Tab4` 1–4, `Unreads` `u`).
- Test: activity.rs, api.rs.

**Interfaces:**
- Produces: `pub enum Tab { All, Dms, Mentions, Threads }` with `fn types(self) -> &'static str` (the exact lists in
  Global Constraints); `Client::activity_feed(&self, types: &str, limit: u32) -> Result<Value>`;
  `activity::Kind::Dm { latest_ts: String }` parsed from `item.bundle_info.payload.dm_entry.latest_message.{channel, ts}`.
- Produces: `fn layout(ws: &Workspace, items: &[Item], messages: &HashMap<(String,String),Option<Message>>, width: u16, today: NaiveDate) -> (Vec<Line<'static>>, Vec<Option<usize>> /* row → item index */)`
  with date headers ("Today", "Yesterday", "<Weekday>, <Month> <day>") in the user's time zone/format settings.

- [ ] **Step 1: Failing tests**
  - `tabs_ask_for_their_types`: each `Tab::types()` equals the Global Constraints list; none contains `bot_dm,` or ends
    with `bot_dm`.
  - `parses_a_dm_item`: shape `{feed_ts, is_unread, key, item:{type:"dm", bundle_info:{payload:{dm_entry:{latest_message:{channel:"D1", ts:"1.0"}}}}}}`
    → `Kind::Dm`, channel D1, ts 1.0.
  - `item_layout`: a mention item with a 6-line message → author + time + status emoji line, 3 message lines then "…",
    a "Mention in #c thread" line; a reaction item → "Reacted :x: to: …" with the pill; a DM item → "Direct message".
  - `date_headers_group_items`: items today, yesterday, 3 days ago → three headers in that order.
  - `unread_filter`: `u` hides read items and shows them again; selection stays on a visible item.
  - `missing_message_says_so` (as VW4).
- [ ] **Step 2:** run → FAIL.
- [ ] **Step 3:** Implement. Tab/Shift+Tab/1–4 switch tabs (refetch with that tab's types; keep each tab's list while the
  view is open); the tab bar is a row above the list (mouse clickable). Enter jumps: DM → open the DM; mention → its
  message (thread pane if a reply); thread → the thread at its newest/first unread reply; reaction → the reacted message.
- [ ] **Step 4:** `cargo test` → PASS. Live: B DMs, mentions and reacts to A in rslack-testbed; each tab lists the right
  items; Enter lands on each; narrow terminal.
- [ ] **Step 5: Commit** "klack: the Activity view — tabs, the messages themselves, date headers (VW5)" + README +
  PROGRESS row.

### Task 6 (VW6): Huddle lists in slack-core (Flannel + huddles.history), verified on a real test huddle

**Files:**
- Modify: `crates/slack-core/src/api.rs` (new fns), `crates/slack-core/src/types.rs` (new types), `docs/protocol/huddles.md`
  (the verified shapes, no values).
- Test: api.rs / types.rs tests with inline shapes.

**Interfaces:**
- Produces: `Client::flannel_url(&self) -> Result<String>` (`api.getFlannelHttpUrl include_external_workspaces=true` →
  `url`, cached for `ttl_seconds`); `Client::flannel(&self, method: &str, body: serde_json::Value) -> Result<Value>`
  (POST JSON `{token, …body}` to `<url>/<method>` with the session cookie, same 429/`ok` handling as `ask`);
  `Client::huddles_live(&self) -> Result<Vec<LiveHuddle>>` (`huddles/list`, then `huddles/info {channel_ids}` in chunks
  of 100); `Client::huddles_history(&self, cursor: Option<&str>, limit: u32) -> Result<(Vec<PastHuddle>, Option<String>)>`
  (limit ≤ 30).
- Produces: `pub struct LiveHuddle { pub channel: String, pub call_id: String, pub members: Vec<String> /* active − dropped */ }`;
  `pub struct PastHuddle { pub id: String, pub channel: String, pub channel_type: String, pub start: i64, pub end: i64, pub ended: bool, pub thread_ts: Option<String>, pub people: Vec<String>, pub missed: Vec<String> }`.

- [ ] **Step 1: Verify the live shape first.** B starts a test huddle in a rslack-testbed channel with synthetic media
  (`klack ctl --profile b huddle-media <channel> 120` — no camera/screen flags needed, never a real device); while it
  runs, A calls `huddles/list` and `huddles/info` through a throwaway `ctl api`-style probe that prints **shapes only**
  (keys/types/lengths). Record the shapes in `docs/protocol/huddles.md`. If the fields differ from the research note,
  adjust `LiveHuddle` parsing to what was seen. If the safety classifier blocks a probe, stop and report.
- [ ] **Step 2: Failing tests** `parses_huddles_info` (the verified shape, made-up ids → `members` = active − dropped),
  `parses_huddles_history` (research §1 item shape → `PastHuddle`, `missed` from `missed_participants` keys,
  `next_cursor`), `empty_lists`.
- [ ] **Step 3:** run → FAIL; implement; run → PASS (`cargo test`).
- [ ] **Step 4: Commit** "slack-core: live and recent huddles (Flannel huddles/list+info, huddles.history) (VW6)" +
  PROGRESS row.

### Task 7 (VW7): The Huddles view and the sidebar Huddles row with its badge

**Files:**
- Create: `crates/klack/src/tui/huddles.rs` (the view: `Huddles` state, layout, row hit-testing)
- Modify: EL (`View::Huddles`, fetches, RTM effects), `sidebar.rs` (Huddles row + badge), `live.rs`
  (`ended_huddle_update`, `huddle_start` arms; reconnect hook), `keymap.rs` (global `Huddles` action, a free key — e.g.
  `<leader> h l` plus Ctrl+H if free; check the table), `state.rs` (live list on `Workspace`).
- Test: huddles.rs, sidebar.rs, live.rs.

**Interfaces:**
- Consumes: `Client::huddles_live`, `Client::huddles_history`, `LiveHuddle`, `PastHuddle` (VW6); `ws.huddles`
  (`Huddle{room, participants, thread}`, state.rs:155) kept by `sh_room_*` (live.rs:157); `Workspace::status_of` (VW2).
- Produces: `Workspace::joinable_live(&self, me_in: Option<&str>) -> Vec<&str>` (channel ids: member, not muted, ≥1
  participant, not `me_in`); `sidebar` Huddles row badge = `joinable_live(..).len()` (hidden at 0);
  `huddles::layout(ws, live, past, width, now) -> (Vec<Line<'static>>, Vec<Option<Row>>)` with
  `enum Row { Live(String /*channel*/), Past(usize) }`.

- [ ] **Step 1: Failing tests**
  - state.rs `joinable_live_rules`: member+unmuted+2 people → counted; muted → not; not a member → not; 0 participants →
    not; the one I'm in → not.
  - live.rs `ended_huddle_clears_live`: `sh_room_update` with `has_ended: true` / `ended_huddle_update` → the channel
    leaves the live list; badge count drops.
  - huddles.rs `live_first_then_recent`: one live + two past → "Live now" header, the live row (marked live, duration,
    people with status emoji), "Recent" header, past rows "N ago · duration", a missed one in `p.danger` reading "Missed
    huddle", "N replies" when known.
  - sidebar.rs `huddles_row_badge`: 2 joinable → the row shows the badge "2"; 0 → no badge.
- [ ] **Step 2:** run → FAIL.
- [ ] **Step 3:** Implement. On workspace start and on every RTM reconnect fetch `huddles_live` into the workspace's live
  list (merged with `ws.huddles` from RTM); `huddles_history` when the view opens (pages of 30 on scroll). Enter on a live
  row opens its conversation (joining stays the existing Huddle key); on a past row opens its thread (`thread_ts`) in its
  conversation. Add the Huddles row to the sidebar (after Activity) and `View::Huddles`.
- [ ] **Step 4:** `cargo test` → PASS. Live: B in a test huddle (synthetic media) in a channel A is in → A's Huddles row
  shows 1, the view lists it live with B; B leaves → it moves to Recent, the badge clears; a missed DM huddle shows red.
- [ ] **Step 5: Commit** "klack: the Huddles view and a Huddles row with live huddles counted (VW7)" + README +
  PROGRESS row.

### Task 8 (VW8): Keep your own "in a huddle" state alive

**Files:**
- Modify: `crates/klack/src/tui/call.rs` or EL (heartbeat next to `alone_tick`/`ring_tick`, EL ~3716-3737),
  `crates/slack-core/src/api.rs` (a `users.profile.set` helper for the huddle fields), `docs/protocol/huddles.md`.
- Test: call.rs / EL unit test on the timing; api.rs request shape.

**Interfaces:**
- Produces: `Client::set_huddle_state(&self, fields: &serde_json::Value) -> Result<()>` (`users.profile.set` with only the
  fields the official client sends); `App.huddle_beat: Option<Instant>`.

- [ ] **Step 1: Read the client code** (`~/projects/slack-rea`, fresh bundles per `tools/README.md`): the pref gate
  (`huddles_set_status`), exactly which profile fields it sends on join, every ~240 s, and on leave. Live on rslack-testbed:
  B joins a klack test huddle; A reads B's profile shape (`huddle_state`, expiry present?) to learn whether the server sets
  `in_a_huddle` itself. Write the result in `docs/protocol/huddles.md`. Classifier blocks → stop and report.
- [ ] **Step 2: Failing test** `heartbeat_timing`: connected call, pref on → a beat at join, then after 240 s, not before;
  pref off → never; call ended → the leave action once (as the client does), then nothing.
- [ ] **Step 3:** run → FAIL; implement (send only the fields found in Step 1; never status emoji/text); run → PASS.
- [ ] **Step 4:** Live: B in a klack huddle for > 6 minutes (synthetic media) → A keeps seeing B "in a huddle" (VW2's
  marker) throughout; after B leaves it goes away.
- [ ] **Step 5: Commit** "klack: your 'in a huddle' state stays alive while you are in a huddle (VW8)" + README + PROGRESS
  row.

### Task 9 (VW9): Docs and the old popups' leftovers

**Files:** README.md (views, keys, statuses, Huddles, settings), PROGRESS.md (NEXT UP, Tests line), keymap which-key
texts, F6 help, i18n `locales/*.toml` template (new strings via the existing template command `klack ctl
locale-template`), remove dead popup-only code left after VW3–VW5.

- [ ] **Step 1:** `cargo build` with no warnings; grep for popup leftovers (`clear_modal` uses for threads/activity,
  `Option<Threads>`); remove.
- [ ] **Step 2:** README + help + locale template updated; `cargo test` green.
- [ ] **Step 3: Commit** "docs: Threads, Activity and Huddles views, statuses (VW9)" + PROGRESS row.

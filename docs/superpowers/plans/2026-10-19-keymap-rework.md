# Keymap rework (queue U) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** NORMAL / INSERT / DIALOG modes, a configurable `<leader>` tree with a which-key panel, an nvim-like status line, and a `keys.toml` the user can rewrite freely — so no single key in NORMAL acts on anything and every action is discoverable.

**Architecture:** `keymap.rs` stays the single source of bindings; it learns modes, two leaders (stored as a placeholder key), context precedence, groups and per-action labels/icons, and exposes `resolve` (dispatch) and `children` (which-key, help, F6). The event loop dispatches by mode first: DIALOG input goes only to the topmost dialog, NORMAL/INSERT to the main screen. New modules: `which_key.rs` (layout + drawing), `keys_file.rs` (format-preserving `keys.toml` edits and the template), `status.rs` (status line layout).

**Tech Stack:** Rust, ratatui 0.30, crossterm 0.29, `toml` 1.1, new direct dependency `toml_edit` 0.25 (already in `Cargo.lock`), `unicode-width`.

**Spec:** `docs/superpowers/specs/2026-10-19-keymap-rework-design.md` — read it first; this plan argues from it and does not repeat its tables (the leader tree, the shrink order, the file format).

## Global Constraints

- Each task = one step of queue U, done the project's way: `cargo build` without warnings → `cargo test` green (never commit on red) → live check in the kitty test window (`tools/launch.sh`, `tools/shot.sh`, `kitty @ --to unix:@klack-test …`; quit the test klack with Ctrl+Q before closing its window) → a PROGRESS.md row (LIVE/UNIT/NOT honest, "(this commit)") → commit with the `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>` trailer → a `docs: PROGRESS for U<n>` commit with the sha → `git push` (routine, no asking) → watch CI.
- No `cargo fmt`. Match the surrounding code's comment density and naming.
- Never rebuild `target/release` (the user runs it); live tests use the debug build.
- `~/.config/klack/keys.toml` and `settings.toml` are the user's: back them up before a live test that writes them, restore after; never delete.
- Defaults: `leader = "space"`, `insert_leader = "ctrl+space"` on every platform; which-key delay **0.4 s**, corner **bottom-right**, max height **50 %** of the screen, icons on.
- Times in settings are seconds.
- Default icons are Emoji_Presentation characters (no VS16), drawn in a fixed 2-cell column.
- Existing `keys.toml` action names keep working; the user's binding always beats a default.

## Review Focus

- A v0.5.0 `keys.toml` (`leader = "ctrl+b"`, `[messages] react = "r"`, `[global] huddle = "f3"`) loads with no lost binding and only informative warnings — test in Task 2.
- Space while typing (INSERT), including with the completion list open, types a space and never starts a sequence — test in Task 2 (`resolve`) and Task 3 (live).
- A pending sequence when the mode changes underneath (a dialog opens from a net event, a notification click, Ctrl+Q prompts) is dropped, never completed in the new mode — test in Task 3.
- Tiny terminals: status line at 10 columns, which-key on a 6-row screen — no panic, sequence still shown, panel pages — tests in Tasks 4 and 5.
- A hand-written `keys.toml` with comments, or with a TOML syntax error, touched from F6: comments survive; a file that doesn't parse is never overwritten (F6 says so) — tests in Task 9.

---

### Task 1 (U1): DIALOG mode — dialogs own all input

**Files:**
- Modify: `crates/klack/src/tui/keymap.rs` (add `Mode`)
- Modify: `crates/klack/src/tui/event_loop.rs:1845-1906` (`active_contexts`), `:2715-3331` (`handle_event`), `:4001` (`draw_confirm` caller)
- Modify: dialogs lacking a drawn-area field (`switcher.rs`, `search.rs`, `details.rs`, `profile.rs`, `activity.rs`, `threads.rs`, `sections.rs`, `help.rs`, `prefs.rs`, `viewer.rs`, `snippet.rs`, `video.rs`/player) — add `pub area: Rect` set in `draw`

**Interfaces:**
- Produces: `pub enum Mode { Normal, Insert, Dialog }` in `keymap.rs` (Copy, Eq, Debug).
- Produces: `App::mode(&self) -> Mode` — Dialog when `active_contexts()` has an overlay, Insert when focus is `Composer`/`ThreadComposer`, else Normal.
- Produces: `App::dialog_area(&self) -> Option<Rect>` — the topmost dialog's drawn area.
- Produces: `fn handle_dialog(app: &mut App, ev: &Event) -> bool` and `fn handle_main(app: &mut App, ev: Event) -> bool` in `event_loop.rs`; `handle_event` = Quit check, then by `app.mode()`.
- Produces: `fn run_global(app: &mut App, action: Action) -> bool` — every `Context::Global` action's effect (today spread over `handle_event` arms: F-keys, views, workspaces, focus, scroll), extracted so `handle_main` (now) and `handle_dialog` (Task 2) share it; returns false for Quit.

- [ ] **Step 1: Write failing tests** in `event_loop.rs` tests: `confirm_prompt_names_the_bound_key` — `confirm_question("delete this message?", &Keymap::load("[confirm]\nyes = \"j\"\n").0)` ends with `"j / n"`, and with defaults ends with `"y / n"`.
- [ ] **Step 2: Run** `cargo test -p klack confirm_prompt` — expect FAIL (no `confirm_question`).
- [ ] **Step 3: Implement.** `fn confirm_question(q: &str, keys: &Keymap) -> String` using `first_key(Context::Confirm, Action::Yes)`; use it where the "… y / n" text is built (~3841). Move every dialog arm of `handle_event` (menu, help, prefs capture/picker/text/prefs, switcher, mover, save, activity, threads, profile, details, search, player, snippet, viewer, confirm, picker, and the picker/menu/save mouse arms) into `handle_dialog`, unchanged inside; it never consults `Context::Global` except `Quit`. Mouse in DIALOG: menu/picker/save keep their handlers; for the others a wheel goes to `wheel_in_popup`, a click outside `dialog_area()` closes the topmost dialog, a click inside does nothing. `handle_main` keeps today's global/pane/mouse arms (F6, F1, Ctrl+K/F/T, F-keys, panes); global views no longer open over dialogs because dialogs never reach it. Completion stays in `handle_main` (it is part of INSERT).
- [ ] **Step 4: Run** `cargo test -p klack` — PASS; `cargo build` — no warnings.
- [ ] **Step 5: Live check:** Ctrl+F then F3 → nothing happens (no huddle); Ctrl+K then a click on a sidebar row → the switcher closes, the selection doesn't move; right-click with Details open → no menu underneath; delete prompt shows `y / n`.
- [ ] **Step 6: Close** (PROGRESS row U1, commit `feat: DIALOG mode — dialogs own keys and clicks (U1)`, docs commit, push).

### Task 2 (U2): Keymap engine — two leaders, precedence, the user wins

**Files:**
- Modify: `crates/klack/src/tui/keymap.rs` (`Keymap`, `load`, `parse_sequences`, `lookup` → `resolve`, tests)
- Modify: `crates/klack/src/tui/event_loop.rs:1908-1938` (`hold_sequence`, `action_in` use `resolve`)

**Interfaces:**
- Consumes: `Mode` (Task 1).
- Produces: `pub const LEADER: Key` — a placeholder (`KeyCode::Null`, no mods) that `<leader>` parses to; `Key::label` of it is the leader's label at draw time via `Keymap::show(&[Key]) -> String`.
- Produces: `Keymap { pub leader: Option<Key>, pub insert_leader: Option<Key>, … }`; defaults `space` / `ctrl+space` when `keys.toml` doesn't set them; `insert_leader = ""` → `None`.
- Produces: `pub enum Resolved { Action(Context, Action), Pending, None }` and `Keymap::resolve(&self, contexts: &[Context], mode: Mode, pending: &[Key], key: Key) -> Resolved` — `contexts` most specific first (dialog, or pane then `Global`). A pressed key stands for `LEADER` when it equals `insert_leader`, or equals `leader` and (`mode == Normal` or the leader isn't text). First context with an Action wins; Pending if any context has a longer match. In Insert/Dialog a `Global` sequence whose first key is text is skipped; in Dialog only `Global` sequences starting with `LEADER`, and `quit`, count.
- Produces: `pub struct Pending` (`push(&mut self, key: Key, mode: Mode)`, `pop(&mut self)`, `clear(&mut self)`, `keys(&self) -> &[Key]`, `sync(&mut self, mode: Mode)` — clears when the mode differs from the one the sequence started in); `App.pending` becomes this type.
- Produces: `Keymap::show(&self, seq: &[Key]) -> String` — short form for status/which-key/menus: Space `␣`, Ctrl+X `^X`, Ctrl+Space `^␣`, Alt+x `M-x`, others as `Key::label`, no separators (`␣h`, `^␣hm`).

- [ ] **Step 1: Write failing tests** in `keymap.rs`:
  - `leaders_default_and_stand_for_leader`: `Keymap::load("[global]\nmute = \"<leader> h m\"\n")`: `resolve(&[Messages, Global], Normal, &[], space)` = Pending; `resolve(&[Composer, Global], Insert, &[], space)` = None (types); `resolve(&[Completion, Composer, Global], Insert, &[], space)` = None; `resolve(&[Composer, Global], Insert, &[], ctrl+space)` = Pending; `resolve(&[Search, Global], Dialog, &[], space)` = None (Space belongs to the dialog); `resolve(&[Search, Global], Dialog, &[LEADER, h], m)` = `Action(Global, Mute)`.
  - `pending_drops_on_mode_change`: `let mut p = Pending::default(); p.push(LEADER, Mode::Normal); p.sync(Mode::Dialog); assert!(p.keys().is_empty())`; `sync` with the same mode keeps the keys.
  - `pane_beats_global`: `load("[global]\nactivity = \"ctrl+a\"\n")`, `resolve(&[Composer, Global], Insert, &[], ctrl+a)` = `Action(Composer, LineStart)`.
  - `printable_global_keys_act_only_in_normal`: `load("[global]\nmute = \"x\"\n")` gives no warning; Normal on `&[Messages, Global]` → `Action(Global, Mute)`; Insert on `&[Composer, Global]` → None; `[composer] send = "x"` still warns "would be typed".
  - `user_binding_beats_default_prefix`: with a test default table containing `<leader> h` group sequences (use current defaults plus `load("[global]\nmute = \"<leader> h m\"\nhuddle = \"<leader> h\"\n")`): `huddle` wins on `␣h`, `mute` is dropped, a warning names `mute`; a v0.5.0 file `leader = "ctrl+b"\n[messages]\nreact = "r"\n[global]\nhuddle = "f3"\n` loads with `warnings` free of "keeping the default" and those keys resolve.
  - `show_is_short`: `show(&[LEADER, k('h')])` = `"␣h"`; with `leader = "ctrl+b"` it is `"^Bh"`.
- [ ] **Step 2: Run** `cargo test -p klack keymap::` — FAIL.
- [ ] **Step 3: Implement** the above. In `load`, bind user sequences first, then defaults; a default that equals, or is a prefix of / prefixed by, a user sequence in the same context is dropped with `keys.toml: [ctx] default <seq> of <action> gives way to <user action>`. Two user sequences colliding: first in table order keeps it (today's message). Drop the "shadowed by a [global] binding" warning; replace with "[global] <seq> is hidden by [<ctx>] <action>" only for user-written sequences. `types_text()` no longer includes `Global`. `hold_sequence`/`action_in` call `resolve` with `self.mode()` and the contexts list (pane first) and call `pending.sync(self.mode())` before every key and every draw. `handle_dialog`: when `resolve` gives `Action(Global, a)` (a leader sequence or Quit), call `run_global(app, a)`.
- [ ] **Step 4: Run** `cargo test -p klack` — PASS (update older keymap tests that assumed global-first or `types_text(Global)`; keep their intent).
- [ ] **Step 5: Live check:** typing a space in the composer works (also with `@` suggestions open); with a scratch `keys.toml` (back up the user's) `[global] mute = "x"` → `x` in the sidebar says "not in a huddle"-style result or toggles nothing harmful, `x` in the composer types.
- [ ] **Step 6: Close** (U2).

### Task 3 (U3): NORMAL and INSERT behave as designed

**Files:**
- Modify: `crates/klack/src/tui/keymap.rs` (defaults: `sidebar.open` = `["enter", "i"]`, `messages.back` stays `esc` meaning close-thread only; new `HalfPageUp`/`HalfPageDown` actions `ctrl+u`/`ctrl+d` in `sidebar` and `messages`)
- Modify: `crates/klack/src/tui/event_loop.rs` (`message_action` `Back`/`Type`, `KeyOutcome::Leave`, `sidebar_action`, `mode_label`, `draw_composer` border)

**Interfaces:**
- Consumes: `Mode`, `App::mode` (Task 1).
- Produces: `Action::HalfPageUp`, `Action::HalfPageDown` (names `half_page_up`, `half_page_down`).

- [ ] **Step 1: Write failing test** in `keymap.rs`: `normal_insert_defaults` — `resolve(&[Sidebar, Global], Normal, &[], esc)` = None; `resolve(&[Messages, Global], Normal, &[], ctrl+d)` = `Action(Messages, HalfPageDown)`; `resolve(&[Sidebar, Global], Normal, &[], i)` = `Action(Sidebar, Open)`.
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement.** Composer `Leave` (Esc, not while editing — editing's first Esc still cancels the edit) → focus `Messages` with the newest shown message selected (reply box → `Thread`, newest reply). `Back` in the messages pane does nothing; in the thread pane closes it. `Type` (`i`) → the composer of the pane (thread → reply box). Half page: messages/thread scroll by half the pane height (like `ScrollDown`, halved) and move the selection with it; sidebar moves the selection by half its height. Mode pill text: `NORMAL` for Sidebar/Messages/Thread/Huddle focus, `INSERT` for both composers (editing too), dialogs keep their names; the composer border uses the INSERT colour (`p.good`) while focused.
- [ ] **Step 4: Run** `cargo test -p klack` — PASS.
- [ ] **Step 5: Live check:** Esc in the composer → messages pane, newest selected, pill NORMAL; `i` → INSERT; Esc in the sidebar does nothing; Ctrl+D/Ctrl+U page half; Esc in a reply box → thread pane; a right-click menu opened while `␣` is pending → the pending sequence is gone (Review Focus 3; unit-tested in Task 2).
- [ ] **Step 6: Close** (U3).

### Task 4 (U4): The status line

**Files:**
- Create: `crates/klack/src/tui/status.rs` (pure layout + tests); register in `tui/mod.rs`
- Modify: `crates/klack/src/tui/event_loop.rs:3858-3931` (build inputs), remove `status_line` (`:3969`) and move its test `status_bar_fits` into `status.rs`
- Modify: `crates/klack/src/tui/keymap.rs` `hints_for` (display order, least useful first) and `context_hints` → returns `Vec<(String, String)>` (key, word)

**Interfaces:**
- Produces: `pub enum Pill { Normal, Insert, Dialog(&'static str) }` — colours: Normal `p.accent`, Insert `p.good`, Dialog a third palette colour (`p.warn` or the theme's nearest).
- Produces: `pub struct Info { pub unread: usize, pub mentions: usize, pub others: String, pub availability: String, pub live: bool, pub note: Option<String> }` — `note` is a status message or the ringing text.
- Produces: `pub fn line(pill: Pill, info: &Info, showcmd: &str, hints: &[(String, String)], width: u16) -> Line<'static>`.

- [ ] **Step 1: Write failing tests** in `status.rs` (plain-text rendering helper), with `info = 3 unread, 1 mention, live`, `showcmd = "␣"`, hints `[("↑/↓","move"),("Enter","thread"),("i","type"),("␣","actions"),("?","help")]`:
  - width 110: `" NORMAL  3 unread · 1 mention · ● live"` at the left, ends with `"␣ ↑/↓ move · Enter thread · i type · ␣ actions · ? help "`-style right part flush right (assert `ends_with("? help ")`, `starts_with(" NORMAL ")`).
  - width 80: info reads `3 · @1 ●`, all hints still there.
  - width 60: `↑/↓ move` and `Enter thread` gone, `i type · ␣ actions · ? help` kept.
  - width 40: pill is `" N "`.
  - width 24: hints `?`, info `●`, showcmd `␣` present.
  - width 10: no panic, contains `␣`.
  - a note (`"copied a link to the message"`) at 60: replaces info, hints shrink before the note, note ends with `…` if still too long.
  - every result's display width == `width`.
- [ ] **Step 2: Run** `cargo test -p klack status::` — FAIL.
- [ ] **Step 3: Implement** `line` with the spec's shrink order: (1) short info, (2) drop hints from the front one by one (keep the last), (3) pill to its first letter, (4) last hint to its key only, info to `●`; the showcmd is never cut; a note outranks hints and is cut with `…` last. The conversation crumb is gone; availability and `● live` are in the info; `F1 keys ·` is gone. NORMAL hints come from `hints_for` (`Messages`: up/down "move", OpenThread "thread", Type "type", leader "actions", Help "help"; `Sidebar`: move, open, type, actions, help; INSERT (`Composer`): Newline "new line", Send "send", Leave "normal"); "move" shows the first keys of Up and Down joined by `/`; the leader hint uses `show(&[LEADER])`. Ringing note: `📞 X invites you to a huddle · <keys of huddle> join · <keys of decline_huddle> decline` with `show`.
- [ ] **Step 4: Run** `cargo test -p klack` — PASS.
- [ ] **Step 5: Live check:** shots at a wide window and after `kitty @ resize-window` narrow (or a small kitty window): order of collapse as in the mockup `status-widths`.
- [ ] **Step 6: Close** (U4).

### Task 5 (U5): which-key panel

**Files:**
- Create: `crates/klack/src/tui/which_key.rs`; register in `tui/mod.rs`
- Modify: `crates/klack/src/tui/keymap.rs` (`ActionInfo` gets `label` and `icon`; `actions!` rows get two more literals; `GROUPS`; `children`)
- Modify: `crates/klack/src/tui/settings.rs` (`WhichKey` section), `crates/klack/src/tui/event_loop.rs` (state, timer arm in the `select!` ~2623, keys while pending, draw)

**Interfaces:**
- Produces in `keymap.rs`: `pub struct Child { pub key: Key, pub label: String, pub icon: String, pub group: bool, pub action: Option<(Context, Action)> }` and `Keymap::children(&self, contexts: &[Context], mode: Mode, prefix: &[Key]) -> Vec<Child>` — the next keys after `prefix`, actions first in table order then groups by key, deduplicated (more specific context wins); group label from `[groups]`/`GROUPS`, else `+<key>`.
- Produces: `pub const GROUPS: &[(&str, &str, &str)]` — `("<leader> c", "conversation", "💬")`, `h huddle 🎧`, `n new ✨`, `a availability 🟢`, `f find 🔎`, `w workspace 🏢`.
- Produces in `settings.rs`: `pub struct WhichKey { pub show: bool, pub corner: String /* "bottom-right" | "bottom-left" | "top-right" | "top-left" */, pub delay_secs: f32, pub max_height_percent: u16, pub icons: bool }`, `Settings.which_key`, defaults `true, "bottom-right", 0.4, 50, true`.
- Produces in `which_key.rs`: `pub struct Panel { pub page: usize }`, `pub fn layout(children: &[Child], screen: Rect, max_height_percent: u16, icons: bool) -> Layout` with `pub struct Layout { pub columns: usize, pub rows: usize, pub pages: usize, pub width: u16 }`, and `pub fn draw(f: &mut Frame, screen: Rect, title: &str, children: &[Child], panel: &Panel, settings: &WhichKey)`.
- `App` gains `pending_since: Option<Instant>`, `which_key: Option<Panel>` (Some once shown).

- [ ] **Step 1: Write failing tests** in `which_key.rs`:
  - `columns_align_by_display_width`: children with icons `😀`/`🧵` and labels of different length → every row's key column starts at the same cell (render to a `TestBackend` buffer and check the `→` column).
  - `grows_columns_then_pages`: 18 children on an 80×40 screen → 2 columns, 1 page; on an 80×10 screen (50 % = 5 rows, minus title/footer) → pages > 1 and the footer contains `1/` and `Ctrl+D`.
  - `six_row_screen_does_not_panic` (Review Focus 4).
  - `icons_off_drops_the_column`.
  - `long_labels_are_cut` with `…` at the column width.
  In `keymap.rs`: `children_lists_actions_then_groups` — on `load("[messages]\nreact = \"<leader> r\"\n[global]\nmute = \"<leader> h m\"\n")`, `children(&[Messages, Global], Normal, &[LEADER])` is `[r → react (action), h → huddle (group, from GROUPS)]`; `children(…, &[LEADER, h])` is `[m → mute]`; `[groups]` `"<leader> h" = { name = "calls", icon = "📞" }` renames it; a prefix with no `[groups]`/`GROUPS` entry is labelled `+<key>`.
- [ ] **Step 2: Run** `cargo test -p klack which_key` — FAIL.
- [ ] **Step 3: Implement.** Labels/icons: every `actions!` row gets a short label and an emoji (Emoji_Presentation only); `[labels.<context>] action = "label" | { label, icon }` and `[groups] "<seq>" = "name" | { name, icon }` parsed in `load` (unknown context/action → warning). The panel opens when `pending` is non-empty and `pending_since + delay_secs` has passed (add a `select!` arm sleeping until then when `which_key.is_none()`; delay 0 opens at once); while pending, `Ctrl+D`/`Ctrl+U` turn pages, `Esc` clears `pending`, `Backspace` pops one key; any other key goes through `resolve` as usual; `which_key = None` when `pending` empties. Placement: the chosen corner of the body area (above the status line), clear underneath (`media::clear_modal`). Title: `show(prefix)` + group name. Footer: `Esc close · Backspace back` (and paging when pages > 1). Labels and group names go through `i18n::tr`; add them to the locale template words.
- [ ] **Step 4: Run** `cargo test -p klack` — PASS.
- [ ] **Step 5: Live check:** with a scratch `keys.toml` binding a few `<leader>` sequences: `Space` → after ~0.4 s the panel bottom-right; `h` swaps at once; Backspace back; a short window pages with Ctrl+D; `which_key.show = false` in settings → no panel, the status line still shows `␣h`.
- [ ] **Step 6: Close** (U5).

### Task 6 (U6): The default tree — actions that had no key

**Files:**
- Modify: `crates/klack/src/tui/keymap.rs` (new actions + defaults), `crates/klack/src/tui/event_loop.rs` (handlers), `crates/klack/src/tui/composer.rs` (`set_with_cursor`)

**Interfaces:**
- Produces new actions (Global unless said; `keys.toml` names in quotes; `Details`, `Star`, `MoveToSection` are existing variants that gain Global entries): `Details "details"`, `Star "star"`, `MoveToSection "move_to_section"`, `NotificationLevel "notification_level"`, `LeaveConversation "leave"`, `JoinConversation "join"`, `RenameSection "rename_section"`, `DeleteSection "delete_section"`, `NewChannel "new_channel"`, `NewPrivateChannel "new_private_channel"`, `NewDm "new_dm"`, `NewSnippet "new_snippet"`, `Upload "upload"`, `SetStatus "set_status"`, `ClearStatus "clear_status"`, `AwayActive "away"`, `PauseNotifications "pause_notifications"`, `BrowseChannels "browse_channels"`; Sidebar: `Details`, `NotificationLevel`, `LeaveConversation` beside existing `Star`/`MoveToSection`.
- Produces: `Composer::set_with_cursor(&mut self, text: &str, cursor: usize)`; `App::prefill(&mut self, command: &str, wrap: Wrap) ` with `enum Wrap { No, Caption, Body }`; `App::cycle_conversation_level(&mut self)` (extracted from `change_preference`'s `ConversationLevel` branch).

- [ ] **Step 1: Write failing tests:** in `keymap.rs` `conversation_new_availability_find_groups` — `resolve(&[Messages, Global], Normal, &[LEADER, c], b)` = `Action(Global, NotificationLevel)`, `[LEADER, n] f` = `Upload`, `[LEADER, a] a` = `AwayActive`, `[LEADER, f] '#'` = `BrowseChannels`, `[LEADER, w] '3'` = `Workspace3`, `[LEADER] ' '` = `Switcher`, `resolve(&[Sidebar, Global], Normal, &[LEADER], s)` = `Action(Sidebar, Star)`, `resolve(&[Sidebar, Global], Normal, &[], z)` = `Action(Sidebar, FoldSection)`, `resolve(&[Sidebar, Global], Normal, &[], space)` = Pending; `Keymap::load("")` has no warnings. In `event_loop.rs` `prefill_keeps_drafts` on a `Composer`: empty + `/create ` → text `/create `, cursor 8; `"hello"` + Caption `/upload ` → `"/upload  hello"`, cursor 8; `"line"` + Body `/snippet ` → `"/snippet \nline"`, cursor 9; `"hello"` + No → unchanged and `Err("the message box is in use")`; editing → `Err`. (Make the composer part a pure `fn prefilled(text: &str, editing: bool, command: &str, wrap: Wrap) -> Result<(String, usize), &'static str>`.)
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement** handlers per the spec's tree: `c d` details, `c s` star current, `c m` mover for current, `c b` `cycle_conversation_level` + note naming the new level, `c l` confirm leave (refuse in a DM with a note), `c j` join current (refuse when a member), `c r` prefill `/section rename `, `c x` the `/section delete` confirm; `n c/p/d/s/f` prefill `/create `, `/create-private `, `/dm @`, `/snippet ` (Body), `/upload ` (Caption); `a s` prefill `/status `, `a c` run `/status clear`, `a a` away ↔ active from the current presence, `a p` prefill `/dnd `; `f f` switcher, `f #` switcher with query `#`, `f s` search, `f t` threads, `f a` activity; `w 1…9` workspaces (extra sequence beside Alt+n); `␣␣` switcher. Prefill always targets the channel composer and focuses it (INSERT). Remove Space from `sidebar.fold_section` (`["z"]`) and from `huddle_view.maximize` (`["enter", "f"]`). Sidebar `␣d/␣b/␣l` act on the selected conversation (it is current).
- [ ] **Step 4: Run** `cargo test -p klack` — PASS.
- [ ] **Step 5: Live check** in the test workspace (DM D0CA2V8G8CE): `␣cb` cycles and says the level (restore it after), `␣nf` with a draft → `/upload  <draft>` cursor after `/upload `, `␣as` → `/status `, `␣fs` search, `␣w2` (if the profile has one, else the "there is no workspace 2" note), which-key shows the groups. Restore any Slack-side state changed.
- [ ] **Step 6: Close** (U6).

### Task 7 (U7): The default tree — existing actions move behind the leader

**Files:**
- Modify: `crates/klack/src/tui/keymap.rs` (defaults), `crates/klack/src/tui/menu.rs` (key column via `show`), `crates/klack/src/tui/event_loop.rs` (profile `Huddle`/`Vip`, `Offers` filter for which-key)

**Interfaces:**
- Consumes: `children` (Task 5), `Offers::of` (`menu.rs`).
- Produces: `App::applies(&self, context: Context, action: Action) -> bool` — false for edit/delete on others' messages, open without things, save without files, copy without text, join when a member, leave in a DM, `open_thread` in the thread pane, `huddle` (profile) off a profile card; which-key filters `children` with it.

- [ ] **Step 1: Write failing test** in `keymap.rs` `defaults_follow_the_tree`: Messages `[LEADER] r/e/d/y/Y/o/s/p/u/t/m` → React/Edit/Delete/Copy/CopyLink/Open/Save/Pin/Profile/OpenThread/ContextMenu; `resolve(&[Messages, Global], Normal, &[], r)` = None; Global `[LEADER, h] h/d/m/c/s/v/t` → Huddle/DeclineHuddle/Mute/Camera/ShareScreen/HuddleView/HuddleThread; Messages `[LEADER, h] T` → ThreadHuddle; Profile `[LEADER, h] p` → Huddle and `[LEADER] v` → Vip; Global `ctrl+a` → Activity; `f2,f3,f4,f5,f7,f8,f9,f10,shift+f3,shift+f10,menu` resolve to None everywhere; every action still has a key (keep the existing "every action is reachable" assertion). Update the older default tests (`defaults_are_today_s_keys_and_conflict_free`, `hints_follow_context_and_bindings`, `help_follows_the_bindings`) to the new defaults.
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement** the defaults, `applies`, menus showing `show(seq)` for the first sequence. In dialogs (the profile card included) the leader is `insert_leader` (Ctrl+Space), as the spec says; Space keeps whatever the dialog gives it.
- [ ] **Step 4: Run** `cargo test -p klack` — PASS.
- [ ] **Step 5: Live check:** `r` alone does nothing in NORMAL; `␣r` opens the picker; `␣hh` joins/leaves a huddle in the A↔B DM (no real mic: muted join, leave at once); Ctrl+Space h m while typing toggles mute in a call; right-click menu shows `␣r` etc.; which-key hides edit/delete on B's messages. Delete any test posts.
- [ ] **Step 6: Close** (U7).

### Task 8 (U8): F1 help and the shared tree

**Files:**
- Modify: `crates/klack/src/tui/keymap.rs` (`tree`), `crates/klack/src/tui/help.rs`

**Interfaces:**
- Produces: `pub struct Section { pub title: String, pub rows: Vec<TreeRow> }`, `pub struct TreeRow { pub info: usize /* index into ACTIONS */, pub keys: String, pub label: String, pub icon: String }`, `pub fn tree(keys: &Keymap) -> Vec<Section>` — sections in order: "Direct keys" (Global + Sidebar/Messages/Huddle single-key bindings), "␣ on a message", "␣ on a conversation", one per group prefix (`␣c conversation`, …, user groups too), "Typing (INSERT)", each dialog's title, "Unbound". A row sits in the section of its first sequence.

- [ ] **Step 1: Write failing test** `help_follows_the_tree` in `help.rs`: titles in that order; `react` under "␣ on a message" with keys `␣r`; with `load("[messages]\nreact = \"r\"\n")` react moves to "Direct keys"; `pin = []` → under "Unbound"; workspaces still one row.
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement** `tree`; `help::lines` draws it (headings, `keys → icon label`, description dim). Help footer drops "rebind in keys.toml" for "F6 → Keys · klack ctl keys-template". Add the Ctrl+Space note to help: "Ctrl+Space: the leader while typing or in a dialog — if your system takes it (macOS input source, IME), rebind it in F6 → Keys".
- [ ] **Step 4: Run** `cargo test -p klack` — PASS.
- [ ] **Step 5: Live check:** F1 shows the sections; scrolls; translated headings come from the locale words.
- [ ] **Step 6: Close** (U8).

### Task 9 (U9): `keys.toml` kept intact; `klack ctl keys-template`

**Files:**
- Create: `crates/klack/src/tui/keys_file.rs`; register in `tui/mod.rs`
- Modify: `crates/klack/Cargo.toml` (`toml_edit = "0.25"`), `crates/klack/src/tui/keymap.rs` (remove `to_toml`), `crates/klack/src/tui/event_loop.rs:1547-1588` (`capture_key`, `save_keys`, Unbind/Reset arms), `crates/klack/src/ctl.rs` (`KeysTemplate`)

**Interfaces:**
- Produces: `pub enum Change { Binding { context: Context, name: &'static str, seqs: Option<Vec<String>> /* None = back to default */ }, Leader(Option<String>), InsertLeader(Option<String>), Group { prefix: String, name: Option<String>, icon: Option<String> } }`.
- Produces: `pub fn apply(text: &str, change: &Change) -> Result<String, String>` (Err when `text` doesn't parse: nothing is written) and `pub fn template(keys: &Keymap) -> String` (from `Keymap::load("")`, sections from `tree`, each action `name = ["…"]  # description`, `leader`/`insert_leader` and `[groups]` on top, commented).
- Produces: `Ctl::KeysTemplate` → `klack ctl keys-template` prints `template`.

- [ ] **Step 1: Write failing tests** in `keys_file.rs`:
  - `comments_and_order_survive`: text with a header comment, `[messages]\n# mine\nreact = "r"  # quick\n` + `apply(Binding{messages, "pin", Some(["P"])})` → result still contains `# mine` and `# quick`, `react` line unchanged, `pin = ["P"]` added under `[messages]`.
  - `reset_removes_the_entry`, `unbind_writes_empty_list`, `leader_and_group_edits` (`[groups]` `"<leader> h" = { name = "calls", icon = "📞" }`).
  - `broken_file_is_not_touched`: `apply("[messages\nreact=", …)` is `Err`.
  - `template_loads_clean`: `Keymap::load(&template(&Keymap::load("").0))` has no warnings and equals the defaults on every `(context, name)`; it contains `insert_leader = "ctrl+space"` and a comment line per section.
  - `creates_from_nothing`: `apply("", Binding{…})` → just that entry.
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement** with `toml_edit::DocumentMut`; sequences written in today's notation (`"<leader> h m"`, keys as `Key::spec`). `save_keys` reads the current file (missing → ""), `apply`, on Err shows `keys.toml has an error; fix it by hand: <e>` in the preferences and writes nothing.
- [ ] **Step 4: Run** `cargo test -p klack` — PASS; `cargo run -p klack -- ctl keys-template | head` shows the commented file.
- [ ] **Step 5: Live check:** back up the user's `keys.toml`; write a commented one; rebind one action in F6; `diff` shows only that line changed; restore.
- [ ] **Step 6: Close** (U9).

### Task 10 (U10): F6 → Keys by the tree, which-key settings, recording, group names

**Files:**
- Modify: `crates/klack/src/tui/prefs.rs` (Keys category fields, filter, recording state), `crates/klack/src/tui/event_loop.rs` (`capture_key`, Preferences arms), `crates/klack/src/tui/keymap.rs` (Preferences action `edit_icon` "i")

**Interfaces:**
- Consumes: `tree` (Task 8), `keys_file::apply` / `Change` (Task 9), `WhichKey` (Task 5).
- Produces: `Field::{WhichKeyShow, WhichKeyCorner, WhichKeyDelay, WhichKeyHeight, WhichKeyIcons, Leader, InsertLeader, GroupHeader(usize), Binding(usize)}`; `Prefs.filter: String`, `Prefs.recording: Option<Recording>` with `pub struct Recording { pub keys: Vec<Key>, pub last: Instant, pub done: bool }`; `const RECORD_PAUSE: Duration = 1 s`.

- [ ] **Step 1: Write failing tests** in `prefs.rs`: `keys_category_follows_the_tree` (fields: the five which-key settings, the two leaders, then for each `tree` section a `GroupHeader` followed by its `Binding`s); `filter_narrows_bindings` (filter "huddle" keeps only rows whose label/description/keys match, headers of empty sections hidden); `recording_ends_after_a_pause` (`Recording` with keys `[space, h, m]`, `last` 1.1 s ago → `done`; Esc and Enter as keys are recorded, not acted on).
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement.** Typing a printable key in Keys (not bound in Preferences) goes to the filter; Backspace erases it when there is one. Enter on a binding starts `recording`; the event loop ends it after `RECORD_PAUSE` (timer arm like which-key's), shows `<keys> · Enter keep · Esc discard`, then Enter writes `Change::Binding` (the leader key at position 0 is written as `<leader>`), Esc discards. Enter on a group header edits its name (TextField), `i` its icon. The which-key settings cycle/adjust like other prefs (`+`/`-` for delay in 0.1 s steps and height in 10 % steps; corner cycles).
- [ ] **Step 4: Run** `cargo test -p klack` — PASS.
- [ ] **Step 5: Live check** (user's `keys.toml`/`settings.toml` backed up and restored): record `␣ x` for an action; filter "mute"; rename the huddle group and see it in which-key; move the panel to top-left; delay 0 → panel at once.
- [ ] **Step 6: Close** (U10).

### Task 11 (U11): Docs, platforms, release notes

**Files:**
- Modify: `README.md` (keys section: modes, leader, which-key, `keys-template`, Ctrl+Space note), `crates/klack/src/tui/help.rs` `COMMANDS` if needed, `PROGRESS.md` (Next: the follow-up queue for mute / mark unread / next unread / conversation link)

- [ ] **Step 1:** README: the new keys overview generated from `tree` wording (copy, not code), the habit-change note ("single letters and F-keys moved behind Space; `klack ctl keys-template` prints everything; put any action back on a key in keys.toml").
- [ ] **Step 2: Cross-platform check** on the Windows VM `win11-dev` and the Mac `macbook` (see the test-machines memory; clean up after): build, start, Ctrl+Space in the composer reaches klack or not — record the result per platform in PROGRESS; no default changes (user: same default everywhere).
- [ ] **Step 3:** PROGRESS: U done; add queue V (mute a conversation `␣cM`, mark unread `␣cu`, next unread conversation, copy a conversation link) as the user's follow-up, NEXT UP.
- [ ] **Step 4: Close** (U11; `docs:` commit, push, CI green). A release is the user's call — ask.

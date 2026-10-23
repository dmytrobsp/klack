# F6 help per item, guided sign-in (`auth`), profiles and `purge` (queue W) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan
> task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build queue W as designed and approved by the user on 2026-10-20: formatted help per F6 item, a guided
sign-in that lifts sessions from the user's browsers, `auth login|logout|status`, `profile list|default|remove`,
`purge`, and handling of expired sessions.

**Architecture:** F6 help is a new module `crates/klack/src/tui/prefs_help.rs` (content + layout) drawn by
`Prefs::draw`. The command line moves out of `main.rs`: `crates/klack/src/auth.rs` (login guide, logout, status,
session checks), `crates/klack/src/pick.rs` (an inline arrow-key picker and a text prompt for plain terminal
questions, ratatui inline viewport), profile/purge file logic in `crates/slack-core/src/profiles.rs` (pure over a
`Dirs` value so it is testable without touching real folders). Browser discovery grows in
`crates/klack/src/login_import.rs`; Slack's "which workspaces does this cookie cover" in
`crates/slack-core/src/session.rs`. The TUI learns about signed-out workspaces through a shared set passed into
`event_loop::run`.

**Tech Stack:** Rust 2024 (rustc 1.98: `std::fs::File::try_lock` is available), clap 4 derive, ratatui 0.30
(`Viewport::Inline`), crossterm 0.29, reqwest, rusqlite, directories 6. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-10-20-prefs-help-and-auth-design.md` — read it whole before your task; it
records who decided what ("(user)" decisions are not to be changed).

## Global Constraints

- The house step routine for every task: `cargo build` with no warnings → `cargo test` green (never commit on red:
  `cargo test 2>&1 | grep -q FAILED && exit 1`) → live check → a PROGRESS.md row in the table (after the last `| V21`
  row; format `| W<n> — what (user's words where they exist) | done | UNIT: …; LIVE: …; NOT: … | (this commit) |`,
  honest about what was not checked live) → ONE commit `feat|fix: … (W<n>)` ending with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. The controller does the "docs: PROGRESS for W<n>" sha
  commit and pushes.
- Never print tokens or cookies (logs and test output included); fixtures use made-up ids (`T0000000001`,
  `U0000000001`), never the test workspace's or anyone's email.
- The user's real data is never touched: `~/.config/klack/test` (their work workspace) is never signed in/out,
  removed or read for tokens; `settings.toml` / `keys.toml` are backed up and restored if a live check could write
  them; real drafts stay. **Every live check of `auth`, `profile`, `purge` and the guide runs under a fake home:**
  `F=<scratchpad>/fakehome; env HOME=$F XDG_CONFIG_HOME=$F/.config XDG_CACHE_HOME=$F/.cache
  XDG_DATA_HOME=$F/.local/share ./target/debug/klack …`. A test session there is made by copying a test account's
  entry (python, never printed) from `~/.config/klack/default/session.json` (A) or `b/session.json` (B).
- Never rebuild `target/release` (the user runs it). Debug builds only. No `cargo fmt`. `pgrep -x`/`pkill -x` only,
  never `pkill klack`. Quit the test klack with Ctrl+Q before closing its kitty window.
- Real browsers: discovery may READ the user's real browser profiles only in the counts-only ignored test of W6. Never
  sign in with a session lifted from the user's real browsers; live guide checks use the fake home's synthetic
  Firefox profile (W6 builds it).
- `klack login` is removed outright (user): no alias, no pointer.
- Help texts and every new user-facing string go through `i18n::tr` and appear in `ctl locale-template`.

## Review Focus

- A browser whose key can't be read (locked keyring, macOS Keychain prompt over ssh, Windows Chrome) — the guide must
  neither hang nor abort: a timeout (10 s) and a dim heading with the reason. Test in W6.
- Ctrl+C or a terminal resize during the inline picker / text prompt — raw mode off, cursor shown, the picker's lines
  erased, nothing half-saved. Test in W5 (a guard type restoring the terminal on drop; unit-test the guard's restore
  is called on the Ctrl+C key path).
- Old `session.json` files (no `source`, one account per team, `default_team` set) and a profile holding two accounts
  of one team with `default_team` naming that team — load, start and `--team` behave. Tests in W3.
- A profile name with path characters (`../x`, `a/b`, `.`, empty, `default-profile`) — refused before any folder is
  created or removed (critical for `profile remove` / `purge`). Test in W3.
- A long list (more sign-ins than terminal rows) — the picker scrolls with the cursor, headings stay readable. Test in
  W5.

---

### Task 1 (W1): F6 help per item — the help box and every non-Keys item

**Files:**
- Create: `crates/klack/src/tui/prefs_help.rs` (declare in `crates/klack/src/tui/mod.rs`)
- Modify: `crates/klack/src/tui/prefs.rs` (`Prefs::draw`: remove the `fixed` / `hints` block ~lines 1112–1144 and draw
  the box; the locale list at ~line 581 gains `prefs_help::strings()`)

**Interfaces:**
- Produces:
  - `pub enum HelpLine { Text(String), Value { name: String, meaning: String }, Key { keys: String, does: String }, Blank }`
  - `pub struct Help { pub title: String, pub lines: Vec<HelpLine> }`
  - `pub fn help(field: Field, ws: &Workspace, keys: &Keymap) -> Help` (W2 adds the Keys arms)
  - `pub fn layout(help: &Help, width: u16) -> Vec<Line<'static>>` (wrapped, styled lines of the box's inside)
  - `pub fn box_height(help: &Help, width: u16, room: u16) -> Option<u16>` (None: don't draw)
  - `pub const MIN_TEXT_ROWS: u16 = 3;`
  - `pub fn draw(f: &mut Frame, area: Rect, help: &Help)`
  - `pub fn strings() -> Vec<&'static str>` (every translatable help sentence, for `ctl locale-template`)

- [ ] **Step 1: Write the failing tests** (in `prefs_help.rs`):
  - `every_item_has_help`: for every `Field` in `prefs::CATEGORIES`, `Field::ScheduleHours` and `Field::ScheduleDay(0..7)`,
    `help(..)` has a non-empty `title` (= the row's label) and at least one `Text` line.
  - `the_schedule_lists_its_kinds`: `help(Field::Schedule, ..)` has `Value` lines named exactly `off`, `every day`,
    `weekdays`, `custom`, in that order, and a `Key` line whose `keys` contain the user's key for "change" (see below).
  - `a_custom_day_explains_its_values`: `help(Field::ScheduleDay(0), ..)` has `Value` lines `receive all day` and
    `paused all day` and mentions `9:00-17:00`.
  - `on_off_items_are_short`: `help(Field::Compact, ..)` has no `Key` line and at most 2 `Text` lines.
  - `help_names_your_keys`: with a keymap where the Preferences "change" action (the one Enter is bound to in
    `Context::Preferences`) is rebound to `x`, the Schedule help's `Key` line reads `x`, not `Enter`.
  - `the_box_disappears_below_its_minimum`: a help of 6 wrapped lines → `box_height(h, 60, 4) == None` (3 text rows +
    2 borders = 5 needed); `box_height(h, 60, 5) == Some(5)`; `box_height(h, 60, 20) == Some(8)`.
  - `a_short_help_needs_less`: a help of 2 lines → `box_height(h, 60, 4) == Some(4)`.
  - `values_align_and_wrap`: `layout` of a help with `Value{"off","…long…"}` and `Value{"every day","x"}` puts both
    meanings at the same column, wraps the long meaning with a hanging indent under that column, names bold.
  - `help_is_translatable`: `strings()` contains the Schedule help's first sentence; `prefs::locale_strings()` (the
    existing list fn at ~line 581, whatever its name) includes it.
- [ ] **Step 2:** `cargo test -p klack prefs_help` → FAIL (module/functions missing).
- [ ] **Step 3: Implement.** Content per the spec's Part 1 "What a help entry says": what the option does; its values
  (as `Value` lines when it has several); how to set it when it is more than Enter toggling — keys from
  `keys.first_key(Context::Preferences, <action>)`. Write every text by reading what the Field really does in
  `prefs.rs` (`change`, `value`, `schedule_change`) — e.g. which are "kept on Slack", what + / - step through, typed
  formats (status `:emoji: text for 30m`, hours `9:00-17:00`), env overrides (`KLACK_AUDIO_INPUT` / `_OUTPUT`). Today's
  hint texts are the facts to redistribute per item. Strings: sentences and value meanings through `tr`; bullets,
  alignment and key colouring by `layout`. The box: rounded border in the dim colour, title = item label, indented
  like the rows, under the list after one blank row, width = the rows' width; when `box_height` is None nothing is
  drawn; a help taller than the box is cut with its last visible line ending in "…". Rows are never covered.
- [ ] **Step 4:** `cargo test` → all green; `cargo build` → no warnings.
- [ ] **Step 5: Live:** `tools/launch.sh`, F6: Availability (Notification schedule on weekdays and on custom: switch to
  custom on test account A only after snapshotting its `dnd_*` prefs, restore exactly after), Appearance, Audio &
  video; screenshot each (`tools/shot.sh`); shrink the font (`kitty @ set-font-size 20`) until the box disappears,
  then `set-font-size 0`. settings.toml unchanged (diff with a backup).
- [ ] **Step 6:** PROGRESS row + commit `feat: F6 shows formatted help for the selected item (W1)`.

### Task 2 (W2): F6 → Keys help (settings, bindings, groups, the filter)

**Files:** Modify `crates/klack/src/tui/prefs_help.rs`, `crates/klack/src/tui/prefs.rs` (pass whether the filter
has the focus).

**Interfaces:**
- Consumes: W1's `Help`, `HelpLine`, `help`, `draw`.
- Produces: `pub fn filter_help(keys: &Keymap) -> Help` (shown while the Keys filter has the focus).

- [ ] **Step 1: Failing tests:**
  - `a_binding_says_where_it_works_and_its_keys`: for `ACTIONS` index of `jump_to`, `help(Field::Binding(i), ..)` has
    `Text` = its full `description`; `Value` lines `Works` → `everywhere (Global)` (the context title for others),
    `Keys` → the keymap's current label, `keys.toml` → `jump_to`; `Key` lines for record (Enter), unbind (Delete),
    reset (r) — from the user's Preferences keys.
  - `the_default_shows_only_when_changed`: no `Default` value line with default keys; rebind `jump_to` → a `Default`
    line with the default keys.
  - `a_group_row_says_how_to_rename`: `help(Field::GroupHeader(n), ..)` has `Key` lines for rename (Enter) and icon (i).
  - `the_filter_explains_itself`: `filter_help` mentions what `/` matches, Enter keeps, Esc clears.
  - Keys settings fields (`WhichKey*`, `Leader`, `InsertLeader`) are covered by extending `every_item_has_help`.
- [ ] **Step 2:** run → FAIL. **Step 3:** implement (check in `keymap.rs`/`prefs.rs` what the filter really matches
  and which keys record/unbind/reset/rename/icon really are). **Step 4:** green, no warnings.
- [ ] **Step 5: Live:** F6 → Keys: a binding, a group row, `/` filter focused; screenshots. Rebind nothing (or restore
  keys.toml from a backup if one existed; none exists today — delete one a test created).
- [ ] **Step 6:** PROGRESS row + commit `feat: F6 → Keys explains each binding, group and the filter (W2)`.

### Task 3 (W3): profiles, `auth` commands (no guide yet), sessions keyed by team + user

**Files:**
- Create: `crates/slack-core/src/profiles.rs` (declare in `crates/slack-core/src/lib.rs`), `crates/klack/src/auth.rs`
- Modify: `crates/slack-core/src/session.rs`, `crates/klack/src/main.rs` (CLI), `crates/klack/src/ctl.rs` (profile
  resolution), `README.md` (commands; drop `login`)

**Interfaces:**
- Produces (slack-core):
  - `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] #[serde(tag = "kind", rename_all = "snake_case")]
    pub enum SessionSource { Browser { browser: String, profile: PathBuf, label: String }, Window, Cookie }`
  - `WorkspaceSession.source: Option<SessionSource>` with `#[serde(default, skip_serializing_if = "Option::is_none")]`
  - `Profile::upsert` replaces only the same `team_id` AND `user_id`; `Profile::matching(&self, team: &str) ->
    Vec<&WorkspaceSession>` (by id or name); `Profile::remove(&mut self, team_id: &str, user_id: &str) -> bool`
  - `pub struct Dirs { pub config: PathBuf, pub cache: PathBuf, pub data: PathBuf }` with `Dirs::system() ->
    Result<Dirs>` (from `session::project_dirs`), in `profiles.rs`
  - `pub fn valid_name(name: &str) -> Result<()>` (non-empty, `[A-Za-z0-9_-]` only, not `default-profile`)
  - `pub fn list(d: &Dirs) -> Vec<String>` (sorted; sub-folders of `d.config` holding `session.json`)
  - `pub fn default_name(d: &Dirs) -> String` (`d.config/default-profile`, first line trimmed, else `"default"`)
  - `pub fn set_default(d: &Dirs, name: &str) -> Result<()>` (refuses unknown names)
  - `pub fn paths(d: &Dirs, name: &str) -> Vec<PathBuf>` (existing `config/<name>`, `cache/<name>`)
  - `pub fn remove(d: &Dirs, name: &str) -> Result<Vec<PathBuf>>` (refuses while running; resets the default to
    `default` when it named this profile)
  - `pub fn lock(d: &Dirs, name: &str) -> Result<Option<std::fs::File>>` (`cache/<name>/running.lock`, `try_lock`;
    None = another klack holds it) and `pub fn is_running(d: &Dirs, name: &str) -> bool`
- Produces (klack): `auth.rs` with `pub struct LoginFlags { workspace: Option<String>, cookie: Option<String>,
  no_browser: bool, from_browser: Option<login_import::Source>, browser_profile: Option<PathBuf> }`,
  `pub async fn login(profile: &str, flags: LoginFlags) -> Result<()>` (today's non-interactive login moved from
  `main.rs`, now recording `source`), `pub async fn logout(profile: &str, team: Option<&str>, all: bool) -> Result<()>`,
  `pub async fn status(profile: &str) -> Result<()>`, `pub fn confirm(question: &str, yes: bool) -> Result<bool>`
  (y/N; without a terminal and without `--yes` it refuses with "add --yes").

CLI shape (clap): `--profile <NAME>` global, no default; the effective profile is `--profile` →
`profiles::default_name` → `"default"`. Subcommands: `auth login [--workspace URL] [--cookie] [--no-browser]
[--from-browser firefox|chrome] [--browser-profile DIR]`, `auth logout [--team T] [--all]`, `auth status`,
`profile list`, `profile default <NAME>`, `profile remove <NAME> [--yes]`, `purge [--yes]` (W4), `ctl …` as today.
No `login`. `init_logging` only runs for the TUI, `ctl`, `auth status` and `auth logout` once the profile has a
`session.json`; `auth login`, `profile …` and `purge` log nowhere (no folder is created for a typo). The TUI holds
`profiles::lock` for its lifetime when it is free (a second klack on the same profile still starts, as today, without
it); the lock only guards `profile remove` and `purge`.

- [ ] **Step 1: Failing tests** (`profiles.rs` with `Dirs` under a temp folder; `session.rs`):
  - `bad_names_are_refused`: `valid_name` rejects `""`, `"."`, `".."`, `"../x"`, `"a/b"`, `"a\\b"`, `"default-profile"`;
    `remove`/`set_default` with them return Err and touch nothing (the temp tree is unchanged).
  - `profiles_are_listed_and_the_default_marked`: config has `a/session.json`, `b/session.json`, `c/` (no session)
    → `list == ["a","b"]`; `default_name == "default"`; after `set_default("b")` → `"b"`; `set_default("zz")` → Err.
  - `removing_takes_config_and_cache_only`: `remove("a")` deletes `config/a`, `cache/a`, keeps `data/drafts/x.json`,
    returns both paths.
  - `removing_the_default_resets_it`.
  - `a_running_profile_is_not_removed`: hold `lock(d,"a")` → `remove` Err mentioning it is running; drop → Ok.
  - `two_accounts_of_one_team_are_kept` and `the_same_account_is_replaced` (`upsert`).
  - `old_session_files_still_load`: a JSON without `source` (and with `default_team`) loads; `source == None`;
    `workspace(None)` returns the default team's first account; `matching("acme")` returns both accounts of a team.
  - `source_round_trips`: each `SessionSource` variant serializes and loads back.
- [ ] **Step 2:** run → FAIL. **Step 3:** implement; `auth status` prints one line per workspace — team name, URL,
  `as <user>`, source label (`Chrome · Work` / `browser window` / `cookie` / `unknown`), `works` / `signed out` /
  `can't check: <why>` from `auth.test` run concurrently (10 s timeout) — and marks the default; `auth logout`
  without `--team`/`--all` in this task errors "say --team or --all" (W9 adds the list); `--team` matching two
  accounts asks which (W9) — here errors listing them. `profile list` prints `★` before the default, then each
  profile's workspaces `team (as user)` or `no sign-ins`; it reads files only. `profile remove` lists what goes and
  asks `confirm`. **Step 4:** green, no warnings.
- [ ] **Step 5: Live (fake home only):** copy A's entry into `$F/.config/klack/one/session.json`; `profile list`;
  `profile default one`; plain `klack auth status` (uses `one`; `works`); `auth logout --team rslack-testbed` then
  `auth status` (no sign-ins); `auth login --workspace rslack-testbed.slack.com` with `KLACK_COOKIE_D` set from A's
  cookie (python sets the env for the child; never echoed) → signed in, source `cookie`; `profile remove one`
  answered `n` then `--yes`; `klack --profile ../x profile list` and `profile remove ../x --yes` refused; `ls -R $F`
  shows no stray folders. `klack login` → clap error. Then `rm -r` the fake home by its literal path.
- [ ] **Step 6:** PROGRESS row + commit `feat: auth and profile commands; sessions remember their source (W3)`.

### Task 4 (W4): `klack purge`

**Files:** Modify `crates/slack-core/src/profiles.rs`, `crates/klack/src/main.rs`, `README.md`.

**Interfaces:**
- Consumes: W3's `Dirs`, `is_running`, `list`, `confirm`.
- Produces: `pub fn purge_targets(d: &Dirs, old: &[PathBuf], temp: &Path) -> Vec<PathBuf>` (existing paths only) and
  `pub fn purge(targets: &[PathBuf]) -> Result<()>`; `Dirs::old_system() -> Vec<PathBuf>` (the `slacktui` folders of
  `session::adopt_old_dirs`'s `ProjectDirs::from("io.github", "rslack", "slacktui")`).

Targets: `d.config`, `d.cache`, `d.data` (whole klack folders: every profile, settings.toml, keys.toml, locales/,
themes/, default-profile, logs, caches, `login-browser/`, drafts, sidebar), existing old-name folders, and in `temp`
only entries named `klack-cookies-*` or `klack-<digits>-*`. Refuses while any profile `is_running`. Prints the list,
asks `confirm("Delete all of this?", yes)`, deletes, prints what was deleted. Files the user saved from messages are
not under these folders and stay.

- [ ] **Step 1: Failing tests:** `purge_lists_every_klack_folder` (temp tree with config/cache/data + a fake old
  folder → all listed, missing ones not); `purge_keeps_unrelated_temp_files` (`klack-cookies-12-34` and
  `klack-777-F1.mp4` listed; `klackery`, `klack-notes.txt`, `other` not); `purge_refuses_while_running`.
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** green.
- [ ] **Step 5: Live (fake home only, `TMPDIR=$F/tmp` too):** populate with W3's steps + a drafts file; `purge`
  answered `n` (nothing gone), then `purge --yes` → `ls -A $F/.config $F/.cache $F/.local/share` shows no klack;
  a decoy `$F/tmp/other.txt` stays. The real `~/.config/klack` is untouched (`ls` before/after identical).
- [ ] **Step 6:** PROGRESS row + commit `feat: klack purge removes everything klack wrote (W4)`.

### Task 5 (W5): the inline picker and the text prompt

**Files:** Create `crates/klack/src/pick.rs` (declare in `main.rs`).

**Interfaces:**
- Produces:
  - `pub struct Item { pub text: String, pub detail: String, pub note: String, pub enabled: bool }`
    (`detail` dim after the text, `note` dim at the end, e.g. `as Dmytro` / `already signed in`; disabled items are
    drawn dim and never land the cursor)
  - `pub enum Row { Heading { title: String, detail: String }, Item(Item) }`
  - `pub enum Outcome { Picked(Vec<usize>), Cancelled }` (indices into the rows, in row order)
  - `pub struct Picker` with `pub fn new(rows: Vec<Row>, multi: bool) -> Picker`, `pub fn key(&mut self, k:
    KeyEvent) -> Option<Outcome>`, `pub fn click(&mut self, visible_row: usize)`, `pub fn visible(&self, height:
    usize) -> Vec<(usize, bool /*cursor*/, bool /*marked*/)>`
  - `pub fn choose(question: &str, rows: Vec<Row>, multi: bool) -> Result<Outcome>` (draws inline, erases itself,
    restores the terminal on every exit path incl. Ctrl+C → `Cancelled`)
  - `pub fn ask(prompt: &str, placeholder: impl Fn(&str) -> String) -> Result<Option<String>>` (a one-line text
    prompt; `placeholder(typed)` is drawn dim after the cursor; Enter → Some, Esc/Ctrl+C → None)
  - `pub fn interactive() -> bool` (stdin and stdout are terminals)

Keys (user): ↑ ↓ and j k move (headings and disabled items skipped, no wrap), Space toggles the cursor's mark when
`multi`, Enter returns the marked (or the cursor's item when none is marked), `/` starts a filter (typed letters go
to it, Backspace edits, Enter keeps it and returns to moving), Esc drops the filter, Esc without a filter (or Ctrl+C)
cancels. j/k never filter. A left click on an item moves the cursor there and toggles its mark (user) — single choice:
the click picks it. Filter: case-insensitive substring over an item's text + detail + note and its heading; a
heading shows while any of its items does. The view scrolls to keep the cursor visible; height = min(rows, terminal
height − 2).

- [ ] **Step 1: Failing tests** (pure `Picker`): `the_cursor_skips_headings_and_dim_items`;
  `space_marks_and_enter_returns_the_marked`; `enter_without_marks_takes_the_cursor`;
  `single_choice_ignores_space`; `slash_filters_esc_drops_it_then_cancels`; `j_and_k_never_filter` (in a filter
  `j` goes to the filter text only after `/`; outside it moves); `a_click_moves_and_marks` (multi) and
  `a_click_picks` (single); `a_long_list_scrolls_with_the_cursor` (30 items, height 8: after 20 Downs the cursor's
  row is visible); `ctrl_c_cancels`. Plus `the_placeholder_follows_the_text` for a pure helper used by `ask`.
- [ ] **Step 2:** FAIL. **Step 3:** implement (crossterm raw mode + ratatui `Terminal::with_options(Viewport::Inline(h))`;
  a guard struct whose `Drop` disables raw mode, shows the cursor and clears the viewport; mouse capture on while
  the picker is up). **Step 4:** green.
- [ ] **Step 5: Live:** none here (no caller yet); W7 checks the picker and the prompt live. PROGRESS says
  `NOT live (checked in W7)`.
- [ ] **Step 6:** PROGRESS row + commit `feat: an inline picker for terminal questions (W5)`.

### Task 6 (W6): find Slack sign-ins in every browser profile

**Files:** Modify `crates/klack/src/login_import.rs`, `crates/slack-core/src/session.rs`; test fixture
`crates/slack-core/tests/fixtures/signin-page.html` (a minimal, made-up page: only the `loggedInTeams` JSON, HTML-escaped
like Slack's, ids `T0000000001`/`T0000000002`, emails `person@example.com`).

**Interfaces:**
- Produces (slack-core): `pub struct LoggedInTeam { pub team_id: String, pub team_name: String, pub domain: String,
  pub url: String /* https://<domain>.slack.com/ */, pub email: String }`; `pub fn parse_logged_in_teams(html: &str)
  -> Vec<LoggedInTeam>`; `pub async fn signed_in_teams(cookie_d: &str) -> Result<Vec<LoggedInTeam>>` (GET
  `https://slack.com/signin` with the cookie and `USER_AGENT`; the page's JSON is HTML-escaped — `&quot;`, `\/`).
- Produces (klack, `login_import.rs`): `pub struct BrowserProfile { pub source: Source, pub dir: PathBuf, pub label:
  String /* "Chrome · Work" */, pub account: Option<String> /* Google account email */ }`;
  `pub fn browser_profiles() -> Vec<BrowserProfile>` (every Firefox and Chromium-family profile found; the family
  name from its root: Chrome / Chromium / Edge / Brave; Chrome-family `Local State` → `profile.info_cache.<dir>.name`
  and `.user_name`; Firefox `profiles.ini` `Name=` for the folder, else the folder name);
  `pub struct Candidate { pub team: LoggedInTeam, pub session: Result<WorkspaceSession, String> }`;
  `pub struct Group { pub profile: BrowserProfile, pub problem: Option<String>, pub candidates: Vec<Candidate> }`;
  `pub async fn discover() -> Vec<Group>` (per profile: its `d` cookies (freshest first) → `signed_in_teams` → for each
  team `session_from_cookie(team.url, cookie)`; all concurrent; 10 s timeout per step; a key that can't be read or
  a timeout becomes `problem` (e.g. "can't read Chrome's key: …"), never an error; Chrome-family skipped on Windows
  with problem "Chrome keeps its cookies to itself on Windows"; sessions carry `source = Browser{…}`).

- [ ] **Step 1: Failing tests:** `the_signin_page_names_every_workspace` (fixture → 2 teams with domain, url, email);
  `a_page_without_teams_gives_none` (signed-out page / garbage → empty); `chrome_profiles_are_named_as_chrome_shows_them`
  (temp root with `Local State` naming `Default` = "Work" / `user_name` = `person@example.com`, `Profile 1` unnamed →
  labels `Chrome · Work` with account, `Chrome · Profile 1`); `firefox_profiles_use_their_ini_names`;
  `an_unreadable_key_is_a_problem_not_an_error` (inject a failing key fn). Ignored: `discover_real_browsers_counts_only`
  (prints only counts: profiles, sign-ins, working) — run once by hand with `--ignored --nocapture` on this machine.
- [ ] **Step 2:** FAIL. **Step 3:** implement (refactor `find` onto `browser_profiles` so `--from-browser` keeps
  working). **Step 4:** green.
- [ ] **Step 5: Live:** run the ignored counts-only test against the real home (counts only in the report). Build the
  fake home's synthetic Firefox profile: `$F/.mozilla/firefox/profiles.ini` (`Name=klack-test`, `Path=kt.default`)
  and `kt.default/cookies.sqlite` with a `moz_cookies` row `d` for `.slack.com` holding test account A's cookie value
  (python + sqlite3, never printed); verify a debug-print-free path by W7's guide. Keep the fake home for W7.
- [ ] **Step 6:** PROGRESS row + commit `feat: find Slack sign-ins in every browser profile (W6)`.

### Task 7 (W7): the guided sign-in

**Files:** Modify `crates/klack/src/auth.rs`, `crates/klack/src/main.rs`, `crates/klack/src/login_browser.rs`,
`README.md`.

**Interfaces:**
- Consumes: W3 `login`, `LoginFlags`, `SessionSource`; W5 `choose`, `ask`, `interactive`, `Row`, `Item`; W6
  `discover`, `Group`, `Candidate`, `browser_profiles`.
- Produces: `pub fn workspace_url(input: &str) -> Result<String, String>`; `pub fn rows(groups: &[Group], profile:
  &Profile) -> (Vec<Row>, Vec<Choice>)` with `pub enum Choice { Candidate(usize, usize) /* group, candidate */,
  Other }` (one `Choice` per `Item` row, in row order); `pub async fn guide(profile_name: &str) -> Result<usize>`
  (sessions saved); `pub async fn sign_in_again(profile_name: &str, ws: &WorkspaceSession) -> Result<bool>` (W8).

Behaviour (spec Part 2 "When the guide runs" and "The list", all marked (user) there): first line `Signing in
profile <p>.` or `Profile <p> has no sign-ins yet.`; `Looking for Slack in your browsers…`; then `choose(.., multi =
true)` over headings `<label>  <account>` and items `<team name>  <domain>.slack.com  as <user>`; failed candidates
disabled with note `signed out in <label>`; workspaces already in the profile (team+user) noted `already signed in`
(picking refreshes); last item `another workspace, or sign in another way…`. Picking that: `ask("Workspace: ",
|t| if t.contains('.') || t.contains('/') { "" } else { ".slack.com" })` → `workspace_url` → a single-choice list of
ways: browser profiles whose candidates match that domain, `in a new browser window` (note `fresh — Google won't know
you; may ask for a captcha`), `paste the d cookie by hand`. Then sign in, `profile.upsert`, save, and print per
workspace `✓ Signed in to <team> as <user> — profile <p> saved.` or `✗ <team>: <why>`. Esc anywhere → `Nothing signed
in.`, exit 0, no folder created. Plain `klack` on a profile with no `session.json`: interactive → the guide, then the
TUI if anything was saved; not interactive → error `profile <p> has no sign-ins — klack auth login`. `auth login` with
flags follows the spec's flag rules (`--workspace` alone in a terminal → the ways list for it; `--from-browser`,
`--no-browser`, `--cookie`/`$KLACK_COOKIE_D` pick the way). `sign_in_again` (user: "remember the source and offer
it first"): source `Browser` → read that profile's cookies, and if one still signs in to that team as that user →
`choose("<label> is still signed in to <team> — use it?", [use it], single)`, Enter saves it, Esc → the full guide;
if none does → print `<label> is signed out of <team> as well.` and run the full guide; source `Window` / `Cookie` /
none → the full guide. The browser window gets `--disable-sync` and the Chrome
switches that stop the sign-in "set up a profile" interception (find the current feature names, e.g.
`--disable-features=…`; record which in the report; can't be checked live without the user's Google account → NOT).

- [ ] **Step 1: Failing tests:** `workspace_url` cases — `acme` → `https://acme.slack.com/`, `acme.slack.com`,
  `https://acme.slack.com/archives/C1` → same, `acme.enterprise.slack.com` kept, ` Acme ` → lower-cased trimmed,
  `https://app.slack.com/client/T1/C1` → Err("that link doesn't name the workspace — type its name"), `a b`/empty →
  Err; `rows_put_headings_failures_and_the_last_row` (two groups, one failed candidate, one already signed in →
  headings, a disabled item with its reason, the note, last item Other, `Choice` per item); `a_problem_group_is_a_dim_heading`
  (problem shown, no items).
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** green.
- [ ] **Step 5: Live (fake home):** kitty test window running `env HOME=$F … ./target/debug/klack --profile g auth
  login; exec sleep 600` (one command; Chrome absent there): the list shows `Firefox · klack-test` with rslack-testbed
  `as <A's handle>`; j/k, `/rsl`, Esc, Space, Enter → `✓ Signed in …`; `auth status` → source `Firefox ·
  klack-test`; again → `already signed in`; `another workspace…` → type `rslack-testbed` (screenshot shows the dim
  `.slack.com`), Esc out; `--profile typo` + Esc → no `$F/.config/klack/typo` nor cache folder; a mouse click on a row
  (SGR escape via `kitty @ send-text`) marks it. Plain `klack --profile g2` (no sign-ins) → guide → TUI opens.
- [ ] **Step 6:** PROGRESS row + commit `feat: klack auth login guides you through signing in (W7)`.

### Task 8 (W8): expired sessions at start; the TUI skips signed-out workspaces

**Files:** Modify `crates/klack/src/auth.rs`, `crates/klack/src/main.rs`, `crates/slack-core/src/api.rs`,
`crates/klack/src/tui/event_loop.rs` (`run`, `switch_workspace`), `crates/klack/src/tui/others.rs`,
`crates/klack/src/tui/rail.rs`.

**Interfaces:**
- Consumes: W7 `sign_in_again`; W5 `choose`.
- Produces: `pub fn is_signed_out(e: &anyhow::Error) -> bool` (api.rs: an `ApiError` whose `error` is one of
  `invalid_auth`, `not_authed`, `token_revoked`, `account_inactive`); `pub enum Health { Works, SignedOut,
  Unknown(String) }`; `pub async fn check(profile: &Profile) -> Vec<Health>` (auth.rs; concurrent `auth.test`, 10 s);
  `pub type SignedOut = Arc<Mutex<HashSet<usize>>>` (event_loop.rs); `event_loop::run(profile, team, signed_out:
  HashSet<usize>)`; `fn start_index(sessions: &[WorkspaceSession], wanted: usize, signed_out: &HashSet<usize>) ->
  Option<usize>`; `fn switch_check(index: usize, current: usize, labels: &[String], in_call: bool, signed_out:
  &HashSet<usize>) -> Result<usize, String>`.

Behaviour (spec "Expired sessions", user's answers 2026-10-20): before the TUI, `check`; for each `SignedOut` one,
`choose("<team> (as <user>): signed out", [sign in again, remove it from the profile, launch anyway], single)` —
"launch anyway" omitted when no workspace works (then `quit`); sign in again → `sign_in_again` (source first); remove
→ `Profile::remove` + save. Not interactive → a warning line per signed-out workspace and launch with the rest (or
exit with an error when none works). `Unknown` (network) → launch as normal. The TUI starts on the wanted workspace
if it works, else the first that does; the rail draws a signed-out workspace dim with `⊘`; switching to it keeps you
where you are with status `<team>: signed out — klack auth login, then restart klack`; the pollers (`others::spawn`,
`spawn_watchers`, `spawn_logos`) skip signed-out sessions.

- [ ] **Step 1: Failing tests:** `signed_out_errors_are_recognised` (ApiError invalid_auth → true; ratelimited,
  a network error → false); `start_skips_signed_out` (wanted 0 signed out → 1; all out → None);
  `switching_to_a_signed_out_workspace_stays` (Err with the exact status text above); rail: a signed-out entry renders
  dim with `⊘` (TestBackend, see existing rail tests or add one).
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** green.
- [ ] **Step 5: Live (fake home):** profile `x` with A's working entry and a copy of B's entry with a broken token
  (`xoxc-0` — invalid_auth): start `klack --profile x` in the test window → the question for B; "launch anyway" → TUI
  on A, rail shows B dim with `⊘`, Alt+2 → the status text; restart, "remove it from the profile" → `auth status`
  lists A only. All-broken profile → no "launch anyway".
- [ ] **Step 6:** PROGRESS row + commit `feat: expired sessions are offered a fix at start; signed-out workspaces are skipped (W8)`.

### Task 9 (W9): sign-out while running; `auth logout` and `--team` lists

**Files:** Modify `crates/slack-core/src/api.rs` (`Client`), `crates/slack-core/src/rtm.rs`,
`crates/klack/src/tui/event_loop.rs`, `crates/klack/src/tui/others.rs`, `crates/klack/src/auth.rs`.

**Interfaces:**
- Consumes: W8 `is_signed_out`, `SignedOut`; W5 `choose`.
- Produces: `Client::signed_out(&self) -> bool` (an `Arc<AtomicBool>` set by `call_raw` when `is_signed_out` holds for
  its error, shared by clones); `RtmEvent::SignedOut` (the RTM loop sends it and stops instead of retrying when the
  client is signed out or `rtm.connect` answers a signed-out error).

Behaviour (spec "While running", user's answer): the current workspace signed out → added to `SignedOut`, status
`<team>: signed out — klack auth login, then restart klack` (kept, not overwritten by routine notes), what is loaded
stays readable, sends fail with that note, RTM stops; another workspace's poller seeing it → added to the set, rail
mark. `auth logout` with no `--team`/`--all` in a terminal → `choose(multi)` over the profile's workspaces; `--team`
matching two accounts in a terminal → `choose(single)`; not interactive → the W3 errors. The same for the TUI's and
`ctl`'s `--team`: two accounts there → `choose(single)` before the TUI starts (not interactive: refuse, listing
`team (as user)` for each).

- [ ] **Step 1: Failing tests:** `a_signed_out_answer_sets_the_flag_for_every_clone` (drive the flag-setting helper
  that `call_raw` uses with an `ApiError`); `the_rtm_loop_stops_when_signed_out` (the loop's decision function:
  signed out → stop, other errors → back off); logout list rows (pure: profile → rows naming `team (as user)`).
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** green.
- [ ] **Step 5: Live (fake home):** extend the debug-only `KLACK_FAIL` (api.rs) to take `method=error` items
  (`KLACK_FAIL=conversations.history=invalid_auth` makes that call fail with an `ApiError` of that error; plain
  `method` keeps today's `injected_failure`) — never revoke a real token. Start the TUI on a fake-home profile with
  A's entry and `KLACK_FAIL=conversations.history=invalid_auth`, open a conversation → the status text, RTM stopped
  (log), what was loaded still shown. `auth logout` list with two entries (A and B copied in): mark one, Enter →
  `auth status` shows the other.
- [ ] **Step 6:** PROGRESS row + commit `feat: a session ending while klack runs is noticed; auth logout lists (W9)`.

### Task 10 (W10): other systems and the docs

**Files:** `README.md`, `CONTRIBUTING.md` if it mentions login, F1 help text in `crates/klack/src/tui/help.rs` if it
mentions `klack login`.

- [ ] `grep -rn "klack login\|\blogin\b --" README.md CONTRIBUTING.md crates/` — every mention updated to `auth login`;
  README gains a "Signing in" section (the guide, `auth status/logout`, profiles, `purge`) in the README's voice.
- [ ] Windows VM (`win11-dev`, see memory klack-test-machines) and the Mac (`macbook`): build the debug binary, run
  `klack profile list`, `klack auth status` with a copied test entry in a scratch profile, `klack auth login` over
  `ssh -t` up to the list (Windows: no Chrome group, Firefox if present; Mac: Chrome's key prompt can't be answered
  over ssh → the dim heading with the reason within 10 s, no hang), Esc. Remove the scratch profile and binaries
  after.
- [ ] PROGRESS row + commit `docs: sign-in, profiles and purge in the README; W checked on Windows and macOS (W10)`.

---

## After the tasks (controller)

1. Whole-branch review (fresh reviewer, all of W1–W10 against the spec) → one fix wave → PROGRESS row W-final.
2. Release (authorized by the user 2026-10-20: "upon completion release a new version") as **v0.7.0** (carries
   queue V and W): bump the workspace versions as the v0.6.0 bump did (`git show 39554bf --stat` for the files), CI
   green on the bump, tag `v0.7.0`, push the tag, watch `release.yml`, release notes first line: "`klack login` is
   now `klack auth login` (with a guided sign-in)"; verify the Linux download against `SHA256SUMS` and run
   `klack --version`; PROGRESS "Next" updated.

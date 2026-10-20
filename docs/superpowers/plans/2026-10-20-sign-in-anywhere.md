# Sign in anywhere (queue Y) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build queue Y as approved by the user on 2026-10-20. After the browser sign-ins it already lists, the
guide offers three equal ways to sign in: a code from your email (QR + link for the phone; klack confirms the
code), "in your browser" (a printed link, with klack watching the browser profiles for the new sign-in) and
pasting a cookie. The throwaway browser window goes.

**Architecture:** Slack's code protocol lives in a new `crates/slack-core/src/signin.rs`, a cookie-jar client
built on the existing `api::http_client` (reqwest's `cookies` feature). The QR drawing and the OSC 8 link are in a
new `crates/klack/src/qr.rs`. Watching the browsers is a new `crates/klack/src/login_watch.rs`, built on
`login_import`. To make that possible, the browsers' keys are read separately from looking at the profiles
(`read_keys` + `discover_with_keys`). `auth.rs` gets the new list rows and the three way flows. `pick.rs` gets
pre-marked rows. `login_browser.rs` is deleted.

**Tech Stack:** Rust 2024, reqwest 0.13 (+ `cookies` feature, `reqwest::cookie::Jar`), `qrcode` crate
(`default-features = false`), crossterm 0.29 (`EventStream` for the browser wait), ratatui 0.30 inline viewport
(existing picker). No other new dependencies; `tokio-tungstenite` leaves `crates/klack` if only `login_browser.rs`
used it.

**Spec:** `docs/superpowers/specs/2026-10-20-sign-in-anywhere-design.md`. Read the whole spec before your task. "(user)"
decisions must not be changed. It builds on W: `docs/superpowers/specs/2026-10-20-prefs-help-and-auth-design.md`.

## Global Constraints

- The house routine for every task: `cargo build` with no warnings → `cargo test` green (never commit on red:
  `cargo test 2>&1 | grep -q FAILED && exit 1`) → `cargo build` again (`cargo test` does not rebuild
  `target/debug/klack`) → the task's live check → a PROGRESS.md table row after the `| X8` row, in the format
  `| Y<n> — what (user's words where they exist) | done | UNIT: …; LIVE: …; NOT: … | (this commit) |`. Be honest
  about what was not checked live → ONE commit `feat|fix: … (Y<n>)` ending with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. The controller makes the "docs: PROGRESS for Y<n>" sha
  commit, pushes master and watches CI (all four builds, macOS included).
- Never print or log tokens, cookies, codes, magic links or `magic_login_code`. The email appears only on screen,
  where the user typed or confirms it, never in `tracing`. `Debug` of every new type holding one of these redacts
  it (as `WorkspaceSession` does). Fixtures use made-up ids (`T0000000001`, `U0000000001`) and
  `person@example.com`, never the test workspace's ids or anyone's real email.
- Nothing is signed out anywhere; every way only adds sessions. klack never opens a browser and never changes the
  system, a browser or its profiles (it only reads them). It never sends the code itself.
- The user's real data is never touched. `~/.config/klack/test` (their work workspace) is never signed in or out,
  removed, or read for tokens. `settings.toml` and `keys.toml` are backed up and restored if a check could write
  them, and real drafts stay. **Every live check of the guide runs under a fake home:** `F=<scratchpad>/fakehome;
  env HOME=$F XDG_CONFIG_HOME=$F/.config XDG_CACHE_HOME=$F/.cache XDG_DATA_HOME=$F/.local/share ./target/debug/klack
  --profile <p> auth login …` (0700, deleted after the check). A test session there is copied from
  `~/.config/klack/default/session.json` (A) or `b/session.json` (B) with python and never printed.
- Browser-way checks use only throwaway Firefox profiles under the fake HOME. The checker builds them (a
  `profiles.ini` and a `cookies.sqlite` with a `moz_cookies` table, as W6 did), never the user's real browsers.
- **Codes are requested only by the user**, on their phone, for test account B (they read B's inbox). The
  implementer never requests one and never runs the code way live alone: the controller asks the user and relays
  the code typed into the test window by `kitty @ send-text` from a variable, never echoed.
- Never build into `target/release`. Debug builds only. No `cargo fmt`. `pgrep -x`/`pkill -x` only, never
  `pkill klack`, never `rm -rf $VAR`. Quit the test klack with Ctrl+Q (or Esc out of the guide) before closing its
  kitty window. Windows VM and Mac: restore the power state found.
- User-facing text: the wording quoted in the spec is fixed; anything else is the builder's, in the house voice
  (plain, short, lower-case notes). `auth.rs` and `pick.rs` do not go through `i18n::tr` today; keep it that way.

## Review Focus

- **A code typed or pasted the way people do it**, like ` rep-oeu `, `REP OEU`, `repoeu` or `REP–OEU` (en dash),
  is taken as `REPOEU`. Anything that is not 6 letters or digits after dropping spaces and dashes is refused with
  no request sent. Test in Task 1.
- **A reply that is not Slack's JSON**, such as HTML from a captcha or rate-limit page, an HTTP 429 or 5xx, or
  `{"ok":false}` with no or an unknown `error`, gives a readable error and no panic. The body is never shown, as it
  may hold a link. Test in Task 1.
- **One of several marked teams fails to redeem** (no `d` in the jar afterwards, or `session_from_cookie` refuses).
  The others are still saved and that team gets a `✗ <team>: <why>` line. Test in Task 3.
- **A terminal too small for the QR, or a dark theme.** Too narrow puts the link under the code; too short shows
  the link alone. The modules always use explicit black-on-white SGR colours. Test in Task 2.
- **The browser rewrites its cookie file with no new sign-in** (mtime changes, same `d`): nothing is offered. A
  database that is mid-write or unreadable at one tick is tried again at the next, with no error shown. A profile
  created during the wait is watched too. Esc or Ctrl+C while waiting puts raw mode back. Tests in Task 5.

---

### Task 1 (Y1): slack-core — Slack's code sign-in protocol and `SessionSource::Code`

**Files:**
- Create: `crates/slack-core/src/signin.rs` (declare `pub mod signin;` in `crates/slack-core/src/lib.rs`)
- Create: `crates/slack-core/tests/fixtures/confirm-code-ok.json`, `find-workspaces.json` (made up, using the
  field names the spec recorded: `current_teams[]` with `id, name, domain, url, magic_login_url,
  magic_login_code, sso_required, two_factor_required, is_logged_out`, plus `current_orgs`, `invited_teams`,
  `domain_enabled_teams`. Mark both "made-up shape, from the fields seen live on 2026-10-20".)
- Modify: `crates/slack-core/Cargo.toml` (reqwest features + `"cookies"`)
- Modify: `crates/slack-core/src/session.rs` (`SessionSource::Code { email: String }`, plus its round-trip in
  `source_round_trips`)
- Modify: `crates/klack/src/auth.rs` (`source_label`: `Code { email }` → `code to <email>`, `Window` →
  `a browser window`, and `sources_are_named` updated)

**Interfaces:**
- Produces (in `slack_core::signin`):
  - `pub const SIGNIN_PAGE: &str = "https://slack.com/signin";`
  - `pub fn signin_link(email: &str) -> String`: `https://slack.com/signin?email=<percent-encoded>`. Keep
    `A–Z a–z 0–9 - _ . ~`, encode every other byte of the UTF-8 as `%XX` (upper-case hex).
  - `pub fn normalize_code(typed: &str) -> Option<String>`: drop whitespace and `-` `‐` `–` `—`, upper-case. `Some`
    only for exactly 6 ASCII alphanumerics.
  - `pub fn email_ok(typed: &str) -> bool`: trimmed, one `@` with text before it and a `.` inside the part after.
  - `#[derive(Debug, Clone, PartialEq, Eq)] pub enum CodeError { Failed, Expired, RateLimited, Other(String) }` with
    `Display`: `Failed` → "that code didn't work — type it again, or ask for a new one on the phone", `Expired` →
    "that code has expired — ask for a new one on the phone", `RateLimited` → "Slack says too many tries — wait a
    while, then ask for a new code", `Other(e)` → "Slack refused the code: {e}". Implements `std::error::Error`.
  - `pub fn parse_confirm(reply: &serde_json::Value) -> Result<bool, CodeError>`: `has_workspace` on ok. Errors
    `failed`, `expired`, `ratelimited` map to their variants; any other error, or none, is `Other` with the name,
    or "no answer" when missing.
  - `#[derive(Clone, PartialEq, Eq)] pub struct CodeTeam { pub team_id: String, pub name: String, pub domain:
    String, pub url: String, pub magic_login_url: String, pub sso_required: bool, pub two_factor_required: bool }`,
    with a manual `Debug` that redacts `magic_login_url`.
  - `pub fn parse_workspaces(reply: &serde_json::Value) -> Result<Vec<CodeTeam>>`: `current_teams`, then each
    entry of `current_orgs`. An org that has an array `teams` contributes those entries, and one that has a
    `magic_login_url` itself is a team. This part is unverified (no Grid account), so say so in a comment. Teams
    without `id` or `magic_login_url` are dropped. Each team appears once, by `team_id`. `url` goes through
    `session::workspace_url` and falls back to `https://<domain>.slack.com/`. `ok:false` is an error naming
    Slack's error.
  - `pub fn cookie_value(header: &str, name: &str) -> Option<String>`: the value of `name` in a `Cookie:` header
    (`a=1; d=xoxd-…; x=2`).
  - `pub struct CodeSignIn { email: String, http: reqwest::Client, jar: std::sync::Arc<reqwest::cookie::Jar> }`
    (manual `Debug`: email only)
  - `impl CodeSignIn`:
    - `pub async fn start(email: &str) -> Result<CodeSignIn>`: builds the client with
      `api::http_client(|| Client::builder().user_agent(session::USER_AGENT).cookie_provider(jar.clone()))`, then
      `GET SIGNIN_PAGE`.
    - `pub fn email(&self) -> &str`
    - `pub async fn confirm(&self, code: &str) -> Result<bool, CodeError>`: `signin.confirmCode` with `email`
      and `code`. A transport error, a non-2xx status or a non-JSON body becomes `Other("Slack did not answer as
      expected (HTTP <status>)")`, without the body.
    - `pub async fn workspaces(&self) -> Result<Vec<CodeTeam>>`: `signin.findWorkspaces` with `email` and
      `reason=get_started_workspaces`.
    - `pub async fn redeem(&self, team: &CodeTeam) -> Result<WorkspaceSession>`: GET `magic_login_url` (redirects
      followed, which is reqwest's default), then `d` from `jar.cookies(slack.com)` via `cookie_value`. It errors
      "Slack did not sign in to <name>" when there is none. Then
      `session::session_from_cookie_with(&session::web_client()?, &team.url, &d)` (a client without the jar, so
      there is only one Cookie header), returned with `source: Some(SessionSource::Code { email })`.
  - Private `async fn call(&self, method: &str, form: reqwest::multipart::Form) -> Result<serde_json::Value,
    CodeError>`: `POST https://slack.com/api/<method>?_x_id=noversion-<unix seconds>`.

- [ ] **Step 1: Write the failing tests** in `signin.rs`'s `mod tests`:
  - `the_link_fills_in_the_email`: `signin_link("you@gmail.com") == "https://slack.com/signin?email=you%40gmail.com"`,
    `signin_link("a+b c@x.io")` ends in `a%2Bb%20c%40x.io`, `signin_link("ü@x.io")` ends in `%C3%BC%40x.io`.
  - `codes_are_taken_as_people_type_them`: `REP-OEU`, ` rep-oeu `, `REP OEU`, `rep–oeu` and `repoeu` all give
    `Some("REPOEU")`. `REP-OE`, `REP-OEUX`, `RE?-OEU` and `` give `None`.
  - `emails_are_checked_loosely`: `you@gmail.com` and ` a.b@c.co ` are ok; ``, `you`, `@x.io`, `you@x` and `a@b@c.io`
    are not.
  - `confirm_replies_map_to_errors`: `{"ok":true,"has_workspace":true}` → `Ok(true)`. `failed`, `expired` and
    `ratelimited` give their variants, `{"ok":false,"error":"captcha_failed"}` gives `Other("captcha_failed")`,
    and `{"ok":false}` gives `Other(..)`. Also check the `Failed` message text against the spec.
  - `workspaces_come_from_teams_and_orgs`: the fixture gives the teams in order, Grid teams after (from an org
    `teams` array in the fixture), the duplicate team once and the one without `magic_login_url` dropped. Check
    `sso_required` and `two_factor_required`, and that `format!("{t:?}")` holds no `magic_login_url` value.
  - `a_cookie_is_found_by_name`: `cookie_value("b=1; d=xoxd-a%2Fb; d-s=9", "d") == Some("xoxd-a%2Fb")`, and
    `cookie_value("b=1", "d") == None`.
  - `a_page_that_is_not_json_says_so_without_its_body` (`#[tokio::test]`): a local `TcpListener` answers `HTTP/1.1
    429` with an HTML body that contains `https://secret.example/magic`. A test-only `CodeSignIn::at(base)`
    constructor (a `#[cfg(test)]` base-URL override) calls `confirm`, which gives `Other(m)`, with `m` containing
    `429` and not `secret`.
  - In `session.rs`: `source_round_trips` gains `SessionSource::Code { email: "person@example.com".into() }`, and
    its JSON contains `"kind":"code"`.
- [ ] **Step 2: Run them** with `cargo test -p slack-core signin`. Expected: FAIL to compile (module missing).
- [ ] **Step 3: Implement** `signin.rs` as the Interfaces block says, add the `cookies` feature and
  `SessionSource::Code`. Fix every `match` on `SessionSource` the compiler names (`source_label` in `auth.rs`).
- [ ] **Step 4: Run** `cargo build` (no warnings), then `cargo test`. Expected: all green.
- [ ] **Step 5: Live check** (no code needed, nothing signed in): an ignored test or a scratch `ctl`-free example
  that runs `CodeSignIn::start("person@example.com")`. It must get the sign-in page (cookies `b`, `x` in the
  jar, names only printed). Then `confirm("AAAAAA")` must return `Failed`, `Expired` or `Other(..)`, mapped
  without a panic. Record the error name. This sends no email: confirming does not need the captcha and sends
  nothing. PROGRESS: UNIT + this LIVE, and "NOT: a real code (Task 3)".
- [ ] **Step 6: Commit** `feat: Slack's email-code sign-in in slack-core — link, code, workspaces, magic link → session (Y1)`.

### Task 2 (Y2): the QR code and the code screen's layout

**Files:**
- Create: `crates/klack/src/qr.rs` (declare `mod qr;` in `main.rs`)
- Modify: `crates/klack/Cargo.toml` (`qrcode = { version = "0.14", default-features = false }`)

**Interfaces:**
- Produces:
  - `pub fn modules(text: &str) -> Vec<Vec<bool>>`: dark = `true`, with error correction level M and a 4-module
    quiet zone of `false` on every side. The result is square.
  - `pub fn half_blocks(modules: &[Vec<bool>]) -> Vec<String>`: one terminal line per two module rows (an odd
    last row is padded with light). Each cell is `▀`, with the foreground the top module and the background the
    bottom one: dark = `\x1b[30m` / `\x1b[40m`, light = `\x1b[97m` / `\x1b[107m`. Each line ends with `\x1b[0m`.
  - `pub fn osc8(url: &str) -> String`: `\x1b]8;;{url}\x1b\\{url}\x1b]8;;\x1b\\`.
  - `pub enum Fit { Beside, Under, LinkOnly }` and `pub fn fit(qr_cols: usize, qr_rows: usize, text_cols: usize,
    width: u16, height: u16) -> Fit`. `Beside` needs `qr_cols + 3 + text_cols <= width`, `Under` needs
    `qr_cols <= width`, and both need `qr_rows + 6 <= height` (heading, email, blank, code prompt, hint, margin).
    Otherwise `LinkOnly`.
  - `pub fn code_screen(email: &str, width: u16, height: u16) -> Vec<String>`: the spec's Way-1 block. The lines
    are "Sign in with a code", "  Email: <email>", a blank, then the QR with the text column ("On your phone, scan
    this — or open", the OSC 8 link, "Slack emails you a code. Type it here;", "don't enter it on the phone.")
    placed by `fit`. `text_cols` is the visible width of the longest text line, measured with the link's visible
    text (not its escapes).

- [ ] **Step 1: Write the failing tests** in `qr.rs`:
  - `the_code_has_its_quiet_zone_and_finders`: `modules("https://slack.com/signin?email=person%40example.com")` is
    square. Rows 0–3 and the last 4 are all light. Module (4,4) to (4,10) is dark (the top-left finder's top
    edge) and (5,5) is light (the finder's inner ring).
  - `two_rows_make_one_line`: `half_blocks(&[vec![true,false], vec![false,true]])` gives one line,
    `"\x1b[30m\x1b[107m▀\x1b[97m\x1b[40m▀\x1b[0m"` (or the agreed SGR order, pinned exactly). Three rows give two
    lines, and the last line's bottoms are light.
  - `the_link_is_its_own_text`: `osc8("https://x.io/a")` is `"\x1b]8;;https://x.io/a\x1b\\https://x.io/a\x1b]8;;\x1b\\"`.
  - `the_screen_fits_the_terminal`: `fit(37, 19, 50, 95, 40) == Beside`, `fit(37, 19, 50, 80, 40) == Under`,
    `fit(37, 19, 50, 30, 40) == LinkOnly` and `fit(37, 19, 50, 95, 20) == LinkOnly`. Also check that
    `code_screen("person@example.com", 30, 40)` contains the OSC 8 link and no `▀`.
- [ ] **Step 2: Run** `cargo test -p klack qr`. Expected: FAIL to compile.
- [ ] **Step 3: Implement** with `qrcode::QrCode::with_error_correction_level(text, EcLevel::M)` and
  `to_colors()`/`width()`. Choosing beside or under is `fit`'s job only.
- [ ] **Step 4: Run** `cargo build`, then `cargo test`. Expected: green, no warnings.
- [ ] **Step 5: Live check:** add a hidden, test-only way to print `code_screen` (an `#[ignore]` test printing to a
  file, then `cat` in the kitty test window under the light and dark kitty themes the window has). Take a
  screenshot with `tools/shot.sh` and look at it. If `zbarimg` is installed, decode the screenshot and compare it
  with `signin_link`; do not install it without asking (machine-installed-packages rule). Check that kitty
  Ctrl+Shift+click on the link works. PROGRESS: say whether it was decoded or only looked at.
- [ ] **Step 6: Commit** `feat: a QR code and the code screen, laid out for the terminal's size (Y2)`.

### Task 3 (Y3): the guide's three ways, and the code way end to end

**Files:**
- Modify: `crates/klack/src/pick.rs` (pre-marked rows)
- Modify: `crates/klack/src/auth.rs` (rows, `Choice`, the guide loop, the code flow, `--email`)
- Modify: `crates/klack/src/main.rs` (`AuthCmd::Login` gains `#[arg(long, value_name = "ADDRESS")] email:
  Option<String>`, passed through `LoginFlags.email`)

**Interfaces:**
- Consumes: Task 1's `signin::{CodeSignIn, CodeTeam, CodeError, signin_link, normalize_code, email_ok}`, Task 2's
  `qr::code_screen`.
- Produces:
  - `pick::choose_marked(question: &str, rows: Vec<Row>, marked: &[usize]) -> Result<Outcome>`: multi-choice with
    those row indices marked. `choose(.., true)` stays and is `choose_marked` with `&[]`. `Picker::with_marks(rows,
    multi, marked: &[usize]) -> Picker` ignores indices that are not enabled items.
  - `pub enum Choice { Candidate(usize, usize), Code, InBrowser, Cookie }` (replaces `Other`)
  - `pub fn rows(groups: &[Group], profile: &Profile, browsers: Browsers) -> (Vec<Row>, Vec<Choice>)`. After the
    groups (and the blank heading when there are any) come exactly these three:
    `item("with a code from your email", "", "phone or any device", true)`, then `item("in your browser", "",
    note, enabled)`, then `item("paste a cookie", "", "", true)`.
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum Browsers { Some, None, OnlyUnreadable }` (computed in
    Task 5; until then the guide passes `Browsers::Some`). `None` gives the note "no browser found here — use the
    code" (dim), and `OnlyUnreadable` gives "Chrome on Windows can't be read — use Firefox, or the code" (dim).
  - `pub struct Hint { pub email: Option<String>, pub team: Option<(String, String)>, pub link: Option<String> }`
    (`Default`): what signing in again knows (Task 7). The guide passes `Hint::default()`.
  - `async fn code_way(hint: &Hint) -> Result<Step<Vec<Result<WorkspaceSession, Failed>>>>`
  - `enum Step<T> { Done(T), Back }`: Back means Esc, and the guide shows its list again. (Not `Way`: that name is
    the `--workspace` ways enum until Task 6.)
  - `fn team_rows(teams: &[CodeTeam], profile: &Profile, team_hint: Option<&str>) -> (Vec<Row>, Vec<usize>)` with
    the marked rows. One usable team, or the hinted team, is pre-marked. An `sso_required` team is dim "needs your
    browser (SSO)", and a `two_factor_required` team is dim "needs your browser (two-factor)" (see Step 8). A team
    the profile already has is noted "already signed in", as in W (matched by `team_id`: the code reply has no
    user id).
  - `fn redeemed(teams: Vec<(String, Result<WorkspaceSession>)>) -> Vec<Result<WorkspaceSession, Failed>>`: the
    team name and its redeem result become `finish`'s input. It is pure, for the Review Focus test.

**The flow** (`code_way`):
1. `pick::ask("Email: ", …)` prefilled with `hint.email`. This needs `pick::ask_with(prompt, initial: &str,
   placeholder)`: Enter on the initial text keeps it. Ask until `email_ok`; Esc gives `Back`.
2. `looking("Asking Slack…", CodeSignIn::start(email))`.
3. Print `qr::code_screen(email, terminal width, height)`. Then `pick::ask("Code: ", |t| if t.is_empty() {
   "___-___".into() } else { String::new() })`. The code is shown as typed (spec). A code that `normalize_code`
   refuses gets "a code is 6 letters or digits, like ABC-123", and the prompt is asked again. Then
   `looking("Checking the code…", s.confirm(code))`: `Failed` prints its message and asks again, `Expired`
   prints its message and asks again (the user asks the phone for a new one), and `RateLimited` or `Other`
   prints its message and gives `Done(vec![])`. The printed screen is erased when leaving the step
   (`MoveToPreviousLine(n)` + `Clear(FromCursorDown)`, n = lines printed; skip the erase if the screen was taller
   than the terminal).
4. `looking("Finding your workspaces…", s.workspaces())`. If there are none: "no workspace signed in with
   <email> — sign up in Slack first" and `Done(vec![])`. Then `pick::choose_marked("Sign in to:", team_rows…)`.
   Esc gives `Back`.
5. Each marked team, one after another, through `looking("Signing in to <name>…", s.redeem(t))`, then
   `Done(redeemed(...))`.

`guide` becomes a loop. It shows the list. Esc there leaves (nothing saved by this pass), and Enter takes the
marked items. Browser candidates are saved first with `finish`, then each marked way runs in list order. A way
that returns `Back` shows the list again (rebuilt, so newly saved accounts read "already signed in"), and the
guide ends when the ways are `Done`. The function returns the total saved. `InBrowser` and `Cookie` call Task 4's
and Task 5's functions. Until those land, `InBrowser` prints "not yet" and `Cookie` keeps W's prompt then
`ask_workspace`.

Non-interactive `auth login --email ADDRESS`: print `signin_link` and "type the code Slack emails you:" to stderr,
then read one line from stdin, confirm it, and redeem every team that is not dim. `--email` in a terminal runs
`code_way` directly with `Hint { email }`.

- [ ] **Step 1: Write the failing tests:**
  - `pick.rs` `marked_rows_start_marked`: `Picker::with_marks(sample rows, true, &[4, 0, 2])` has only row 4
    marked (0 is a heading, 2 is dim), and Enter gives `Picked([4])`.
  - `auth.rs` `rows_put_headings_failures_and_the_last_row` and the two siblings: update the expected tails to the
    three way rows (`- with a code from your email |  | phone or any device`, `- in your browser |  | `,
    `- paste a cookie |  | `). Choices end with `[Code, InBrowser, Cookie]`.
  - `the_browser_way_is_dim_without_browsers`: `rows(&[], &p, Browsers::None)` gives the browser row as `(dim)`
    with "no browser found here — use the code". `OnlyUnreadable` gives the Windows text.
  - `code_teams_are_listed_with_why_not`: three teams (plain, `sso_required`, `two_factor_required`) plus one the
    profile has. The rows read `- Acme | acme.slack.com | `, `… needs your browser (SSO) (dim)`, `… needs your
    browser (two-factor) (dim)` and `… already signed in`. Only one is usable, so it is the marked row. With a hint
    `T0000000002`, that one is marked instead.
  - `a_team_that_fails_does_not_stop_the_others` (Review Focus): `redeemed(vec![("Acme", Ok(s1)), ("Widgets",
    Err(anyhow!("Slack did not sign in to Widgets"))), ("Other", Ok(s2))])` gives `[Ok, Err(("Widgets", "Slack did
    not sign in to Widgets")), Ok]`.
- [ ] **Step 2: Run** `cargo test -p klack`. Expected: the new and updated tests FAIL.
- [ ] **Step 3: Implement** `with_marks`/`choose_marked`, `ask_with`, `Browsers`, `Hint`, the rows, the guide
  loop, `code_way`, `team_rows`, `redeemed` and `--email`. `by_way` (the `--workspace` path) is left as it is
  until Task 6.
- [ ] **Step 4: Run** `cargo build`, `cargo test` and `cargo build`. Expected: green, no warnings.
- [ ] **Step 5: Live check WITH THE USER** (the controller does this step, not the implementer). Use a fake home
  with sounds off and no `DISPLAY`/`WAYLAND_DISPLAY` in the env (as over SSH). Run in the kitty test window:
  `env -u DISPLAY -u WAYLAND_DISPLAY HOME=$F … ./target/debug/klack --profile y auth login`. Pick "with a code from
  your email" and type B's address. That is the user's own gmail, which they named for these tests; send it with
  `send-text` from a variable. Take a screenshot of the QR screen. **Ask the user** to scan or open
  the link on the phone and request the code, then to paste it in the chat; send it with `send-text` from a
  variable. Expected: the picker lists rslack-testbed, marked. Enter gives "✓ Signed in to rslack-testbed as …",
  and `auth status` under the fake home says `code to …` and `works`. Also try one wrong code first (`AAA-AAA`):
  the message and the prompt come again. Delete the fake home after. PROGRESS row LIVE: what was seen. NOT: Grid
  orgs, 2FA, SSO.
- [ ] **Step 6: Commit** `feat: sign in with a code from your email — QR for the phone, klack confirms the code (Y3)`.
- [ ] **Step 7 (only if the user has an account with two-factor):** ask them. If they do, try `auth.loginMagic`
  (`magic_token`, `two_factor_pin`, `ssb`) after redeeming, as the spec's Way-1 step 4 says. If it works, make
  the row usable and ask for the 2FA code; if not, keep the dim row. Record either way in PROGRESS. If the answer
  is no, add the row's state to PROGRESS "Known gaps".

### Task 4 (Y4): paste a cookie lists that cookie's workspaces

**Files:**
- Modify: `crates/klack/src/auth.rs`

**Interfaces:**
- Consumes: `session::signed_in_teams_with`, `session::session_from_cookie_with`, `session::web_client`, Task 3's
  `Step`, `Hint`, `team_rows`-style marking via `pick::choose_marked`.
- Produces: `async fn cookie_way(hint: &Hint, profile: &Profile) -> Result<Step<Vec<Result<WorkspaceSession,
  Failed>>>>` and `fn cookie_rows(teams: &[LoggedInTeam], profile: &Profile, team_hint: Option<&str>) ->
  (Vec<Row>, Vec<usize>)`.

**The flow:** `pick::ask_secret(COOKIE_PROMPT)`: Esc gives `Back`. Trim it and drop `d=`. Then
`looking("Asking Slack…", signed_in_teams_with(..))`:
- `Some(teams)` with some teams: the picker. The single team, or the hinted one, is marked, and "already signed
  in" notes are added as in W. Each marked team goes through `session_from_cookie_with`, and each session saved
  gets `SessionSource::Cookie`.
- `Some([])`: "that cookie isn't signed in to any workspace", and the prompt again.
- `None` (the page changed): fall back to `ask_workspace()` + `cookie_session` (W's way).

- [ ] **Step 1: Write the failing test** `a_cookie_s_workspaces_are_offered`. Two `LoggedInTeam`s, one of them in
  the profile, give the rows `- Acme | acme.slack.com | ` and `- Widgets | widgets.slack.com | already signed in`,
  with no mark (two teams, no hint). With one team it is marked, and with a hint of the second team's id the
  second is marked.
- [ ] **Step 2: Run** `cargo test -p klack cookie`. Expected: FAIL.
- [ ] **Step 3: Implement** and wire `Choice::Cookie` to it.
- [ ] **Step 4: Run** `cargo build`, `cargo test` and `cargo build`. Expected: green.
- [ ] **Step 5: Live check** (no user needed). Use a fake home and the guide's "paste a cookie". B's `cookie_d` is
  read from `~/.config/klack/b/session.json` with python into a shell variable and sent with `kitty @ send-text`,
  never printed. The input shows only "N characters". Expected: rslack-testbed marked, Enter gives "✓ Signed in
  … as" B, and `auth status` says `cookie`, `works`. Delete the fake home.
- [ ] **Step 6: Commit** `feat: a pasted cookie lists the workspaces it is signed in to (Y4)`.

### Task 5 (Y5): "in your browser" — a link to open, and klack watching the browsers

**Files:**
- Modify: `crates/klack/src/login_import.rs` (keys read once, apart from looking; `stamp`; Windows Chrome present)
- Create: `crates/klack/src/login_watch.rs` (declare in `main.rs`)
- Modify: `crates/klack/src/auth.rs` (`browser_way`, `Browsers` computed)

**Interfaces:**
- Produces in `login_import`:
  - `pub struct Keys(HashMap<String, Result<Vec<u8>, String>>)`
  - `pub async fn read_keys(profiles: &[BrowserProfile]) -> Keys` (the key half of today's `discover_with`, same
    `KEY_READS` rules)
  - `pub async fn discover_with_keys(profiles: Vec<BrowserProfile>, keys: &Keys) -> Vec<Group>` (the other half).
    `discover_in` becomes `read_keys` + `discover_with_keys`, with the existing tests unchanged and green.
  - `pub fn stamp(profile: &BrowserProfile) -> Option<std::time::SystemTime>`: the newest mtime of the cookie
    database and its `-wal`/`-journal`.
  - `pub fn unreadable_chrome_here() -> bool`: Windows only. True when `%LOCALAPPDATA%\Google\Chrome\User Data`
    exists. Elsewhere `false`.
- Produces in `login_watch`:
  - `#[derive(Clone, PartialEq, Eq, Hash)] struct Seen { profile: PathBuf, team_id: String, user_id: String, cookie:
    u64 }`. `cookie` is a `std::hash` hash of `cookie_d`, never the value.
  - `pub struct Baseline(HashSet<Seen>)` and `pub fn baseline(groups: &[Group]) -> Baseline`. Every `Ok` session
    goes in, plus each failed candidate as `user_id = ""` and `cookie = 0`.
  - `pub fn new_ones(before: &Baseline, now: &[Group]) -> Vec<(usize, usize)>`: the `(group, candidate)` pairs whose
    `Ok` session's `Seen` is not in `before`.
  - `pub const TICK: Duration = Duration::from_secs(2);` and `pub const GIVE_UP: Duration = Duration::from_secs(600);`
  - `pub async fn watch(before: Baseline, keys: &Keys) -> Result<Option<Vec<Group>>>`. It loops every `TICK`. Each
    time it calls `browser_profiles()` (a profile that is new since the start has no stamp, so it counts as
    changed) and `stamp`s each one. Profiles whose stamp changed are looked at with `discover_with_keys`. A group
    with a `problem` at a tick is ignored for that tick, and its stamp is not recorded, so the next tick tries it
    again. When `new_ones` finds anything, it returns `Some(groups looked at)`. It gives `None` on Esc or Ctrl+C
    (read through `crossterm::event::EventStream` in raw mode, under a guard that turns raw mode off on drop) or
    when `GIVE_UP` passes.
- Produces in `auth`: `async fn browser_way(hint: &Hint, groups: &[Group], keys: &Keys) -> Result<Step<Vec<Result<
  WorkspaceSession, Failed>>>>`. `guide` keeps the `Keys` from its own discovery (`read_keys` +
  `discover_with_keys` instead of `discover`) and passes them on, so on macOS the Keychain is asked once.
  `Browsers` is `None` when `browser_profiles()` is empty and `!unreadable_chrome_here()`, `OnlyUnreadable` when
  it is empty and Chrome is there, and `Some` otherwise.

**The screen** (printed, then `watch`): the spec's Way-2 block. The link is `qr::osc8(hint.link or SIGNIN_PAGE)`.
If Chrome can't be read here but other browsers can, add the line "Chrome on Windows can't be read — use Firefox,
or the code". When `watch` gives groups, the picker shows `new_ones`, all marked, each row labelled "<profile label>
— <team> as <user>". Enter saves them (their `SessionSource::Browser` is set by `profile_group`). Esc, a timeout
("nothing new after 10 minutes") or Esc while waiting gives `Back`. The printed screen is erased as in Task 3.

- [ ] **Step 1: Write the failing tests** in `login_watch.rs` (and `login_import.rs` for `stamp`):
  - `a_new_session_is_new`: the baseline from groups `[P1: Acme U1 cookie "x"]`. `now = [P1: Acme U1 "x", Widgets
    U1 "y"]` gives `[(0,1)]`. A changed cookie for Acme U1 (`"z"`) gives that entry. Acme signed out at the
    baseline and signed in now gives that entry.
  - `a_rewrite_without_a_sign_in_offers_nothing` (Review Focus): `now` equal to the baseline groups (rebuilt, same
    values) gives `[]`.
  - `a_profile_made_during_the_wait_is_watched` (Review Focus): a group for a profile dir not in the baseline gives
    its `Ok` candidates.
  - `the_baseline_keeps_no_cookie` : `format!("{:?}", …)` of a `Seen` (derive Debug) holds no cookie text.
  - `stamp_follows_the_wal`: a temp Firefox profile with `cookies.sqlite`. Writing `cookies.sqlite-wal` later makes
    `stamp` newer (set mtimes with `File::set_modified`).
  - `waiting_gives_back_the_terminal` (Review Focus): the raw-mode guard's `Drop` calls the restore function. Test
    it through a guard generic over a `restore: impl FnOnce()` (set a flag), as W5 did.
  - Existing `login_import` discovery tests stay green unchanged.
- [ ] **Step 2: Run** `cargo test -p klack login_`. Expected: FAIL to compile.
- [ ] **Step 3: Implement** the split, `stamp`, `login_watch` and `browser_way`, and wire `Choice::InBrowser` and
  `Browsers`.
- [ ] **Step 4: Run** `cargo build`, `cargo test` and `cargo build`. Expected: green.
- [ ] **Step 5: Live check** (no user needed). The fake HOME holds a throwaway Firefox profile
  (`$F/.mozilla/firefox/profiles.ini` + `kt.default/cookies.sqlite` with an empty `moz_cookies`). Start the guide
  and pick "in your browser". The screen and link show, and `send-text` Ctrl+Shift+click is not needed. Then, from
  another command in the SAME bash invocation (background processes die with their shell), insert into the
  profile's `cookies.sqlite` a `d` row for `.slack.com` holding B's cookie (python, never printed). Within about
  4 s the picker shows "Firefox · default — rslack-testbed as …", marked. Enter saves it, and `auth status` says
  `Firefox · default`. Second run: Esc while waiting brings the list back and the terminal echoes normally. Delete
  the fake home. NOT: a real browser writing its cookies (Chrome's flush delay), macOS Keychain (Task 8).
- [ ] **Step 6: Commit** `feat: sign in in your own browser — klack prints the link and picks the new sign-in up (Y5)`.

### Task 6 (Y6): the throwaway window goes

**Files:**
- Delete: `crates/klack/src/login_browser.rs` (and `mod login_browser;` in `main.rs`)
- Modify: `crates/klack/src/main.rs` (drop `--no-browser`, and reword `Login`'s doc comments: no browser window,
  the three ways, `--email`)
- Modify: `crates/klack/src/auth.rs` (drop `window_then_cookie`, `window_session`, `Way::Window`, `WINDOW*`, and
  `no_browser` in `LoginFlags`; `by_way` for `--workspace` lists the browser profiles signed in to that workspace,
  then the three ways with `Hint { team: Some(that workspace), link: Some(its url) }` resolved after the sign-in)
- Modify: `crates/klack/Cargo.toml` (drop `tokio-tungstenite` if `grep -rn tungstenite crates/klack/src` is empty)
- Keep: `SessionSource::Window` (loads, shown "a browser window"), and `login-browser` in `profiles::RESERVED` and
  purge.

**Interfaces:**
- `ways(groups, url, browsers: Browsers) -> (Vec<Row>, Vec<Way>)`. The existing `Way` enum loses `Window` and
  gains `Code` and `InBrowser`: `enum Way { Browser(usize, usize), Code, InBrowser, Cookie }`. With `--workspace
  acme`, the code, cookie and browser ways mark the acme team by host in their pickers (`team_hint` by host as well
  as id).
- Non-interactive `auth login --workspace W` with no cookie reads the cookie from stdin (the window step is gone).

- [ ] **Step 1: Update the failing tests:** in `ways_list_the_browsers_signed_in_to_that_workspace_first`, the
  rows after the browser profiles are the three ways, with no window row and no `KLACK_BROWSER` text.
  `source_label(Some(&Window)) == "a browser window"` stays. Add
  `an_old_window_session_still_loads`: JSON with `"source":{"kind":"window"}` deserialises in `session.rs`.
- [ ] **Step 2: Run** `cargo test`. Expected: FAIL (old rows).
- [ ] **Step 3: Implement** the removal. Then run `grep -rn 'login_browser\|no_browser\|KLACK_BROWSER\|new browser
  window' crates` and check the hits are none, or only comments you rewrite.
- [ ] **Step 4: Run** `cargo build`, `cargo test` and `cargo build`. Expected: green, no warnings, and the
  `Cargo.lock` change is limited to the dropped dependency.
- [ ] **Step 5: Live check:** `./target/debug/klack auth login --help` (no `--no-browser`, `--email` shown). In a
  fake home, `--workspace rslack-testbed` in a terminal lists the three ways, and the cookie way with B's cookie
  marks rslack-testbed. Old: a fake-home `session.json` with `"source":{"kind":"window"}` loads in `auth status`
  as "a browser window".
- [ ] **Step 6: Commit** `feat: the throwaway browser window is gone — your own browser, a code or a cookie (Y6)`.

### Task 7 (Y7): signing in again starts from where the session came from

**Files:**
- Modify: `crates/klack/src/auth.rs` (`sign_in_again`, and `guide_with(profile_name, hint: Hint)` used by both)

**Interfaces:**
- `pub async fn guide(profile_name) -> Result<usize>` becomes `guide_with(profile_name, Hint::default())`.
- `sign_in_again(profile_name, ws)`:
  - `Some(Code { email })` runs `code_way(&Hint { email: Some(email), team: Some((ws.team_id, ws.user_id)), link:
    None })` first. `Done` gives `finish`, and `Back` gives the full guide.
  - `Some(Browser { .. })` gives W's one-row question first (unchanged). If that is refused or the profile is
    signed out, `browser_way` runs with `Hint { link: Some(ws.url), team: … }` (the workspace's own address;
    check live that it lands on its sign-in page). `Back` gives the full guide.
  - Anything else (`Window`, `Cookie`, `None`) gives the full guide.
- A pure `fn again_hint(ws: &WorkspaceSession) -> (First, Hint)` with `enum First { Code, BrowserProfile, Guide }`
  for the test.

- [ ] **Step 1: Write the failing test** `signing_in_again_starts_where_it_came_from`. A `Code` session gives
  `(Code, Hint { email: Some("person@example.com"), team: Some(ids), link: None })`. A `Browser` session gives
  `(BrowserProfile, link = ws.url)`. `Window`, `Cookie` and `None` give `Guide`.
- [ ] **Step 2: Run** `cargo test -p klack again`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo build`, `cargo test` and `cargo build`. Expected: green.
- [ ] **Step 5: Live check.** Use a fake home whose B session (source `Code { email: B's }`) has its `token`
  replaced by `xoxc-0-0-0-invalid` (python), so `auth.test` says `invalid_auth`. Plain `./target/debug/klack
  --profile y` asks about the signed-out workspace, and "sign in again" opens the code way with the email filled
  in. **Needs a code from the user**: the controller asks once, ideally batching this with Task 3's live check
  when they are done one after the other. If the user isn't there, check up to the code prompt and mark the rest
  NOT. Browser source: a throwaway Firefox profile whose cookie is invalid offers the browser way with the
  workspace link (seen on screen), and Esc brings the full list.
- [ ] **Step 6: Commit** `feat: signing in again offers the way the session came from first (Y7)`.

### Task 8 (Y8): docs, and the other systems

**Files:**
- Modify: `README.md` (the sign-in section: the guide's list, the three ways, `--email`, `--cookie` /
  `$KLACK_COOKIE_D` + `--workspace` for scripts, no browser window, nothing signed out)
- Modify: `DESIGN.md:36-37`, `CONTRIBUTING.md` (any sign-in or live-test lines about the window)
- Modify: `docs/protocol/auth.md` (a new section "Email-code sign-in without a browser (verified 2026-10-20, Y)": the
  spec's five-step sequence, and that only sending the code needs the captcha. Its last line "so an emailed-code
  sign-in cannot be done without a browser" is corrected to point to it)
- Check: `crates/klack/src/tui/prefs_help.rs` and the `--help` texts for "browser window" (`grep -rn -i 'browser
  window' crates docs README.md`)

- [ ] **Step 1:** Write the docs. Then run `grep -rn -i 'no-browser\|browser window\|KLACK_BROWSER' README.md
  DESIGN.md CONTRIBUTING.md docs/protocol crates`. Expected: only historical mentions (PROGRESS, old specs/plans)
  remain.
- [ ] **Step 2: Windows VM** (`virsh domstate win11-dev` first, and restore that state after). Build the branch
  there as W did (tools / CONTRIBUTING recipe). Check that `klack auth login` in a fresh profile under a temp
  `%APPDATA%` lists the three ways, that "in your browser" is dim with the Windows Chrome text when only Chrome is
  installed, and that the code way reaches the QR screen. Esc out, with no code requested.
- [ ] **Step 3: Mac** (`macbook`, same power-state rule). Check that the code way reaches the QR screen, and that
  the browser way's waiting screen asks the Keychain at most once per browser for the whole guide (the user
  answers the dialog, or it is denied, and both are fine). Mark a real code on the Mac NOT unless the user offers
  one.
- [ ] **Step 4:** `cargo build`, `cargo test` and `cargo build`, then the PROGRESS row. The controller also
  updates the "Next" section: Y done, NEXT UP the rest of V.
- [ ] **Step 5: Commit** `docs: sign in anywhere — README, design, auth protocol; checked on Windows and macOS (Y8)`.

---

## After the last task

A final whole-branch review against the spec (constraints: no secret printed anywhere, `grep -rn
'println\|eprintln\|tracing::' crates/klack/src/auth.rs crates/slack-core/src/signin.rs crates/klack/src/login_watch.rs`
read line by line). One fix wave, then the controller's PROGRESS/push/CI and deleting the SDD ledger. **No
release** unless the user says so.

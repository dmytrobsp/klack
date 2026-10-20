# Links into your Slack open in klack (queue Z) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan
> task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A link in a message that points into one of the profile's workspaces is shown as `↪ <readable label>` in the
accent colour and opens inside klack on a plain click or through `o`. Ctrl+Shift+click still opens the browser.

**Architecture:** A new pure module `crates/klack/src/tui/applink.rs` parses Slack links into an `AppLink` and matches
them to the profile's workspaces. Rendering (`text.rs` `inline_spans` / `links_shown`, `messages.rs` `link_spots`)
gives in-app links their label and style while the real URL stays the OSC 8 hyperlink. Opening reuses the
search-result path (`open_hit` / `show_hit` / `continue_seek`) and, for another workspace, the notification-click path
(`open_after`). A small cache of linked messages' authors (`applink::Authors`) fills "message from Bob".

**Tech Stack:** Rust 2024, ratatui 0.30, crossterm 0.29, reqwest via slack-core's `Client`. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-10-20-in-app-links-design.md`. Read it whole. "(user)" decisions are not to be
changed.

## Global Constraints

- Label forms, verbatim from the spec: `↪ #deploys`, `↪ #deploys › message from Bob`, `↪ DM with Bob › message`,
  `↪ #backend › thread reply` (or `› reply from Bob`), `↪ Bob's profile`, `↪ file report.pdf`,
  `↪ acme · #deploys › message`; a labelled link keeps the author's text as `↪ the fix`; the `o` menu groups "In klack"
  then "In the browser", and shows a labelled in-app link as `↪ the fix — #backend › thread reply`; the hover note is
  `↪ opens #backend › thread reply in klack`, or the URL for an external link.
- In-app links use `theme::p().accent`. External links are unchanged (`p().link`, underlined). The OSC 8 hyperlink under
  an in-app label is the real URL.
- A plain left click with no modifiers opens in klack. A click elsewhere on a message does what it does today.
- Not found → stay in the conversation, with the status note "that message isn't there any more, or you can't see it".
  Never open the browser by surprise.
- Matching: exact host (`<ws>.slack.com`) against the profile's workspaces' URLs, or exact team id for
  `app.slack.com/client/<T…>/…`. No Enterprise Grid.
- No new system or terminal configuration. Never print tokens or cookies. Never quote real names or ids in PROGRESS
  (A, B, the A–B DM).
- House routine per task: `cargo build` (no warnings) → `cargo test` green (never commit on red) → `cargo build` →
  live check → PROGRESS row INSIDE the table after the last row (`| Z<n> — … | done | UNIT …; LIVE …; NOT … |
  (this commit) |`, check with python that it landed in the table) → ONE commit `feat|fix: … (Z<n>)` ending with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. The controller pushes.

## Review Focus

- **Links with odd shapes:** a trailing `/`; extra query parameters; a `p`-ts with fewer than 16 digits; an uppercase
  host; `http://`; a link wrapped by Slack in `<…|…>`; the link inside a code block (stays plain text, not a link).
  Each parses or is rejected cleanly, with no panic. Test in Task 1.
- **A long label that wraps across lines, or a label next to other text:** the click and the hover still find it,
  because `link_spots` matches the drawn pieces to the label's letters. Test in Task 2.
- **The linked message is far back or deleted:** the seek pages back, then stops at the start of history with the
  not-found note. Ctrl+C or leaving the conversation drops the seek. Test in Task 3 (the pure part of the stop rule).
- **Many links on screen at once (a long list of permalinks):** author fetches are limited (at most 3 in flight,
  each link fetched once) and never block drawing. Test in Task 5.
- **A link to the workspace of an account the profile has twice** (two accounts of one team): the shown account's
  workspace wins. Test in Task 1.

---

### Task 1 (Z1): parse and match Slack links

**Files:** Create `crates/klack/src/tui/applink.rs` (declare in `tui/mod.rs`).

**Interfaces — Produces:**
- `pub enum Target { Conversation { channel: String }, Message { channel: String, ts: String, thread_ts: Option<String> }, Person { user: String }, File { file: String } }` (Debug, Clone, PartialEq, Eq)
- `pub enum Team { Host(String), Id(String) }`
- `pub struct AppLink { pub team: Team, pub target: Target }`
- `pub fn parse(url: &str) -> Option<AppLink>` covers:
  - `https://<host>/archives/<ch>`, `…/archives/<ch>/p<digits>` (ts = all but the last 6 digits `.` the last 6), and
    `?thread_ts=<ts>` on that;
  - `https://app.slack.com/client/<T…>/<ch>`, plus `…/thread/<ch>-<ts>`;
  - `https://<host>/team/<U…>` and `https://<host>/files/<U…>/<F…>[/…]`.
  - The host is lower-cased and must end in `.slack.com`; `http://` is taken too. Anything else is None.
- `pub fn workspace_of(link: &AppLink, workspaces: &[(String /*url*/, String /*team_id*/)], shown: usize) -> Option<usize>`:
  the index of the matching workspace, preferring `shown` when several entries match (two accounts of one team).
- `impl From<super::notify::Target> for Target`, and `Target::channel(&self) -> Option<&str>`.

- [ ] **Step 1: failing tests** `parses_every_form`:
  - `https://acme.slack.com/archives/C0123` → Conversation;
  - `…/archives/C0123/p1729440000123456` → Message ts `1729440000.123456`, no thread;
  - the same with `?thread_ts=1729430000.000100&cid=C0123` → thread_ts;
  - `https://app.slack.com/client/T0001/C0123/thread/C0123-1729430000.000100` → Message with thread;
  - `/team/U0001` → Person; `/files/U0001/F0001/report.pdf` → File;
  - `HTTPS://ACME.SLACK.COM/archives/C0123/` → Conversation.

  `rejects_what_is_not_a_slack_link`: `https://example.com/archives/C1`, `https://acme.slack.com/help`,
  `https://acme.slack.com/archives/`, `…/p12` (too short), `mailto:x@slack.com` → None.

  `matches_the_workspace`: by host, by team id, none; two entries with the same team id → `shown` if it is one of them.
- [ ] **Step 2:** `cargo test -p klack applink` → FAIL (no module).
- [ ] **Step 3:** implement. No regex is needed: split on `/`, `?` and `&`.
- [ ] **Step 4:** `cargo test -p klack applink` → PASS.
- [ ] **Step 5:** no live check (pure). PROGRESS row: UNIT only. Commit `feat: parse Slack links and match them to the profile's workspaces (Z1)`.

### Task 2 (Z2): show in-app links as `↪ label`

**Files:** `crates/klack/src/tui/text.rs` (`inline_spans` Link arm ~173, `links_shown` ~182),
`crates/klack/src/tui/messages.rs` (`link_spots` ~600: recognise accent-coloured underlined pieces too; `LinkSpot`
gains `app: Option<applink::AppLink>`), `crates/klack/src/tui/applink.rs` (labels).

**Interfaces:**
- Consumes Task 1.
- Produces:
  - `pub struct Here<'a> { pub ws: &'a Workspace, pub workspaces: &'a [(String, String)], pub names: &'a [String], pub shown: usize, pub authors: &'a Authors }`;
  - `pub fn label(link: &AppLink, here: &Here, author_text: Option<&str>) -> Option<String>`, which is None when the
    link is not in-app;
  - `pub struct Authors` (a cache: `get(&self, channel, ts) -> Option<&str>`; filled in Task 5). It is empty until
    then, so labels read `› message`.
  - `inline_spans` and `links_shown` take `&Here` (thread it from their callers). In-app links draw `↪ <label>` in
    `p().accent` + UNDERLINED, so `link_spots` keeps finding them by style.

Label rules (the spec's forms):
- **Conversation:** `#name` from `ws.channel_label`, or `DM with <name>` / `group DM`.
- **Message:** `› message`, or `› message from <author>` when `Authors` or the message's own link-preview attachment
  names the author. For the attachment, find an attachment whose `from_url`/`original_url` equals the link and use its
  `author_name`, or the name of `author_id`.
- **Reply** (thread_ts set and ts ≠ thread_ts): `› thread reply`, or `› reply from <author>`.
- **Person:** `<name>'s profile`. **File:** `file <name>` when the message carries that file, else `a file`.
- **Another workspace:** prefix `<workspace name> · `. Its channel name is only known if the current `Workspace`
  happens to have it (no extra connection), else `a channel`.
- **Author label:** an author's own label replaces the readable label but keeps `↪ `.

- [ ] **Step 1: failing tests** `in_app_links_are_labelled` (pure `label`):
  - each target form, the DM form and the other-workspace prefix;
  - an author label (`↪ the fix`);
  - `› message from Bob` from an attachment preview;
  - an unknown channel;
  - an external link → None.

  `an_in_app_link_is_found_where_it_is_drawn`: a message with `see <https://acme.slack.com/archives/C1/p…|>` drawn at a
  width that wraps the label. `link_spots` finds every piece with the real URL and `app: Some(..)`, and the existing
  `links_are_found_where_they_are_drawn` still passes.
- [ ] **Step 2:** run them → FAIL.
- [ ] **Step 3:** implement and thread `Here` through the callers. The OSC 8 `hyperlink` call stays with the real URL.
- [ ] **Step 4:** green, no warnings.
- [ ] **Step 5: live:** in a fake home, B posts to the A–B DM a channel link, a message permalink, a reply permalink
  (from a thread in the DM) and A's profile link, built from the test workspace's URL. Screenshot A's view: four `↪`
  labels in the accent colour. Delete the posts. PROGRESS, commit `feat: links into your Slack show as ↪ and where they go (Z2)`.

### Task 3 (Z3): a plain click opens it in klack

**Files:** `crates/klack/src/tui/event_loop.rs` (the messages and thread panes' left-click handling; `open_hit` /
`show_hit` / `continue_seek`; `open_after` / `switch_workspace`; the profile card; the file viewer).

**Interfaces:**
- Consumes `LinkSpot.app`, `applink::Target`, and `workspace_of`.
- Produces:
  - `fn open_app_link(&mut self, link: &AppLink)` on `App`;
  - `open_after` widened from `notify::Target` to `applink::Target` (notifications convert with `From`).

Behaviour:
- A left Down with no modifiers on a `LinkSpot` with `app: Some(_)` calls `open_app_link`. Any other click is
  unchanged.
- **Same workspace:**
  - Message / reply: build a `search::Hit` and call `open_hit`.
  - Conversation: what Jump to does (`jump_to`, or the preview for a channel you are not in).
  - Person: the profile card (find how `Action::Profile` opens it).
  - File: the viewer where `open_things` has one for that file. Otherwise the open menu's entry, or the note "klack
    can't show that file here".
- **Another workspace:** set `open_after = Some((index, target))` and `switch_workspace(index)`. The switched-to run
  opens it as a notification target is opened today (extend that code for Person/File).
- **Not found:** when `continue_seek` reaches the start of the history (or the conversation can't be opened) for a
  link-started seek, set the spec's note. Add a `seek_from_link: bool` (or similar) so search behaves as before.

- [ ] **Step 1: failing tests:**
  - `a_click_on_a_link_finds_its_target`: the pure part, mapping a click (column, row) through `link_spots` to the
    `AppLink`; a click beside it → None.
  - `a_seek_that_reaches_the_start_says_so`: the pure stop rule → the note.
  - `notify_targets_still_open`: the `From` conversion.
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** green.
- [ ] **Step 5: live:**
  - B posts the four links again, plus a permalink to a message far back in #history-paging (the test channel with
    long history) and one to a deleted message.
  - A clicks each and lands right: a channel; a message selected; a thread opened with the reply selected; the profile
    card; the far-back message after paging; the not-found note.
  - With a two-workspace fake profile (A + B, the recipe in the lessons), a link to the other entry switches and opens it.
  - Delete the posts.
  - PROGRESS, commit `feat: a plain click on a link into your Slack opens it in klack (Z3)`.

### Task 4 (Z4): the open menu groups in-app links, and the hover note

**Files:** `crates/klack/src/tui/open.rs` (`Thing::Link` gains `app: Option<AppLink>`, `label()`), the open picker /
`open_things` in event_loop.rs, and the mouse-move handling over the messages and thread panes.

- **The `o` menu:**
  - In-app links first, under a heading "In klack", each shown `↪ <label>`, or `↪ <author label> — <readable label>`.
  - Then the rest under "In the browser", with pictures and files as today.
  - A single thing that is an in-app link opens in klack at once.
  - Choosing an in-app row calls `open_app_link`.
- **Hover:** the mouse over a `LinkSpot` sets a hover note in the status bar's note slot: `↪ opens <label> in klack`, or
  the URL without `https://` for an external link. It clears when the mouse leaves the link. Use the slot status
  notes use, without overwriting a real status message that arrived later (builder's call how; say which in the report).

- [ ] **Step 1: failing tests:**
  - `the_open_menu_groups_in_app_links` covers the order, the headings, the labels and the single-in-app shortcut.
  - `hovering_a_link_names_where_it_goes` covers in-app, external and leaving.
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** green.
- [ ] **Step 5: live:**
  - `o` on a message with an in-app and an external link shows the two groups.
  - The hover note appears and clears.
  - Ctrl+Shift+click still reaches kitty: launch the test kitty with `-o open_url_with=<a scratch script that appends
    the URL to a file>`, Ctrl+Shift+click the label (`kitty @ send-text` can't do that, so use a real
    `kitty @ … send-mouse` if available, or note it as NOT checked), and the script logs the real URL.
  - PROGRESS, commit `feat: the open menu puts links into your Slack first, and a hover says where a link goes (Z4)`.

### Task 5 (Z5): "message from Bob" — the linked message's author

**Files:** `crates/klack/src/tui/applink.rs` (`Authors`), event_loop.rs (fetch on draw), slack-core `api.rs` (reuse the
history and replies calls: one message at a ts, `inclusive=true`, `limit=1`).

- `Authors` maps `(channel, ts)` to `Known(name) | Pending | Missing`. When a frame draws in-app message links whose
  author is unknown, klack asks for at most 3 at a time. Each link is asked for once per run. The answer updates the
  cache and triggers a redraw. Missing (deleted, no access) stays `› message`.
- Links to another workspace are never fetched (spec).

- [ ] **Step 1: failing tests:**
  - `authors_are_fetched_once_and_few_at_a_time`: the pure scheduler. Ten links on screen → 3 requests, the rest
    queued, never twice.
  - `a_missing_message_keeps_the_plain_label`.
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** green.
- [ ] **Step 5: live:** B posts a permalink to an old message of B's. A's label turns into `› message from <B's name>`
  shortly after drawing. Delete the post. PROGRESS, commit `feat: a link to a message names who wrote it (Z5)`.

### Task 6 (Z6): docs

- README: a short section "Links into your Slack". It covers the `↪` label, a plain click opens in klack, the `o`
  menu groups, and Ctrl+Shift+click for the browser. Also update the F6/help text of the open action if it describes
  links.
- PROGRESS "Next": queue Z done.
- PROGRESS, commit `docs: links into your Slack open in klack (Z6)`.

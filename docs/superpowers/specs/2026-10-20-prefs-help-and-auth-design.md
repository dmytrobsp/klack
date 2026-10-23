# F6 contextual help, guided sign-in with `auth`, profiles and `purge` — design (queue W)

Status: brainstormed with the user on 2026-10-20; builder check done, its three open questions answered by the user (Claude's recommendations). Awaiting the user's read-through. "(user)" marks their decisions, "(Claude's call)" marks choices
made for them that they did not object to when played back, "(builder's call)" is left to the implementer. Anything
unmarked follows from the user's request or from the code as it is.

Out of this design (tracked separately): the notification-schedule flicker the user found on 2026-10-20 (switching the
kind every day → weekdays → custom → off shows the previous value for a moment) is a bug, fixed as its own step
before W starts; the rest of queue V comes after W.

## Why

- F6's help is "a solid wall of text where it was really hard to discern where one option description ends and
  another one starts" (user); the worst is Availability → Notification schedule → custom.
- Signing in needs `--help` and flags; the temporary browser window means "none of my google sign-in worked", "email
  sign-in ran me through captcha every time, sometimes for minutes", and Chrome asked to "set up a profile" (user).
- There is no way to list profiles, pick a default (the user types `--profile test` every time), or remove one without
  "hunting their folders down in the filesystem" (user) — e.g. the test profiles when W is done, or the stray
  `~/.cache/klack/asd` a mistyped `--profile asd` left.
- No way to wipe everything klack wrote ("as if the app was never launched", user).

## Part 1 — contextual help in F6

### What the user sees
- The cursor's item (the selected row, not the hovered one — Claude's call; hovering only highlights, as today; a
  first click selects a row and so shows its help, a click on the selected row acts as Enter, as today) has its own help, in **a box under the
  list** (user: option A of the mockups), titled with the item's name, replacing today's dim per-category hint block
  (`fixed` / `hints` in `Prefs::draw`, crates/klack/src/tui/prefs.rs ~1112–1144). The rows keep the full width.
- Each entry says (user): **what the option does**, **what its values mean** (a list when there are several), and
  **how to set it** when that is more than Enter toggling it. "Neatly formatted" (user): line breaks and lists where
  they help, never one long paragraph.
- Layout of an entry (from the mockup the user picked): one or two plain lines of description; a blank line; a list of
  values `• name   meaning` with the names bold and aligned; a blank line; key lines `Enter / + -   change the kind`
  with the keys in the accent colour. Sections that don't apply are left out (an on/off item is a line or two, no
  "how to set" — Claude's call).
- The keys named in a help are the user's **current** bindings (from the keymap, Preferences context), not defaults
  (Claude's call), so the text stays right after rebinding.
- The box is rounded, dim-bordered, indented like the rows. When the list is long the box shrinks; a help that does
  not fit is cut (last line ends in "…"); the rows are never covered (Claude's call). **Minimum height** (user): below
  room for 3 lines of text plus the borders (or the whole help, if it is shorter — Claude's call on the number) the
  box is not drawn at all.

### Which items get help
- Every Field in every category, Keys' settings above the bindings included (which-key, leader, insert leader).
- The custom schedule's **day rows** get their own help (hours as 9:00-17:00, "receive all day", "paused all day",
  what going back to hours does) — today's worst offender, split per row (Claude's call).
- **Keys category** (user approved the mockup):
  - a binding row: the full description (the row's copy is often cut); where it works (Global / the context title);
    its keys now; the default keys only when changed; its keys.toml name (`ActionInfo::name`, e.g. `jump_to`); then
    `Enter` record new keys (until a 1 s pause; any key counts), `Delete` unbind, `r` reset to the default.
  - a group row: what the group holds; `Enter` renames it, `i` changes its icon.
  - the filter line: what `/` matches; Enter keeps the filter, Esc clears it.

### Text and translation
- Help texts are English strings that go through `i18n::tr` like the rest, so `ctl locale-template` lists them
  (Claude's call). Keep the formatting outside the translated strings where practical (the bullet, alignment and key
  colouring are drawn by code; the strings are the sentences and value meanings) — builder's call how to structure it.
- Writing the text for every item is part of the work: accurate to what the code does now (check each against the
  Field's behaviour, e.g. which settings are "kept on Slack", what +/- step through).

## Part 2 — `auth`: guided sign-in, status, logout

### Commands
- `klack login` is replaced by **`klack auth login | logout | status`** (user). `klack login` is **removed outright**
  (user): no alias, no pointer; clap's usual "unrecognized subcommand" answer is all.
- All act on `--profile` if given, else on the **default profile** (Part 3). Signing in under a new profile name
  creates that profile; there is no separate "create" (Claude's call).
- The old flags stay on `auth login` for scripts (Claude's call): `--workspace URL`, `--cookie`, `--no-browser`,
  `--from-browser firefox|chrome`, `--browser-profile DIR`. With enough flags nothing is asked; when stdin/stdout is not
  a terminal nothing is ever asked (an error says which flag is missing). In a terminal, `--workspace` alone skips the
  workspace question and shows the list of ways for that workspace (browser profiles first) instead of opening the
  window at once as `login` did (Claude's call: the window is now the fallback); `--from-browser`, `--no-browser`,
  `--cookie` / `$KLACK_COOKIE_D` pick the way and skip that list. Without a terminal, `--workspace` alone keeps today's
  behaviour (the window, then the cookie).

### When the guide runs
- `klack auth login` without flags, or plain `klack` on a profile with **no sign-ins** (user). The guide's first line
  names the profile: `Signing in profile default.` For a profile that doesn't exist yet (e.g. a typo):
  `Profile asd has no sign-ins yet.` — Esc leaves **without creating any folder** (Claude's call; today any name
  creates `~/.cache/klack/<name>` via `init_logging` — the guide must not, and a plain run that ends in Esc must not
  leave folders either).
- Plain terminal questions before the TUI starts, no full-screen UI (user: "we dont need fancy TUI for this, just a
  guided process that doesnt require the user to scour through --help").

### The list (user: browsers first, option A of the mockups)
1. klack finds every Slack sign-in in the user's browsers and lists them **grouped by where they come from**, so the
   user "can clearly distinguish which login comes from where" (user). A group heading names the browser and its
   profile as the user knows it: `Chrome · Work  dmytro@acme.com` (Chrome's profile display name and Google account,
   from Chrome's `Local State` → `profile.info_cache`), `Firefox · default-release` (profiles.ini name). Under it, one
   row per workspace: name, address dim, `as <user name>`.
2. How the workspaces are found (verified on 2026-10-20 with test account A, read-only): one `d` cookie can be signed
   in to several workspaces; `GET https://slack.com/signin` with that cookie returns a page whose (HTML-escaped) JSON
   has `"loggedInTeams":[{team_id, team_name, team_domain, team_url, user_email, …}]`. For each team, the existing
   `session_from_cookie(team url, cookie)` (customize/emoji → api_token, then auth.test) checks it and gives the user
   name. These checks run concurrently; a short "Looking for Slack in your browsers…" line shows meanwhile.
3. Rows that fail the check (signed out in that browser, expired) are **shown dim and cannot be picked** with the
   reason: `acme — signed out in Chrome · Work` (Claude's call; the user raised that a lifted cookie may be expired).
   Workspaces already in this profile are marked `already signed in`; picking one refreshes its session (Claude's
   call).
4. The last row: `another workspace, or sign in another way…` →
   - `Workspace: ` prompt where **`.slack.com` is shown dim after what is typed as a placeholder** (user), so it's
     obvious it is filled in; typing a dot or a full domain drops the suffix; a pasted URL from the browser is accepted
     (user) — `https://acme.slack.com/…`, `acme.slack.com`, `app.slack.com/client/T…` (builder's call how far to go
     with the last; at least the first two). Enterprise Grid domains (`acme.enterprise.slack.com`) pass through as is.
   - then a list of ways: the browser profiles signed in to that workspace (if any), `in a new browser window` with its
     downside said (`fresh — Google won't know you; may ask for a captcha`), `paste the d cookie by hand`.
   - The browser window stays a temporary profile deleted afterwards, as today — **no persistent browser profile owned
     by klack** (user). Claude's call: start it with the flags that suppress Chrome's first-run, sync and "set up a
     profile" sign-in prompts (e.g. `--no-first-run`, `--no-default-browser-check`, `--disable-sync`, and disabling the
     sign-in interception feature) — builder verifies which flags actually stop the prompt the user saw.
5. Keys (user): ↑ ↓ and j k move (headings and dim rows are skipped), **Space marks several, Enter signs in to all
   marked** (or the cursor's row if none marked), `/` starts a filter and Esc drops it (only `/` filters — typing
   letters would clash with j/k, user), Esc with no filter leaves the guide. A mouse click on a row **moves the cursor
   to it and toggles its mark** (user), like Space; Enter then signs in to the marked ones.
6. Drawn inline in the terminal (crossterm raw mode, ratatui's inline viewport or plain crossterm — builder's call; no
   new dependency); when done the list is erased and only result lines stay:
   `✓ Signed in to Acme Corp as A — profile default saved.` (one per workspace; a failure says why).
7. Reading Chrome's key: Linux Secret Service, macOS Keychain (macOS asks once). Windows Chrome encrypts its cookies so
   only Chrome can read them — Chrome isn't listed on Windows (Firefox, the window and the cookie are). A browser whose
   key can't be read (locked keyring, refused) is listed as a dim heading with the reason, not an error that stops
   the guide.

### Remembering where a session came from (user: "remember the source and offer it first")
- Each saved `WorkspaceSession` gets a **source**: `{ browser: chrome|firefox, profile: <folder>, label: "Chrome ·
  Work" }`, or `window`, or `cookie`. Names and paths only — never anything secret. Old session.json files without it
  load as "unknown".
- Re-login of a workspace offers that source first as a one-row question:
  `Chrome · Work is still signed in to acme — use it?` (Enter yes, Esc → full list). If that source has expired too:
  `Chrome · Work is signed out of acme as well.` and straight to the full list (user's edge case).

### Expired sessions
- Facts (checked 2026-10-20 on test account A, read-only): Slack does **not** send a renewed `d` cookie on klack's
  requests (auth.test, customize/emoji, app.slack.com — only `d-s`, a "last seen" stamp, and tracking cookies come
  back). A session lives until Slack, an admin's session-length policy, a password change or a sign-out ends it.
  There is nothing to refresh; detecting the end is the fix.
- **At start** (user): klack checks each workspace of the profile (`auth.test`, concurrently, with a short timeout;
  a network failure is "can't check", not "expired" — launch as normal). For each expired one it warns and asks, with
  the same list UI: **sign in again** (the source first, as above), **remove it from the profile**, **launch anyway**.
  With "launch anyway" (user, 2026-10-20, Claude's recommendation): klack **skips** the signed-out workspace and opens
  the first that works (`--team` / `default_team` first if it works); the signed-out one is marked in the Alt+1…9
  rail; switching to it does not switch — the status line says `acme: signed out — klack auth login` and you stay
  where you are. **No "signed out" screen and no client-less mode of `run_workspace`.** If every workspace is signed
  out, "launch anyway" is not offered: sign in again, remove, or quit.
- **While running** (Claude's call): an `invalid_auth` / `not_authed` / `token_revoked` / `account_inactive` answer
  marks that workspace signed out; the status line says `acme: signed out — klack auth login`; other workspaces keep
  working; the RTM/live connection for it stops retrying. If it is the current workspace, what is loaded stays
  readable and nothing new loads or sends (user, 2026-10-20).
- **No live reload** (user, 2026-10-20): a sign-in made from another terminal while klack runs is picked up on the
  next start; the signed-out note says to restart (`… — klack auth login, then restart klack`). The TUI keeps reading
  session.json once at start.

### Several accounts in one workspace (user, 2026-10-20)
- One profile may hold **two accounts in the same workspace** (e.g. acme as you from Chrome · Work and acme as someone
  else from Chrome · Personal, both marked with Space). A session is keyed by **team + user**: `Profile::upsert`
  replaces only the same team+user (today it replaces by team), "already signed in" matches team+user, the rail lists
  both (`workspace_labels` already tells them apart), `auth logout` and the source-first re-login act on a team+user
  entry, and `--team acme` with two accounts there asks which one (or, without a terminal, refuses and lists them).
  `default_team` keeps naming a team; with two accounts in it, the first one saved is started (builder may store
  team+user instead, keeping old files loadable).

### `auth status` and `auth logout`
- `auth status` (Claude's call): one line per workspace of the profile — name, address, who you are, source
  (`Chrome · Work` / `browser window` / `cookie` / `unknown`), `works` or `signed out` (checked live, concurrently);
  the default workspace marked.
- `auth logout` (user: **only forgets the session on this machine**, never signs out on Slack's side — a lifted cookie
  is the browser's own session): with no workspace named, the profile's workspaces in the same list (Space marks,
  Enter forgets the marked); `--team <id|name>` / `--all` for scripts. Logging out of the last workspace leaves an
  empty profile (only `profile remove` deletes it) (Claude's call). Drafts and sidebar layout stay (they are kept per
  workspace+user and another profile may use them).

## Part 3 — profiles (command line only)

The user: profile management isn't worth an interactive flow, and not inside the messenger.

- `klack profile list` — every profile, ★ on the default, its workspaces and who you are there; reads local files only
  (instant, no network) (Claude's call). An empty profile says `no sign-ins`.
- `klack profile default <name>` — plain `klack` (and `ctl`, `auth`) now use that profile. Stored in a small file next
  to the profiles (`<config>/klack/default-profile`, one line), **not** in settings.toml so the user's settings file is
  never edited for them (Claude's call). `--profile` always wins. An unknown name is refused with the list.
- `klack profile remove <name>` — y/N prompt (or `--yes`) listing what goes: `<config>/klack/<name>/` (sign-ins) and
  `<cache>/klack/<name>/` (log, caches). Drafts and sidebar layout (keyed by workspace+user under the data dir) stay
  (Claude's call). Removing the default profile resets the default to `default` (Claude's call). Refused while a klack
  is running on that profile (builder's call how to detect: e.g. a lock/pid file in the cache dir).
- Profile names: today `--profile` defaults to `default`; with a chosen default, `--profile`'s clap default goes away
  and the effective profile is `--profile` → default-profile file → `default`.

## Part 4 — `klack purge`

- User: deletes everything klack writes to the filesystem, "as if the app was never launched"; asks y/N, or `--yes`.
- Before asking it lists what it will delete (Claude's call), resolved with `directories::ProjectDirs` as klack does
  (Linux XDG, macOS ~/Library/…, Windows AppData): the config dir (every profile's sign-ins, settings.toml, keys.toml,
  locales/, default-profile), the cache dir (logs, caches, `login-browser/` temporary profiles), the data dir (drafts,
  sidebar); old-name `slacktui` folders if present (see `session::adopt_old_dirs`); klack's own leftovers in the temp
  dir (`klack-*` files it creates, e.g. the video opener's `klack-<pid>-<file>`) — only names matching klack's patterns.
- Files the user saved from messages (SaveDir) are theirs and stay. Nothing is signed out on Slack's side (Claude's
  call, consistent with logout).
- Refuses while a klack is running (builder's call how), and prints what it deleted.

## Constraints
- Rust, existing crates (clap, crossterm, ratatui, reqwest, directories, the cookie readers in login_import.rs); no new
  dependency without a reason the user would accept (binary size work in queue T).
- Never print tokens or cookies (logs included); anonymise ids in fixtures.
- Builds on Linux, macOS, Windows; test on the Windows VM and the Mac (docs: klack-test-machines memory).
- House rules: small verified steps, one commit each, `cargo build` without warnings, `cargo test` green before every
  commit, live check, a PROGRESS row; no `cargo fmt`.

## Anti-goals
- Touching the user's real data while testing: their `test` profile (their work workspace) is never logged in/out or
  removed; settings.toml/keys.toml are backed up and restored; real drafts stay. Sign-in/sign-out is tested only with
  throwaway profiles on the test workspace (rslack-testbed); `profile remove` only on throwaway profiles; **`purge`
  only with a fake home** (`HOME`/`XDG_*_HOME` pointed at a scratch folder) — never on the real folders.
- Signing anyone out of a browser, or changing anything in a browser profile.
- A klack-owned persistent browser profile.
- Picking by typing numbers.

## Left to the builder
- Module layout (e.g. an `auth` module with the guide and an inline picker reusable by logout and the expired-session
  question; a `profiles` module).
- The exact help strings (accurate, short, formatted as above) and how formatting is kept out of translated strings.
- How the inline picker is drawn and how it behaves when the terminal is too small (degrade to a plain list).
- Concurrency/timeouts of the checks; how "klack is running" is detected for `profile remove` / `purge`.
- README/help updates for the new commands (and dropping `login` from them).

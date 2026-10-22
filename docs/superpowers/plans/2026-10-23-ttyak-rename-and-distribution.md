# ttyak — rename and distribution — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. One task = one or more
> commits on `master` plus a PROGRESS row; run the whole `cargo test` before each commit (and the site / packaging
> suites the task touches).

**Goal:** klack becomes ttyak everywhere (Part 1), then ttyak installs, updates and uninstalls in one command on Linux,
macOS and Windows, signed, with every package published by CI from one tag (Part 2).

**Architecture:** Part 1 is a rename in four slices (code + build tools, docs, site, captures), each proven by a name
guard (`tools/check-name.sh`) that grows with the slices and runs in CI. Part 2 adds three Rust modules to the `ttyak`
crate (`install.rs` — how this copy was installed; `update.rs` — the daily `latest.json` check, notify once, self-update;
`uninstall.rs`), a POSIX `install.sh` and an `install.ps1` served from the site, a `packaging/` folder whose
`render.py` turns one release's `SHA256SUMS` into every package recipe, and a `release.yml` that builds, signs,
attests, packages and publishes.

**Tech Stack:** Rust 2024 (ratatui 0.30, crossterm 0.29, reqwest 0.13 via slack-core, `sha2` 0.10 and `semver` 1 —
both already in Cargo.lock —, `zip` with deflate only), POSIX sh, PowerShell 5.1+, Python 3 (unittest), GitHub Actions,
nfpm (deb/rpm), podman containers for distro tests.

**Spec:** docs/superpowers/specs/2026-10-23-ttyak-rename-and-distribution-design.md — the authority. Its (verify)
items are unproven until a task proves them on a real run; a task that finds one false stops and reports.

## Global Constraints

- The Bash tool runs **bash**, not fish: `NAME=value`, never `set NAME value`. Start multi-command scripts with
  `set -euo pipefail`. Never a recursive chmod/chown/chgrp/rm/mv/cp or `find -delete`/`-exec rm` on a path built from a
  variable: literal absolute paths, or `${VAR:?}` after checking the path exists and lies where expected.
- Never print tokens, cookies, codes, magic links, API keys, private keys or certificate passwords — not in output,
  logs, reports, commits or CI logs. Secrets reach GitHub only by a pipe into `gh secret set` or by the user.
  Workflows reference secrets by name only.
- Never quote real names, emails or Slack ids in PROGRESS, docs, commits or tests — say A, B. Exception: the package
  metadata fields (maintainer / publisher / author) carry the user's legal name and email address, exactly as the user
  gives them (Task 16); never guessed, never anywhere else.
- The `test` profile is the user's work workspace: never sign it in/out, never post there, never open it. Live checks
  use rslack-testbed: profile `default` = A, `--profile b` = B. Posts there need not be deleted.
- `settings.toml` and `keys.toml` are the user's: back up and restore if a test must change them. Real drafts stay.
  For live checks that change settings, use a fake home in the scratchpad (0700) as `slacktui-live-test-gotchas`
  describes.
- Never build `target/release` (the user runs it). `cargo build` (debug) before every live check. No `cargo fmt`.
- Never capture the microphone, camera or screen; never play sound. `pgrep -x`/`pkill -x` only, never `-f`; never
  `pkill klack` / `pkill ttyak`.
- Never screenshot with grim or `tools/shot.sh`: read the test window with `kitty @ --to unix:@<class> get-text`. Close
  the test window when done (`hyprctl dispatch closewindow class:<class>`). Before Task 4 the class/socket is
  `klack-test`, from Task 4 on `ttyak-test`; an implementer's own extra windows use their own class.
- Live waits and observations: at most ~10 minutes in total per task; report instead of waiting longer.
- Implementers never push, tag, release, force-push, create or rename GitHub repos, set secrets, or dispatch
  subagents. The controller pushes `master` after each reviewed task. Tags, releases (also pre-releases), publishing to
  any package registry, and the repo rename only with the user's word.
- Nothing is installed on this machine with apt/pip/npm -g for this work: distro, shellcheck, PowerShell, makepkg and
  nfpm runs happen in podman containers (images pulled are removed at the end, Task 22). Test machines: the Windows VM
  `win11-dev` and the Mac `macbook` (memory `klack-test-machines`): clean up after, restore the VM's power state.
- Clean scratch copies and `target` dirs you made in the scratchpad (literal paths) before you report.
- Match the surrounding code: its comment density and voice (short `///` lines saying what, not how), naming, idiom.
  New user-visible words go through `tr` (the locale template is generated from code). Everything that waits is
  configurable in seconds in `settings.toml` and F6.
- History stays as written: PROGRESS rows, `docs/superpowers/` specs/plans and old release notes keep saying klack /
  slacktui. New rows say ttyak.
- PROGRESS.md: one row per task in the table above "Tests:" (never after the Bug log), LIVE/UNIT/NOT honesty, the
  commit hash; update the "Tests:" counts line. Say what was not verified.
- Names (fixed here, used verbatim by every task): binary `ttyak` / `ttyak.exe`; crate dir `crates/ttyak`, package
  `ttyak`; env vars `TTYAK_*`; folders via `ProjectDirs::from_path("ttyak")`; log `ttyak.log`; temp prefix `ttyak-`;
  release assets `ttyak-<version>-<platform>.zip` with platforms `x86_64-linux`, `aarch64-linux`, `x86_64-macos`,
  `aarch64-macos`, `x86_64-windows`; packages `ttyak.deb` / `ttyak.rpm` named `ttyak_<version>_<amd64|arm64>.deb` and
  `ttyak-<version>-1.<x86_64|aarch64>.rpm`; repo `dmytrobsp/ttyak`; Homebrew tap repo `dmytrobsp/homebrew-ttyak`
  (`brew install dmytrobsp/ttyak/ttyak`); Scoop bucket repo `dmytrobsp/scoop-ttyak`; AUR `ttyak-bin`; Chocolatey id
  `ttyak`; winget id decided in Task 16; site `https://ttyak.sh` (until the domain works:
  `https://dmytrobsp.github.io/ttyak`); update feed `https://ttyak.sh/latest.json`.

## Rulings (controller's calls, made while planning)

- The crate folder `crates/klack` is renamed `crates/ttyak` (`git mv`); the project folder `~/projects/klack` stays.
- The name guard `tools/check-name.sh` fails on any `klack` (any case) in tracked files outside an allowlist:
  `PROGRESS.md`, `docs/superpowers/`, `tools/move-to-ttyak.sh`, `tools/check-name.sh`, and single lines that carry the
  marker `old-name-ok` (e.g. fill.py's match for pre-0.10 asset names). It takes the paths to check as arguments; CI
  runs it on the paths renamed so far, the last slice on everything.
- The site keeps working between the rename and the first ttyak release: `fill.py` accepts `ttyak-` and `klack-` asset
  names (the latter marked `old-name-ok`, dropped after 0.10.0 by Task 21).
- Install method: `install.json` next to the resolved executable wins; else the executable's path tells (Homebrew
  `Cellar`/`homebrew`/`linuxbrew`, Scoop `\scoop\apps\`, winget `\WinGet\Packages\` or `\WinGet\Links\`, Chocolatey
  `\chocolatey\`, `/usr/bin/ttyak` owned by pacman → aur, by dpkg → deb, by rpm → rpm, by checking the package
  databases' files, not by running the managers); else unknown (a manual download).
- Self-update only for method `script` and a folder ttyak can write. The new archive must match both the SHA256 in
  `latest.json` and its line in the release's `SHA256SUMS`; signer checks come with the signing tasks (16, 17).
- Update state (last check, last version notified) lives in `<data>/updates.json`; settings in `[updates]` of
  `settings.toml`: `check = true`, `check_every_secs = 86400`. The state is written before the notification is shown,
  so a crash can never notify twice.
- A new version shows, once: the desktop notification (click → the update screen, Linux only) and, for that run, a
  status-line pill `⬆ <version>` that stays until the update screen is opened; a click on the pill opens it. F6 →
  Updates is the other way in.
- Packages: one zip per platform stays the only archive; Homebrew, the AUR, Scoop, winget and Chocolatey all install
  from it. `.deb`/`.rpm` come from nfpm. The install scripts unzip with `unzip`, else `bsdtar`, else `python3 -m
  zipfile`, and say which to install when none is there.
- Proving routes needs real published versions: a pre-release tag `v0.10.0-rc.N` (the user's word first, Task 22)
  publishes only the GitHub pre-release and the rendered recipes as release assets; registries (tap, bucket, AUR,
  Chocolatey, winget) are pushed only for tags without `-`. Tap and bucket get a branch `rc` for the rehearsal.

## Review Focus

1. **Scoop's update runs the uninstaller.** `scoop update ttyak` must keep the user's data; only `scoop uninstall`
   purges. Test in Task 16 (rendered manifest's uninstaller purges only when `$cmd -eq 'uninstall'`), proven on the
   VM in Task 22 (rc.1 → rc.2 update keeps a settings file).
2. **`curl … | sh` has no terminal on stdin.** The script must read answers from `/dev/tty`, and with no `/dev/tty`
   use the defaults or fail with a message, never hang. Test in Task 10 (run under `setsid` with stdin a pipe).
3. **Running the installer twice.** The PATH line in the shell profile and the Windows user PATH entry are added
   once; a second run updates in place. Tests in Task 10 and Task 11.
4. **macOS shells under Rosetta report x86_64.** The script picks the arm64 build when
   `sysctl -n sysctl.proc_translated` is 1. Test in Task 10 with a fake `uname`/`sysctl` on PATH.
5. **The update feed misbehaves.** Offline, a 404, malformed JSON, a version equal or older, a version string with a
   pre-release part, a missing asset for this platform: no notification, no error dialog, a debug log line, the next
   check at the usual time. Tests in Task 13.

---

## Part 0

### Task 1: Account runbooks the user follows while Part 1 is built

The spec (Accounts and who does what): the user does what only a person can; Claude writes the steps.

**Files:**
- Create: `docs/distribution/accounts.md`

- [ ] **Step 1: Research each vendor's current steps (full web search, sources linked in the doc)** — Porkbun (buy
  `ttyak.sh`, enable API access for the domain, make an API key; where to save it: `~/.config/ttyak-site/porkbun.json`
  0600 as `{"apikey": …, "secretapikey": …}`), Apple Developer Program as an individual (enrol, 2FA, payment; App Store
  Connect API key with the role notarization needs; whether the API can create a Developer ID Application certificate
  from a CSR or the user uploads it on the web), SSL.com individual (IV) code signing + eSigner (order, ID checks,
  eSigner enrolment, TOTP secret, credential id; current price; current GitHub action name and inputs), AUR (register,
  add the SSH public key Task 18 prints), Chocolatey (register, API key), GitHub tokens (a fine-grained token with
  contents read/write on `homebrew-ttyak` and `scoop-ttyak`; a classic token with `public_repo` for winget pull
  requests; whether winget needs a fork of `microsoft/winget-pkgs` under the user's account first).
- [ ] **Step 2: Write `docs/distribution/accounts.md`** — per vendor: what it is for, numbered steps for the user,
  what the user hands over and how (which file, or `gh secret set NAME` in their own terminal, with the exact secret
  names below), roughly how long it takes, the price. A table of every secret name the workflows will use:
  `PUBLISH_TOKEN`, `WINGET_TOKEN`, `CHOCOLATEY_API_KEY`, `AUR_SSH_KEY` (set by Task 18's script), `APPLE_P12`,
  `APPLE_P12_PASSWORD` (set by Task 19's script), `APPLE_API_KEY`, `APPLE_API_KEY_ID`, `APPLE_API_ISSUER`,
  `APPLE_TEAM_ID`, `ES_USERNAME`, `ES_PASSWORD`, `ES_CREDENTIAL_ID`, `ES_TOTP_SECRET`. Mark anything the research could
  not confirm as (verify).
- [ ] **Step 3: Commit** (`docs: distribution — account runbooks`), PROGRESS row (NOT: docs only).

## Part 1 — rename klack → ttyak

### Task 2: Remove the slacktui folder adoption

The spec: "Remove the existing slacktui → klack folder adoption (`slack-core/src/session.rs`, `profiles.rs`, the
`slacktui.log` cache entry and their tests). Old `KLACK_*` names are not read."

**Files:**
- Modify: `crates/slack-core/src/session.rs` (drop `adopt_old_dirs`, `adopt` and their tests)
- Modify: `crates/slack-core/src/profiles.rs` (drop `Dirs::old_system`, `"slacktui.log"` from `CACHE_FILES`; the
  comments on `roaming_data` that mention the former name)
- Modify: `crates/klack/src/main.rs` (the `adopt_old_dirs` call and its log lines; `purge_command`'s `old_system()`)

- [ ] **Step 1:** Remove the code and the tests that exist only for it; adjust `purge_lists_every_klack_folder` (and
  any test passing old folders) so it no longer expects them.
- [ ] **Step 2:** `cargo build` without warnings; `cargo test` all passing; `grep -rn slacktui crates` returns only
  history-free hits (none expected outside vendor).
- [ ] **Step 3: Commit** (`refactor: drop the slacktui folder adoption`), PROGRESS row (UNIT).

### Task 3: The one-off script that moves the user's own data

The spec: "moves the user's own data … to the ttyak locations. It lives outside the product (e.g. `tools/`), uses
literal paths, checks before it moves, never overwrites an existing ttyak folder, and is run once (the user's OK)."

**Files:**
- Create: `tools/move-to-ttyak.sh`
- Create: `tools/test_move_to_ttyak.py`

**Interfaces:**
- Produces: `sh tools/move-to-ttyak.sh [--dry-run]` — exit 0 when every pair was moved or had nothing to move, 1 when
  it refused anything.

- [ ] **Step 1: Write the failing tests** (`python3 -m unittest tools/test_move_to_ttyak.py`, each in a temp HOME the
  test creates and removes): `test_moves_every_folder` (`.config/klack`, `.cache/klack`, `.local/share/klack`,
  `.config/klack-site` → the `ttyak` names, contents intact, modes kept, e.g. a 0600 `session.json` stays 0600);
  `test_renames_logs` (`.cache/ttyak/<p>/klack.log` → `ttyak.log` when no `ttyak.log` exists);
  `test_never_overwrites` (an existing `.config/ttyak` → that pair refused, the rest moved, exit 1, nothing of the
  existing folder changed); `test_dry_run_moves_nothing`; `test_refuses_while_klack_runs` (a fake `pgrep` on PATH
  that reports a match → nothing moved, exit 1); `test_reports_old_names` (a `settings.toml` mentioning `klack` and a
  fish config setting `KLACK_LOG` → listed in the output as "check by hand", not edited).
- [ ] **Step 2:** Run → FAIL (no script).
- [ ] **Step 3: Implement** — POSIX sh, `set -eu`; `H=${HOME:?}` and `[ -d "$H" ]`; pairs written out as
  `"$H/.config/klack" "$H/.config/ttyak"` etc. (no loops building paths from other variables); `mv` per pair only when
  the source is a directory and the target does not exist; `pgrep -x klack` and `pgrep -x ttyak` refuse; prints what
  it did / would do; prints the leftovers to check by hand (grep -l for `klack`/`KLACK_` in `settings.toml`,
  `keys.toml`, `~/.config/fish`, `~/.bashrc`, `~/.profile`).
- [ ] **Step 4:** Tests pass; `sh tools/move-to-ttyak.sh --dry-run` on the real home lists the four moves and exits 0
  (dry run only — the real run waits for the user's OK, in the controller's hands).
- [ ] **Step 5: Commit** (`tools: one-off move of klack's folders to ttyak's`), PROGRESS row (UNIT; the real run NOT).

### Task 4: Rename the code, the build and the dev tools; the name guard

The spec: "the binary (`ttyak`, `ttyak.exe`), the crate / package names, `KLACK_*` environment variables → `TTYAK_*`,
the data / config / cache folders (`directories` project dirs), the log and lock file names, the release asset names,
… in-app text, the locale template, `ctl` / help output, the `klack-test` kitty class and socket in `tools/`".

**Files:**
- Create: `tools/check-name.sh`, `tools/test_check_name.py`
- Move: `crates/klack` → `crates/ttyak` (`git mv`)
- Modify: `Cargo.toml` (members, `repository = "https://github.com/dmytrobsp/ttyak"`, the patch comments),
  `Cargo.lock`, `crates/ttyak/Cargo.toml` (`name = "ttyak"`), every `.rs` under `crates/` (clap `name = "ttyak"`, user
  text, `tr` strings, comments, tracing filter `ttyak=info,…`, `ttyak.log`, temp prefixes `ttyak-`, notification
  `appname("ttyak")`, huddle signaling `app_name`/`platform_name`, `TTYAK_*`), `crates/slack-core/src/session.rs`
  (`from_path("ttyak")`), `.github/workflows/{ci,release,pages}.yml` (exe names, artifact and asset names, release
  title `ttyak $GITHUB_REF_NAME`; CI runs `sh tools/check-name.sh crates .github Cargo.toml tools/launch.sh
  tools/shot.sh tools/container-build.sh tools/streamrec.py`), `tools/launch.sh` (class/socket `ttyak-test`, the
  binary), `tools/shot.sh`, `tools/container-build.sh`, `tools/streamrec.py`

**Interfaces:**
- Produces: `sh tools/check-name.sh PATH…` — prints each offending `file:line` and exits 1, exits 0 when clean; the
  allowlist and the `old-name-ok` marker as in Rulings. Tasks 5, 6 and 21 extend what CI passes to it.

- [ ] **Step 1: Write the failing tests** for the guard (`tools/test_check_name.py`, temp git repo): a tracked file
  with `Klack` fails naming file:line; `PROGRESS.md` and `docs/superpowers/x.md` pass; a line with `old-name-ok` passes;
  untracked files are ignored.
- [ ] **Step 2:** Run → FAIL; implement `tools/check-name.sh` (`git ls-files -z -- PATH… | xargs -0 grep -niI klack`
  minus the allowlist); tests pass; `sh tools/check-name.sh crates .github Cargo.toml` → FAIL with the current hits
  (the RED for the rename, show the count in the report).
- [ ] **Step 3: Rename** everything listed. Tests that assert texts with klack change with them; test names that say
  klack are renamed. `vendor/` is Task 5's (docs and comments only).
- [ ] **Step 4:** `cargo build` (no warnings), `cargo test` all passing, `target/debug/ttyak --version` prints
  `ttyak 0.9.0`, `target/debug/ttyak ctl locale-template | grep -ci klack` → 0, the guard on the CI paths → exit 0.
  No live check yet (the user's folders move after this task).
- [ ] **Step 5: Commit** (`rename: klack → ttyak in code, build and dev tools`), PROGRESS row (UNIT; LIVE after the
  move).

> **Controller between Tasks 4 and 5:** ask the user's OK to run `sh tools/move-to-ttyak.sh` (their real folders, the
> `test` profile's files moved, never signed in/out); after it, a live smoke check of `ttyak` on A
> (`tools/launch.sh`, read the window with `kitty @ --to unix:@ttyak-test get-text`): signed in, the DM with B loads,
> the status line says ttyak. Tell the user their own build is now `target/release/ttyak`.

### Task 5: Rename the docs, the vendored patches' notes and the remaining tools

**Files:**
- Modify: `README.md`, `CONTRIBUTING.md`, `DESIGN.md`, `docs/protocol/*.md`, `tools/README.md`, other `tools/*`
  scripts not done in Task 4 (not `tools/site/`)
- Move: `vendor/rtc-turn/KLACK-PATCH.md` → `TTYAK-PATCH.md`, `vendor/crossterm/KLACK-PATCH.md` → `TTYAK-PATCH.md`
  (and the comments pointing at them in `Cargo.toml` and vendored sources)
- Modify: `PROGRESS.md` title line and its intro paragraph only (add "until R… the project was called klack: rows
  before it use that name" beside the slacktui sentence)
- Modify: `.github/workflows/ci.yml` (guard paths: everything except `site`, `tools/site`)

- [ ] **Step 1:** Run the guard on the new paths → FAIL (the RED).
- [ ] **Step 2:** Rename; links to `github.com/dmytrobsp/klack` become `…/ttyak` (GitHub redirects the old ones).
  README keeps its install section as it is until Task 21.
- [ ] **Step 3:** Guard on everything except `site tools/site` → exit 0; `cargo test` passing (vendored code
  compiles).
- [ ] **Step 4: Commit** (`rename: klack → ttyak in docs and tools`), PROGRESS row (UNIT).

### Task 6: Rebrand the site and its tools

The spec: "every 'klack' in `site/` and `tools/site/` — titles, headings, copy, meta / Open Graph tags, the logo /
wordmark and favicon, alt texts, the download section, `fill.py`'s generated text and `numbers.json` fields,
`check_private.py`'s config path."

**Files:**
- Modify: `site/index.html`, `site/img/favicon.svg`, `site/js/*.js`, `site/css/site.css` (wordmark only if it is drawn
  in CSS), `site/data/{numbers,release}.json` (field `klack` → `ttyak`; the measured version stays as measured),
  `tools/site/*.py`, `tools/site/*.mjs`, `tools/site/*.md`, fixtures
- Modify: `tools/site/check_private.py` (config folder `~/.config/ttyak-site`, env `TTYAK_SITE_FORBIDDEN`, allowed
  addresses `github.com/<owner>/ttyak`, `<owner>.github.io/ttyak`, `ttyak.sh`)
- Modify: `tools/site/fill.py` (`REPO = "dmytrobsp/ttyak"`, asset pattern accepting `ttyak-` and, marked
  `old-name-ok`, `klack-`)
- Modify: `.github/workflows/ci.yml` (guard on the whole repo)

- [ ] **Step 1: Failing tests:** in `tools/site/test_fill.py`, `test_assets_of_old_name_still_link` (a fixture
  release with `klack-0.9.0-x86_64-linux.zip` fills the download links) and `test_assets_of_new_name_link`
  (`ttyak-0.10.0-…`); in `test_check_private.py`, `ttyak.sh` and `github.com/dmytrobsp/ttyak` allowed, the config
  read from `~/.config/ttyak-site`. The existing structure/copy tests updated to say ttyak first, so they fail.
- [ ] **Step 2:** Rebrand. The wordmark and favicon spell ttyak in the site's current style (same fonts and colours).
  Copy keeps its voice; "klack" → "ttyak" with grammar fixed where the word length matters (Space Mono widths). The
  captures under `site/captures` are Task 8's; their alt texts and captions change here.
- [ ] **Step 3:** `python3 -m unittest discover -s tools/site -p 'test_*.py'` and `node --test "tools/site/*.test.mjs"`
  passing; `python3 tools/site/check_private.py site` clean; `python3 tools/site/fill.py site --fetch-release --strict`
  in a scratch copy works against the live v0.9.0 release (old names); the page opened from a local server
  (`python3 -m http.server` in a scratch copy) and read with `tools/site/pagecheck.mjs`; the guard on the whole repo
  → exit 0.
- [ ] **Step 4: Commit** (`site: rebrand to ttyak`), PROGRESS row (UNIT + local page check; the published site after
  Task 7).

### Task 7: The repo rename (controller, with the user's OK)

- [ ] **Step 1:** `gh repo rename ttyak -R dmytrobsp/klack --yes`; `git remote set-url origin
  https://dmytrobsp@github.com/dmytrobsp/ttyak.git`; `gh repo edit dmytrobsp/ttyak --homepage
  https://dmytrobsp.github.io/ttyak/` (ttyak.sh after Task 21); description says ttyak.
- [ ] **Step 2:** Push; CI green; the Pages workflow deploys; `curl -sI https://dmytrobsp.github.io/ttyak/` → 200 and
  the page says ttyak; `https://dmytrobsp.github.io/klack/` gone (the spec: dropped, no redirect).
- [ ] **Step 3:** Update the memory notes (project status, test machines, live-test gotchas: `ttyak-test`, folders
  `~/.config/ttyak`, `~/.config/ttyak-site`). PROGRESS row (LIVE).

### Task 8: Capture the site's terminal pictures again from ttyak

The spec: "The terminal captures (`site/captures`) … are captured again from the renamed app on rslack-testbed, with
the existing capture tooling and privacy checks."

**Files:**
- Modify: `site/captures/*.html` (and `swan.webp` only if it shows the name), `tools/site/CAPTURES.md` if a step
  changed

- [ ] **Step 1:** Follow `tools/site/CAPTURES.md` with `target/debug/ttyak` (`cargo build` first) on rslack-testbed
  (`#prod-deploys`, the huddle in `#coffee`, profiles `default`/`b`); the huddle capture needs no real mic/camera
  (synthetic sender as CAPTURES.md says, else keep the old huddle capture and say so).
- [ ] **Step 2:** `check_private.py site` clean; site suites passing; `grep -ci klack site/captures/*` → 0.
- [ ] **Step 3: Commit** (`site: captures from ttyak`), PROGRESS row (LIVE).

## Part 2 — distribution

### Task 9: How this copy was installed (`install.rs`)

**Files:**
- Create: `crates/ttyak/src/install.rs`
- Modify: `crates/ttyak/src/main.rs` (`mod install;`)

**Interfaces:**
- Produces:
  - `pub enum Method { Script, Homebrew, Scoop, Winget, Chocolatey, Aur, Deb, Rpm, Unknown }` with
    `pub fn name(self) -> &'static str` (`"script"`, `"homebrew"`, …, `"unknown"`, the strings of `install.json`) and
    `pub fn update_command(self) -> Option<&'static str>` (`brew upgrade ttyak`, `scoop update ttyak`,
    `winget upgrade <winget id>`, `choco upgrade ttyak`, `yay -Syu ttyak-bin`, None for Script/Deb/Rpm/Unknown) and
    `pub fn uninstall_command(self) -> Option<&'static str>` (`brew uninstall ttyak`, `scoop uninstall ttyak`,
    `winget uninstall <winget id>`, `choco uninstall ttyak`, `sudo pacman -Rns ttyak-bin`, `sudo apt remove ttyak`,
    `sudo dnf remove ttyak`; None for Script/Unknown).
  - `#[derive(Serialize, Deserialize)] pub struct Record { pub method: String, pub version: String, pub dir: PathBuf }`
    — the `install.json` written by the scripts.
  - `pub struct Install { pub method: Method, pub exe: PathBuf, pub writable: bool }` and
    `pub fn detect(exe: &Path, fs: &impl Probe) -> Install` plus `pub fn current() -> Install` (real exe, real fs).
  - `pub trait Probe { fn read(&self, p: &Path) -> Option<String>; fn exists(&self, p: &Path) -> bool; fn writable(&self, dir: &Path) -> bool; }`
  - `pub fn platform() -> Option<&'static str>` — this build's release platform name (`x86_64-linux`, … as in
    Global Constraints), None for any other target.
- The winget id is a placeholder constant `WINGET_ID` until Task 16 sets it.

- [ ] **Step 1: Failing tests** (a fake `Probe`): `install_json_wins` (a `/opt/x/ttyak` with `install.json`
  `{"method":"script",…}` → Script even under a Homebrew-looking path); `paths_tell_the_manager` (one case per row of
  the Rulings list, Windows paths included, case-insensitive on Windows-style paths); `usr_bin_by_package_database`
  (`/var/lib/pacman/local/ttyak-bin-0.10.0-1/files` listing `usr/bin/ttyak` → Aur; `/var/lib/dpkg/info/ttyak.list` →
  Deb; rpm database present and neither → Rpm); `nothing_known_is_unknown`; `a_broken_install_json_is_ignored`;
  `platform_of_this_build_is_named`.
- [ ] **Step 2:** Run → FAIL; implement; pass; `cargo test` all passing.
- [ ] **Step 3: Commit** (`install: how this copy was installed`), PROGRESS row (UNIT).

### Task 10: The install script for Linux and macOS (`site/install.sh`)

The spec (The install script): one-liner `curl -fsSL https://ttyak.sh/install.sh | sh`; installs or updates;
`~/.local/bin/ttyak`; PATH in the shell's profile only when missing, and says so; checks `SHA256SUMS`; flags and
matching env vars `--yes`, `--version X.Y.Z`, `--dir PATH`, `--uninstall`, `--keep-data`; never waits for input
without a terminal; writes `install.json` (method `script`).

**Files:**
- Create: `site/install.sh`
- Create: `tools/install/test_install_sh.py`, `tools/install/fixtures/` (built by the test, not committed binaries)

**Interfaces:**
- Consumes: `install.json` shape from Task 9 (`{"method":"script","version":…,"dir":…}`); `ttyak uninstall` from
  Task 12 (`--uninstall` runs `"$dir/ttyak" uninstall` with `--yes`/`--keep-data` passed on).
- Produces: env vars `TTYAK_YES=1`, `TTYAK_VERSION`, `TTYAK_DIR`, `TTYAK_KEEP_DATA=1`, `TTYAK_UNINSTALL=1`; for tests
  and mirrors `TTYAK_LATEST_URL` (default `https://ttyak.sh/latest.json`) and `TTYAK_DOWNLOAD_URL` (default
  `https://github.com/dmytrobsp/ttyak/releases/download`); the PATH line ends with `# added by the ttyak installer`
  (fish: `~/.config/fish/conf.d/ttyak.fish` holding `fish_add_path ~/.local/bin` and the marker).

- [ ] **Step 1: Failing tests** — each in a temp HOME, a local `http.server` serving a fake release (a zip holding a
  `ttyak` shell stub that answers `--version` and `uninstall`, `SHA256SUMS`, `latest.json`), run under `dash` and
  `bash`: `test_installs_latest` (binary 0755 at `~/.local/bin/ttyak`, `install.json` method script + version);
  `test_updates_in_place`; `test_version_flag_and_env`; `test_dir_flag`; `test_refuses_bad_checksum` (nothing
  installed, exit ≠ 0, message names the mismatch); `test_path_added_once` (bash, zsh, fish and plain sh profiles; a
  second run adds nothing; `~/.local/bin` already on PATH → nothing added); `test_no_tty_never_waits` (under `setsid`,
  stdin a pipe, no `--yes`: completes with defaults or exits with a message within 10 s); `test_picks_platform` (fake
  `uname`/`sysctl` on PATH: Linux x86_64/aarch64, macOS arm64, macOS x86_64 under Rosetta → arm64, anything else → a
  clear refusal); `test_unzip_fallbacks` (no `unzip` on PATH → `python3 -m zipfile`); `test_uninstall_passes_flags`.
  Shellcheck runs in a podman container (`docker.io/koalaman/shellcheck:stable`) as part of the test when podman is
  present.
- [ ] **Step 2:** Run → FAIL; implement in POSIX sh (`set -eu`; `/dev/tty` for questions; `sha256sum` or
  `shasum -a 256`; `curl` or `wget`); pass.
- [ ] **Step 3:** In a podman `ubuntu:24.04` and a `fedora` container, the script against the fixture server installs
  and the stub runs. macOS: on `macbook`, the script against a fixture server reached over the LAN (or a copied
  fixture dir served there) installs into a temp HOME; clean up.
- [ ] **Step 4: Commit** (`install: install.sh for Linux and macOS`), PROGRESS row (UNIT + container/Mac runs on
  fixtures; the real release route in Task 22).

### Task 11: The install script for Windows (`site/install.ps1`)

**Files:**
- Create: `site/install.ps1`, `tools/install/test_install_ps1.py` (drives the VM over ssh)

**Interfaces:**
- Consumes/produces: the same flags as Task 10 as PowerShell parameters (`-Yes`, `-Version`, `-Dir`, `-Uninstall`,
  `-KeepData`) and the same env vars; default dir `%LOCALAPPDATA%\Programs\ttyak`; user PATH via
  `[Environment]::SetEnvironmentVariable('Path', …, 'User')`, added once; `install.json` beside `ttyak.exe`.
  Invocation with parameters: `& ([scriptblock]::Create((irm https://ttyak.sh/install.ps1))) -Yes`.

- [ ] **Step 1: Failing tests** against `win11-dev` (start it, `ssh -T -o BatchMode=yes windows … < /dev/null`; the
  fixture server on the host at 192.168.122.1; PowerShell 5.1): installs; updates in place while an old copy is not
  running and while it runs (rename aside, as the self-update does); refuses a bad checksum; PATH added once;
  ARM64 refused with a clear message (`$env:PROCESSOR_ARCHITECTURE` faked by a parameter the test passes:
  `-Arch ARM64`, hidden); `-Uninstall -Yes` runs `ttyak.exe uninstall --yes`. PSScriptAnalyzer in a podman
  `mcr.microsoft.com/powershell` container when podman is present.
- [ ] **Step 2:** Run → FAIL; implement; pass. Clean the VM (Test-Path per path) and shut it down.
- [ ] **Step 3: Commit** (`install: install.ps1 for Windows`), PROGRESS row (VM run on fixtures).

### Task 12: `ttyak uninstall`

The spec (Uninstall): script installs — interactive, asks whether to keep the data, removes everything unless asked
to keep it; `--yes` (everything) and `--yes --keep-data` for scripts; builds on `purge`. Package installs — print the
manager's uninstall command and the purge step.

**Files:**
- Create: `crates/ttyak/src/uninstall.rs`
- Modify: `crates/ttyak/src/main.rs` (`Command::Uninstall { yes: bool, keep_data: bool }`; `purge_command` split so
  uninstall reuses its target listing and deletion)

**Interfaces:**
- Consumes: `install::current()`, `Method::uninstall_command()`; `profiles::purge_targets`, `profiles::purge`.
- Produces: `pub fn plan(install: &Install, keep_data: bool, home: &Path) -> Plan` where
  `Plan { remove: Vec<PathBuf>, path_edits: Vec<PathEdit>, purge: bool }` (`PathEdit::ProfileLine(file)` — the
  installer's marked line; `PathEdit::WindowsUserPath(dir)`); `pub fn run(plan: &Plan) -> Vec<(String, io::Result<()>)>`.
  On Windows the running exe is removed after exit by a detached `cmd /c` that waits for the process id.

- [ ] **Step 1: Failing tests:** `script_install_removes_binary_record_and_path_line` (temp dirs; the profile file
  keeps its other lines); `keep_data_leaves_the_folders`; `package_install_prints_the_command` (for each Method with
  a command: the text names it and `ttyak purge`, and nothing is deleted); `unknown_install_names_the_file`;
  `no_terminal_without_yes_refuses` (message says `--yes`, `--keep-data`); `running_profile_refuses` (as purge).
- [ ] **Step 2:** Run → FAIL; implement; pass; `cargo test` all passing.
- [ ] **Step 3:** Live: in a fake HOME, install `target/debug/ttyak` with Task 10's script against the fixture server
  carrying it, then `ttyak uninstall` interactively in a test kitty window (answers sent with `send-text`), then
  again with `--yes --keep-data`; check what is left. On the VM: the same with `install.ps1` and `--yes`.
- [ ] **Step 4: Commit** (`uninstall: ttyak uninstall`), PROGRESS row (UNIT + LIVE in a fake home + VM).

### Task 13: The daily update check and notify-once (`update.rs`)

The spec (Updates): never auto-update; check once a day by default, can be turned off; read `latest.json`; no personal
data, a plain GET; notify once across runs (desktop notification + a status-bar note).

**Files:**
- Create: `crates/ttyak/src/update.rs`
- Modify: `crates/ttyak/src/tui/settings.rs` (`pub struct Updates { pub check: bool, pub check_every_secs: u64 }`,
  defaults `true`, `86400`), `crates/ttyak/src/tui/event_loop.rs` (start the check task; the pill), the status line
  module (the pill), `crates/ttyak/src/tui/notify.rs` (reuse `show` with the click opening the update screen)
- Modify: `crates/ttyak/Cargo.toml` (`reqwest` with slack-core's exact features so no second copy is built; `semver`)

**Interfaces:**
- Consumes: `install::platform()`; the `latest.json` schema (Task 16 renders it): `{"name":"ttyak","version":V,"date":"YYYY-MM-DD","notes":"https://github.com/dmytrobsp/ttyak/releases/tag/vV","assets":{"<platform>":{"url":…,"sha256":…},…}}`.
- Produces: `pub struct Latest { pub version: semver::Version, pub date: String, pub notes: String, pub asset: Asset }`,
  `pub struct Asset { pub url: String, pub sha256: String }`; `pub fn parse(json: &str, platform: &str) -> Result<Latest>`;
  `pub fn newer(current: &semver::Version, latest: &Latest) -> bool` (pre-release versions never offered);
  `pub struct State { pub last_check: u64, pub notified: Option<String> }` read/written at `<data>/updates.json`;
  `pub fn due(state: &State, now: u64, every: u64) -> bool`; `pub fn should_notify(state: &State, latest: &Latest) -> bool`;
  `pub async fn fetch(url: &str) -> Result<String>` (GET, 10 s timeout, User-Agent `ttyak/<version>`, nothing else);
  URL `https://ttyak.sh/latest.json`, overridable by `TTYAK_UPDATE_URL`. The TUI receives `AppEvent::UpdateAvailable(Latest)`.

- [ ] **Step 1: Failing tests:** `parses_latest`; `missing_platform_is_an_error`; `malformed_is_an_error`;
  `older_equal_and_prerelease_are_not_newer`; `due_after_the_interval_only`; `notified_once` (state with
  `notified = "0.10.1"` → no notification for 0.10.1, yes for 0.10.2); `state_written_before_notifying` (the
  sequence function returns the new state to write first); `check_off_never_fetches`; the TUI: `pill_shown_until_the_update_screen_opens`.
- [ ] **Step 2:** Run → FAIL; implement; pass; `cargo test` all passing; a new F6 help text for the two settings is
  Task 15's (F6 page) — here only settings.toml.
- [ ] **Step 3:** Live (fake home, sounds off): a local server with a `latest.json` naming 9.9.9 and
  `TTYAK_UPDATE_URL` pointing at it → the notification (dbus-monitor) and the pill once; restart → no notification;
  `check = false` → no request (server log). Feed down → nothing shown.
- [ ] **Step 4: Commit** (`update: daily check, notified once`), PROGRESS row.

### Task 14: The update screen and self-update

The spec: clicking the notification opens the update screen; "Update now" only when ttyak can update itself
(script install into a writable folder); downloads, checks (SHA256 from `latest.json` / `SHA256SUMS`, the platform
signature where there is one), swaps the binary (Windows: rename the running exe aside), says to restart — never
restarts by itself. Otherwise the screen shows the command to run (or the `.deb`/`.rpm` download link).

**Files:**
- Create: `crates/ttyak/src/tui/update_view.rs` (the dialog)
- Modify: `crates/ttyak/src/update.rs` (`pub async fn self_update(latest: &Latest, install: &Install, progress: …) -> Result<PathBuf>`),
  `crates/ttyak/src/main.rs` (at start: delete a leftover `ttyak.old.exe` beside the exe on Windows),
  `crates/ttyak/Cargo.toml` (`zip` with `default-features = false, features = ["deflate"]`), event loop / keymap
  (open from the notification click, the pill click and F6), `i18n.rs` titles

**Interfaces:**
- Consumes: `Install`, `Method::update_command()`, `Latest`.
- Produces: `update::self_update` verifying in this order: download to `<exe dir>/.ttyak-update-<pid>.zip`, SHA256
  against `latest.asset.sha256`, the same file's line in `SHA256SUMS` fetched from the same release, unzip `ttyak`
  (`ttyak.exe`) into the exe's folder as `.ttyak-new`, (signature check from Tasks 19/20 when the running binary is
  signed), swap (Unix: chmod 755 + rename over; Windows: rename running → `ttyak.old.exe`, rename new → `ttyak.exe`,
  on failure rename back), update `install.json`'s version, remove the temp files. Any failure leaves the old binary
  untouched and says what failed.

- [ ] **Step 1: Failing tests** (local HTTP fixture, temp dirs): `self_update_swaps_and_keeps_mode`;
  `sha_mismatch_leaves_the_old_binary`; `sums_mismatch_leaves_the_old_binary`; `unwritable_folder_offers_the_command`
  (the view model says command, no "Update now"); `package_install_shows_its_command` (one per method);
  `deb_rpm_show_the_download_link`; `windows_swap_renames_aside` (cfg(windows) test, run on the VM via
  `cargo test` there or noted); the view: `update_now_only_when_self_updatable`, `says_restart_never_restarts`.
- [ ] **Step 2:** Run → FAIL; implement; pass.
- [ ] **Step 3:** Live: fake home, the debug build installed by `install.sh` from the fixture server as 0.9.9;
  fixture `latest.json` names 9.9.9 with a zip holding a copy of the debug binary; click the notification
  (`gdbus emit … ActionInvoked "uint32 ID" "default"`), Update now, the screen says restart; `~/.local/bin/ttyak
  --version` shows the new one. The same with a read-only folder → the command shown. Windows on the VM with the
  cross-built debug exe: Update now while running; restart works; the `.old.exe` is gone after the restart.
- [ ] **Step 4: Commit** (`update: the update screen and self-update`), PROGRESS row.

### Task 15: F6 → Updates

The spec: a page showing the current version and the install method, the check on/off, "Check now", the manual update
command for this install method, and an "Update" button whenever an update is available, leading to the update
screen; keys and help like the other sections.

**Files:**
- Modify: `crates/ttyak/src/tui/prefs.rs` (`CATEGORIES` gains `("Updates", &[Field::UpdateVersion,
  Field::UpdateCheck, Field::UpdateEvery, Field::UpdateCheckNow, Field::UpdateCommand, Field::UpdateOpen])`; read-only
  rows render dimmed like others; `UpdateOpen` only when an update is known), `crates/ttyak/src/tui/prefs_help.rs`
  (help per item), README (the updates paragraph)

- [ ] **Step 1: Failing tests:** `updates_category_lists_its_fields`; `version_row_names_method`; `check_toggle_writes_settings`;
  `update_row_only_when_available`; `check_now_sends_the_check`; `every_item_has_help` (the existing all-fields help
  test covers it once the fields exist).
- [ ] **Step 2:** Run → FAIL; implement; pass.
- [ ] **Step 3:** Live (fake home): F6 → Updates shows version + method (`unknown` for a debug build without
  `install.json`), toggling the check writes `[updates] check = false`, Check now with the fixture feed shows the
  Update row, Enter on it opens the update screen.
- [ ] **Step 4: Commit** (`prefs: F6 → Updates`), PROGRESS row.

### Task 16: Package recipes from one release (`packaging/`)

The spec: Homebrew formula (own tap) with caveats giving the cleanup command; Scoop with an uninstaller that purges
(only on a real uninstall); winget portable zip; Chocolatey with `chocolateyUninstall.ps1` purging; AUR `ttyak-bin`;
`.deb`/`.rpm` (no home-folder hooks; the description says how to purge); metadata: the user's legal name + email.

> **Controller before dispatch:** ask the user for their legal name as on the certificates (never guessed) and
> confirm the email; decide the winget id (`<Publisher>.ttyak` per winget-pkgs naming) with the user's name.

**Files:**
- Create: `packaging/render.py`, `packaging/test_render.py`, `packaging/identity.json` (name, email, winget id,
  homepage, licence), `packaging/templates/{ttyak.rb, ttyak.json, ttyak.nuspec, chocolateyInstall.ps1,
  chocolateyUninstall.ps1, PKGBUILD, winget.installer.yaml, winget.locale.en-US.yaml, winget.yaml, nfpm.yaml}`
- Modify: `crates/ttyak/src/install.rs` (`WINGET_ID`)

**Interfaces:**
- Produces: `python3 packaging/render.py --version V --sums SHA256SUMS --out DIR` writing `DIR/homebrew/ttyak.rb`,
  `DIR/scoop/ttyak.json`, `DIR/chocolatey/{ttyak.nuspec,tools/*.ps1}`, `DIR/aur/{PKGBUILD,.SRCINFO}`,
  `DIR/winget/<id parts>/<V>/*.yaml`, `DIR/nfpm.yaml`, `DIR/latest.json`.
  `latest.json`: `{"name":"ttyak","version":V,"date":"YYYY-MM-DD","notes":"https://github.com/dmytrobsp/ttyak/releases/tag/vV","assets":{"<platform>":{"url":…,"sha256":…},…}}`
  (platforms as in Global Constraints; Task 13's update check reads exactly this).
- Every recipe installs from the platform zip and writes no `install.json`: Task 9's path detection names the
  manager.

- [ ] **Step 1: Failing tests** (`python3 -m unittest packaging/test_render.py`, a fixture SHA256SUMS for 0.10.0):
  every file rendered with the right URLs and hashes per platform; the formula has `on_macos`/`on_linux` ×
  `on_arm`/`on_intel`, a `test do` running `ttyak --version`, and caveats naming `ttyak purge`; the Scoop manifest's
  uninstaller runs `ttyak.exe purge --yes` only inside `if ($cmd -eq 'uninstall')`, has `checkver`/`autoupdate`
  omitted (CI pushes each version), `bin` `ttyak.exe`; nuspec carries the identity and a release-notes link;
  `chocolateyUninstall.ps1` purges for the invoking user; PKGBUILD has `source_x86_64`/`source_aarch64` with the
  zips' sha256, `noextract` none, `package()` installing to `/usr/bin/ttyak` and docs/licences, and `.SRCINFO`
  matches `makepkg --printsrcinfo` (that one check runs in a podman `archlinux` container when podman is present);
  winget manifests: `InstallerType: zip`, `NestedInstallerType: portable`, `PortableCommandAlias: ttyak`, schema
  version current (verify); nfpm.yaml: `/usr/bin/ttyak`, docs, licences, description with the purge step;
  `latest.json` as specified; the user's name appears only in maintainer/publisher/author fields.
- [ ] **Step 2:** Run → FAIL; implement; pass.
- [ ] **Step 3:** Validate where tools exist without installing anything here: `brew style`/`brew audit` on the
  `macbook` (copy the formula, run, clean up); `winget validate` on the VM if winget is there (else note it);
  `choco pack` in the VM if Chocolatey is there (else in a podman image with choco, or note it); nfpm in a podman
  `goreleaser/nfpm` container packs both from the 0.9.0 Linux binary renamed (smoke only).
- [ ] **Step 4: Commit** (`packaging: recipes rendered from a release`), PROGRESS row (UNIT + the validators run).

### Task 17: The release workflow builds, packages and attests everything

The spec: add aarch64 Linux; `.deb`/`.rpm` on each release; `SHA256SUMS`; GitHub artifact attestations on every
release asset; publishing automated on a tag.

**Files:**
- Modify: `.github/workflows/release.yml`

- [ ] **Step 1:** Matrix gains `{ name: aarch64-linux, os: ubuntu-22.04-arm, target: aarch64-unknown-linux-gnu, exe:
  ttyak }` (verify the runner label for public repos; else cross-compile on 22.04 and say so). A `package` step in
  the release job: zips as now, nfpm (pinned version, checksum-verified download) builds amd64 and arm64 `.deb` and
  `.rpm`, `SHA256SUMS` over everything, `packaging/render.py` renders the recipes into `dist/recipes/` (uploaded as
  `ttyak-<V>-recipes.zip`; not in SHA256SUMS' platform list), `actions/attest-build-provenance` (verify the current
  action and version) over every asset (`id-token: write`, `attestations: write`).
- [ ] **Step 2:** A `workflow_dispatch` run on master (no tag) builds all five platforms and uploads the would-be
  `dist/` as an artifact (packages + recipes, rendered with version `0.0.0-dispatch`). Run it; download the artifact
  into the scratchpad; check the aarch64 Linux zip's binary with `file` (ARM aarch64), the `.deb` installs and
  `ttyak --version` runs in podman `ubuntu:24.04` (amd64; arm64 under qemu only if the container runtime has it, else
  NOT), the `.rpm` in podman `fedora`. Clean up.
- [ ] **Step 3: Commit** (`release: aarch64 Linux, deb/rpm, recipes, attestations`), PROGRESS row (LIVE: the dispatch
  run).

### Task 18: Publishing from the tag

The spec: "GitHub Release, `latest.json` on the site, the Homebrew tap, the Scoop bucket, the AUR, the Chocolatey push
and the winget pull request all come from the release workflow." Release not held by review queues.

**Files:**
- Modify: `.github/workflows/release.yml` (jobs `tap`, `bucket`, `aur`, `chocolatey`, `winget` after `release`)
- Modify: `.github/workflows/pages.yml`, `tools/site/fill.py` (`site/latest.json` from the release's recipes asset)
- Create: `tools/publish/aur-key.sh` (generates an ed25519 key in a 0700 scratch dir, pipes the private key into
  `gh secret set AUR_SSH_KEY -R dmytrobsp/ttyak`, prints only the public key, deletes the private file)

> **Controller before the jobs can run:** with the user's OK, create the empty public repos
> `dmytrobsp/homebrew-ttyak` and `dmytrobsp/scoop-ttyak` (`gh repo create`), each with a README saying how to use it.

- [ ] **Step 1:** Each publishing job: skips with a notice when its secret is missing; runs only for tags without
  `-` (pre-releases: none of them; the rc rehearsal of Task 22 pushes tap and bucket to their `rc` branch instead);
  tap/bucket — clone with `PUBLISH_TOKEN`, write the rendered file, commit "ttyak V", push; aur — ssh with
  `AUR_SSH_KEY` (known host pinned), commit PKGBUILD + .SRCINFO, push; chocolatey — on `windows-latest`, `choco pack`
  + `choco push --source https://push.chocolatey.org/` with `CHOCOLATEY_API_KEY`; winget — open the PR from the
  rendered manifests with `WINGET_TOKEN` (komac or wingetcreate; verify which works from Linux and for a first
  submission). The GitHub release is created before them and never waits for them.
- [ ] **Step 2:** `fill.py --fetch-release` also writes `site/latest.json` from the release's `latest.json` inside
  the recipes asset; a test in `tools/site/test_fill.py` (`test_latest_json_copied_from_release`); a pre-release never
  becomes `latest.json` (test).
- [ ] **Step 3:** actionlint in podman (`docker.io/rhysd/actionlint`) clean; site suites pass; a `workflow_dispatch`
  run still green (publishing jobs skipped).
- [ ] **Step 4: Commit** (`release: publish to tap, bucket, AUR, Chocolatey, winget`), PROGRESS row (UNIT/actionlint;
  real publishing NOT until Task 22).

### Task 19: macOS signing and notarization

The spec: Developer ID Application certificate in the user's name; CI signs (`codesign --options runtime
--timestamp`) and notarizes (`notarytool` with an App Store Connect API key) both macOS binaries; a bare binary is not
stapled. Waits for the user's Apple enrolment (Task 1's runbook).

**Files:**
- Create: `tools/signing/apple-csr.sh` (key + CSR into `~/.config/ttyak-signing/` 0700 — the user's backup —, the
  CSR path printed for the user to upload; after the user downloads the `.cer`: builds the p12 with a random password
  and pipes the p12 (base64) and the password into `gh secret set APPLE_P12` / `APPLE_P12_PASSWORD`; prints nothing
  secret)
- Modify: `.github/workflows/release.yml` (macOS build jobs: temp keychain, sign, `notarytool submit --wait` of a zip
  of the binary; skipped when the secrets are missing)
- Modify: `crates/ttyak/src/update.rs` (from Task 13: on macOS, a signed running binary only accepts a new one that
  `codesign --verify --strict` passes with the same Team ID)

- [ ] **Step 1:** Test for the update check of signatures with a fake verifier (same team → ok, other team or
  unsigned while the running one is signed → refused with a message).
- [ ] **Step 2:** A dispatch run signs and notarizes; on `macbook`: the downloaded binary passes
  `codesign -dv --verbose=4` (Authority = Developer ID Application: the user), `spctl -a -t install` /
  `--type execute` as applicable, and runs after a browser-style download (quarantine set with `xattr -w
  com.apple.quarantine`) with no Gatekeeper prompt, online; offline first launch noted (verify). Clean up.
- [ ] **Step 3: Commit** (`release: sign and notarize macOS builds`), PROGRESS row (LIVE on the Mac).

### Task 20: Windows signing with SSL.com eSigner

The spec: SSL.com IV certificate with eSigner and the official GitHub action; the Windows build is cross-compiled on
Linux, so the signer runs there (CodeSignTool is Java). Waits for the user's SSL.com certificate.

**Files:**
- Modify: `.github/workflows/release.yml` (sign `ttyak.exe` after the cross-build, before staging; skipped without
  the `ES_*` secrets)
- Modify: `crates/ttyak/src/update.rs` (on Windows, a signed running exe only accepts a new one whose Authenticode
  signer subject matches, via `Get-AuthenticodeSignature` in a hidden `powershell -NoProfile`)

- [ ] **Step 1:** Test with a fake verifier (as Task 19).
- [ ] **Step 2:** A dispatch run signs; on the VM `Get-AuthenticodeSignature` says Valid with the user's name; a copy
  with the Mark of the Web runs with the publisher shown (SmartScreen may still warn until reputation builds — say so
  as observed). Clean up.
- [ ] **Step 3: Commit** (`release: sign the Windows build`), PROGRESS row (LIVE on the VM).

### Task 21: The domain, the site's install call to action, README

The spec: the site moves to `ttyak.sh` on GitHub Pages with HTTPS; DNS via the registrar's API (key from a file the
user wrote); the one-liners become the site's main call to action once they work; README says how to install, verify
(`SHA256SUMS`, `gh attestation verify <file> -R dmytrobsp/ttyak`), build it yourself, update and uninstall. Waits for
the user's domain and API key.

**Files:**
- Create: `tools/dns/porkbun.py` (reads `~/.config/ttyak-site/porkbun.json`; `--dry-run` prints the records it would
  set; sets A `185.199.108.153`–`111.153`, AAAA `2606:50c0:8000::153`–`8003::153` (verify against GitHub's docs), CNAME
  `www` → `dmytrobsp.github.io`; never prints the key), `tools/dns/test_porkbun.py` (a fake API)
- Create: `site/CNAME` (`ttyak.sh`)
- Modify: `site/index.html` + tests (install one-liners per OS as the main call to action, the packages listed,
  "verify" link), `README.md` (Install / Verify / Build it yourself / Update / Uninstall), `tools/site/check_private.py`
  allowlist if needed; drop fill.py's `old-name-ok` klack asset match once 0.10.0 exists (or leave a ledger note)

- [ ] **Step 1:** Failing tests for the DNS script (records diffed against the fake API's state; idempotent second
  run) and the site's new copy (`test_copy.py`, `test_structure.py`).
- [ ] **Step 2:** Implement; pass; `check_private.py site` clean.
- [ ] **Step 3:** (Controller, when the user says the domain and key are ready) run the DNS script, then
  `gh api -X PUT repos/dmytrobsp/ttyak/pages -f cname=ttyak.sh`, wait for the certificate (bounded, ≤10 min a try),
  `-F https_enforced=true`; `curl -sI https://ttyak.sh/` 200, `https://ttyak.sh/install.sh` serves the script,
  `https://ttyak.sh/latest.json` (after the first release).
- [ ] **Step 4: Commit(s)** (`site: install first; ttyak.sh`, `docs: README install, verify, update, uninstall`),
  PROGRESS rows.

### Task 22: Prove every route on a real pre-release, then the release

The spec: everything marked (verify) is unproven until a real run. Only with the user's word: a tag `v0.10.0-rc.1`.

- [ ] **Step 1:** (Controller, with the user's word) bump to `0.10.0-rc.1`, tag, push; the workflow publishes the
  GitHub pre-release with all assets and attestations; tap and bucket get their `rc` branches.
- [ ] **Step 2:** Prove, each on a real machine, and write each result into PROGRESS (LIVE or NOT with why):
  `install.sh --version 0.10.0-rc.1` on this machine (fake HOME) and the Mac; `install.ps1 -Version 0.10.0-rc.1` on
  the VM; `brew install` from the tap's `rc` branch on the Mac and in a podman Linuxbrew container;
  `scoop install` from the bucket's `rc` branch on the VM, then rc.2 (`v0.10.0-rc.2`, the user's word again) and
  `scoop update ttyak` keeps a settings file, `scoop uninstall ttyak` purges; `winget install --manifest` and
  `choco install` from the rendered recipes on the VM (local sources), then `choco upgrade` from rc.1 to rc.2 keeps a
  settings file (verify that Chocolatey's upgrade never runs `chocolateyUninstall.ps1`; if it does, the purge moves
  behind the same real-uninstall check as Scoop's); the `ttyak-bin` PKGBUILD with `makepkg -si` in
  podman `archlinux`; `.deb`/`.rpm` in podman; `gh attestation verify` for one asset per platform; signatures on the Mac
  and the VM (when Tasks 19/20 are done); the update screen from rc.1 to rc.2 via the script install.
- [ ] **Step 3:** Remove everything installed on the machines, the podman images pulled for this plan, the `rc`
  branches; restore the VM's power state.
- [ ] **Step 4:** (Controller) the final whole-branch review; then, with the user's word, bump to `0.10.0`, tag; watch
  every publishing job; the site's `latest.json` and the one-liners against the real release.

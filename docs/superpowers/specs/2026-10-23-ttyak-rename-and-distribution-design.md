# ttyak: rename, distribution, install / update / uninstall, signing — design

Status: draft from the brainstorm of 2026-10-22/23 with the user. Parts marked **(user)** are the user's decisions in
their words or close to them; **(Claude's call)** marks defaults the user heard and did not object to, or details left
open in the conversation; **(verify)** marks facts from research that a real run must confirm before anything relies on
them.

## Why

klack works the way the user wants ("i do like the current state of the app"); the last two substantial pieces are
getting it onto people's machines and keeping it there: "i want to streamline the process of installing/updating/
deleting the app as much as possible. and i would also like for app to be signed so that anyone could see confirm that
the executable he downloaded has not been tampered with."

During the domain search we found klack.sh is "Klack — Make Slack yours", a Slack-customization product: same name, same
space, first. So the project is renamed first (the user "didnt like klack that much anyway"), and everything that bakes
the name in (packages, domain, certificates) comes after.

## Part 1 — Rename klack → ttyak

### Decided
- New name **ttyak** (user): "tty" + "yak" (to talk). Vetted 2026-10-23: no app, software or project of that name found
  on the web; free on crates.io, Homebrew (formula and cask), AUR, Chocolatey, npm, PyPI; the repo `dmytrobsp/ttyak` is
  free (a personal GitHub account `ttyak` exists, unrelated); `ttyak.com` is an artist's doodle site; `ttyak.sh`,
  `.dev`, `.app`, `.io` had no DNS.
- Everything named klack becomes ttyak (user): the binary (`ttyak`, `ttyak.exe`), the crate / package names, `KLACK_*`
  environment variables → `TTYAK_*`, the data / config / cache folders (`directories` project dirs), the log and lock
  file names, the release asset names, the landing page, README, docs, in-app text, the locale template, `ctl` / help
  output, the `klack-test` kitty class and socket in `tools/`, the private landing-page folder `~/.config/klack-site`.
- **No migration code and no backward compatibility** (user): slacktui was never released publicly, and nobody but the
  user installed klack. Remove the existing slacktui → klack folder adoption (`slack-core/src/session.rs`,
  `profiles.rs`, the `slacktui.log` cache entry and their tests). Old `KLACK_*` names are not read.
- **A one-off script** moves the user's own data (user): their klack config / data / cache folders (profiles `default`,
  `b`, `test`, settings.toml, keys.toml, sessions, drafts, folds) and `~/.config/klack-site` to the ttyak locations.
  It lives outside the product (e.g. `tools/`), uses literal paths, checks before it moves, never overwrites an
  existing ttyak folder, and is run once (the user's OK, since it touches their real `test` profile's files — moving,
  never signing in or out).
- **The GitHub repo is renamed** to `dmytrobsp/ttyak` (user; GitHub redirects the old URLs and clones). The local
  project folder `~/projects/klack` keeps its name (user).
- History stays as written: PROGRESS rows, old release notes and old tags keep saying klack / slacktui, as after the
  slacktui → klack rename (Claude's call). Old klack releases stay on GitHub (Claude's call).
- The landing page moves to **`ttyak.sh`** (user) on GitHub Pages with HTTPS. The old `dmytrobsp.github.io/klack`
  address is simply dropped (user: "just nuke it"): no redirect repo.
- **The site is rebranded too** (user: "dont forget the site itself"): every "klack" in `site/` and `tools/site/` —
  titles, headings, copy, meta / Open Graph tags, the logo / wordmark and favicon, alt texts, the download section,
  `fill.py`'s generated text and `numbers.json` fields, `check_private.py`'s config path. The terminal captures
  (`site/captures`) show the app's own name (status line, help, window titles): they are captured again from the
  renamed app on rslack-testbed, with the existing capture tooling and privacy checks. The site gains the install
  one-liners (Part 2) as its main call to action once they work.

### Left to the builder
- The exact order of commits; a check that nothing user-visible still says klack (a grep in CI or a test).
- The first release under the new name: the next version (0.10.0) (Claude's call), only with the user's word, as every
  release.

## Part 2 — Distribution

### Install routes (user, from the conversation)

| Platform | Routes |
|---|---|
| Linux | the install script · Homebrew (on Linux) · AUR `ttyak-bin` · `.deb` and `.rpm` files on each release |
| macOS | the install script · Homebrew formula (own tap) |
| Windows | the PowerShell one-liner · Scoop (own bucket) · winget (portable, zip) · Chocolatey |

- **Targets** (user: yes): add **aarch64 Linux** to the release builds (and to Homebrew on Linux, the AUR, `.deb`,
  `.rpm`, the script). Windows on ARM is not built: the PowerShell script refuses it cleanly with a clear message.
- **Publishing is automated in CI on each tag** (user): GitHub Release, `latest.json` on the site, the Homebrew tap,
  the Scoop bucket, the AUR, the Chocolatey push and the winget pull request all come from the release workflow. One
  tag does everything; no session is needed per release.
- **Package metadata** (user: "name + mail works"): maintainer / packager / owner / publisher fields carry the user's
  legal name (as on the certificates — the user supplies it; not guessed) and their email address.
  **Amended 2026-10-23 (user):** the listings show the user's handle and email (`dmytrobsp`), not the legal name;
  the legal name is only in the code-signing certificates. winget id `dmytrobsp.ttyak`.

- Not doing (user): Flatpak and Snap (the sandbox fights sound, screen capture and reading browser profiles for the
  cookie lift), Homebrew casks, MSYS2 pacman, Microsoft Store, an own Windows installer (`.exe`/`.msi`).
- Later, if people ask (Claude's call): proper APT / RPM repositories (so `apt upgrade` / `dnf upgrade` pick it up),
  nixpkgs, `cargo install` / `cargo binstall`.
- Releases are not held up by review queues (Claude's call): GitHub, the script, Homebrew, Scoop and the AUR get a
  version at once; winget and Chocolatey get it when their review passes.

### The install script

- One-liners (user, slk-style): `curl -fsSL https://ttyak.sh/install.sh | sh` and, on Windows,
  `irm https://ttyak.sh/install.ps1 | iex`. They live on the landing page's site (GitHub Pages at ttyak.sh).
- The same script installs, or updates when ttyak is already there (user).
- Where it installs (Claude's call): Linux / macOS `~/.local/bin/ttyak` (says so, and adds it to PATH in the shell's
  profile only when it is not already there, and tells the user); Windows `%LOCALAPPDATA%\Programs\ttyak\ttyak.exe`,
  added to the user PATH. No sudo / admin.
- It picks the right archive for the OS and CPU, downloads it from the GitHub Release, checks it against
  `SHA256SUMS`, and refuses to install on a mismatch (Claude's call).
- **Non-interactive** (user): flags (and matching environment variables) such as `--yes`, `--version X.Y.Z`,
  `--dir PATH`, `--uninstall`, `--keep-data`; with no terminal attached it never waits for input — it uses defaults,
  or fails with a clear message when an answer is required (`curl … | sh -s -- --yes`, and the PowerShell
  equivalent).
- It records how ttyak was installed (Claude's call): a small `install.json` next to the binary (method `script`,
  version, where). Package recipes write their own method (`homebrew`, `scoop`, `aur`, `deb`, `rpm`, `winget`,
  `chocolatey`). No file: unknown (a manual download).

### Updates (user's design)

- **Never auto-update by default** (user).
- ttyak checks for a new version **once a day** by default (user); the check can be **turned off** (user). What it reads
  (Claude's call): a tiny `latest.json` (version, release date, notes link, per-platform asset names and SHA256s)
  published on the landing page with each release — GitHub Pages serves static files at scale, so no GitHub API rate
  limit (60/hour/IP unauthenticated) and no load concern. No personal data is sent; a plain GET.
- A new version is **notified once** (user): "it should never notify the user about the same update twice" — the last
  version notified is remembered across runs. The notification is a desktop notification as ttyak's message ones, plus
  a note in the status bar, since some Linux setups have no notifications (user's point; the status-bar note is
  Claude's call).
- On macOS and Windows the notification only informs (user: yes) — clicks there need an app bundle / a registered
  Start-menu identity, which bare binaries and package installs don't have — and the status-bar note and the
  settings page lead to the update screen. On Linux, where ttyak's notifications take clicks:
- Clicking the notification opens the **update screen** (user). It offers **"Update now"** only when ttyak can update
  itself (user) — installed by the script into a folder it can write. With consent it downloads the new archive,
  checks it (SHA256 from `latest.json` / `SHA256SUMS`, and the platform signature where there is one), swaps the binary
  in (Windows: rename the running exe aside, then put the new one in place), and says to restart — **it never restarts
  by itself** (user: "correct").
- When it cannot update itself (a package manager owns the file, a manual download, a folder it cannot write), the
  update screen shows **the command to run** instead (user): `brew upgrade ttyak`, `scoop update ttyak`,
  `winget upgrade ttyak`, `choco upgrade ttyak`, the AUR helper's / `yay -Syu`, or the download link for `.deb`/`.rpm`.
- In **settings** (user): a page for updates showing **the current version and the install method**, the
  **check on/off**, **"Check now"**, the **manual update command** for this install method, and an **"Update" button
  whenever an update is available** that leads to the update screen (user: "so that notification is not the only way
  to the update"). Where (Claude's call): a new section in F6 (preferences), keys and help like the other sections.

### Uninstall (user's rules)

- **Installed by the script:** `ttyak uninstall` (or the script with `--uninstall`) is **interactive by default and asks
  whether to keep the data** (user); everything is removed unless the user asks to keep it — the program, settings,
  keys, themes, sign-ins of every profile, drafts, caches, logs. Non-interactive with `--yes` (removes everything) or
  `--yes --keep-data` (user: non-interactive must be supported for scripted use). Builds on the existing
  `klack purge` (to be `ttyak purge`).
- **Installed by a package manager:** `ttyak uninstall` **prints that manager's uninstall command** (user) and the purge
  step for the data.
- **When the package manager itself uninstalls:** non-interactive and **always purges** the user data (user) — where the
  manager allows a hook:
  - Scoop: the manifest's uninstaller script purges.
  - Chocolatey: `chocolateyUninstall.ps1` purges (it runs elevated: the profile of the user who ran choco).
  - Homebrew formula: formulae have no uninstall step; the formula's **caveats** (shown after `brew install` and in
    `brew info`) give the cleanup command (user asked for at least that).
  - winget (portable zip), AUR, `.deb`, `.rpm`: no hook ours (winget portable has none; Arch and Debian rules forbid
    packages touching home folders) — `ttyak uninstall` prints the purge step; the package's description / notes say
    it too.

### Signing and verification

- **Goal** (user): the OS should not complain. Manual verification is a bonus ("you can either use checksum or even just
  build the app yourself").
- **macOS** (user: "99 a year isnt that bad"): Apple Developer Program as an individual; a Developer ID Application
  certificate in the user's name; CI signs (`codesign --options runtime --timestamp`) and notarizes (`notarytool` with
  an App Store Connect API key) both macOS binaries. A bare binary can't be stapled: ship it in the zip as now; a first
  launch offline is the one gap (verify). Script and Homebrew installs don't get the quarantine flag anyway (verify for
  Homebrew formulae).
- **Windows** (user: "i want my name on those binaries"; SSL.com chosen over Certum): an SSL.com individual (IV)
  code-signing certificate with eSigner cloud signing and the official GitHub Action `sslcom/actions-codesigner`
  (verify the current action name and inputs). Roughly $249 + $180/year eSigner Tier 1 (verify the price). SmartScreen
  reputation builds over time with any certificate; the script, Scoop and winget installs don't set the Mark of the
  Web (verify).
- **Everywhere, free:** `SHA256SUMS` as now, plus **GitHub artifact attestations** (`actions/attest`) on every release
  asset, so anyone can run `gh attestation verify <file> -R dmytrobsp/ttyak` (verify availability on the free plan).
  The README says how to verify, and how to build it yourself.
- Linux: nothing to sign for the OS; `.deb`/`.rpm` files unsigned until there are repositories (Claude's call).

### Accounts and who does what

The user does the steps only a person can (identity, payment, captchas). Claude does the rest (user: "you would manage
the rest"). **Secrets never appear in Claude's context or output** (user: yes to this reading): Claude may run commands
that create or use secrets — e.g. a script that generates the AUR key or the Apple certificate request and pipes the
private part straight into `gh secret set`, or one that reads the registrar's API key from a file the user wrote — as
long as no secret value is printed, logged or echoed. Secrets the user receives from a vendor (SSL.com, the Apple API
key, the Chocolatey API key, GitHub tokens for publishing) the user stores themselves (GitHub settings page or
`gh secret set` in their own terminal). The workflows only reference secrets by name.

For CI publishing the user creates once: a token that can push to the tap and bucket repos, a classic GitHub token
with `public_repo` scope for the winget pull requests from their account, and the Chocolatey API key; the AUR key is
generated by Claude's script straight into the secrets.

| Thing | The user | Claude |
|---|---|---|
| Domain `ttyak.sh` | buys it (Porkbun suggested: sells `.sh`, simple DNS API) and makes an API key | DNS records via the registrar's API; GitHub Pages custom domain and HTTPS via `gh` |
| GitHub | — (`gh` is signed in) | repo rename, tap / bucket repos, workflows, Pages |
| Apple | enrolls (ID, payment, 2FA), makes an App Store Connect API key | certificate request (a CSR) — via the API if Apple allows, else the user uploads it once —, signing and notarizing in CI |
| SSL.com | buys, passes ID checks, enrolls eSigner | CI signing |
| AUR | registers (captcha), adds the SSH key Claude generates | the package and pushes per release |
| Chocolatey | registers, makes an API key | the package and pushes per release |
| winget | — | a pull request per version (from the user's GitHub account) |

Accounts, the domain and the certificates take days (ID checks, reviews); they are set up while Part 1 is built.

## Non-goals / anti-goals

- No auto-update without consent; never a restart behind the user's back.
- Nothing ours runs as root or needs sudo / admin to install.
- No secrets in the repo, in logs, or in Claude's context.
- No telemetry: the update check is a plain GET of a static file.
- Don't hold releases on third-party review queues.

## Known traps (Claude's notes)

- Scoop runs a manifest's uninstaller during `scoop update` too: the purge must run only on a real uninstall, or
  every update wipes the user's data.

## Open for the builder / to verify on a real run

- Everything marked (verify) above.
- Which macOS runner signs (the build runs on macos-14 for both targets now) and how the certificate gets onto it.
- The Windows build is cross-compiled on Linux (MinGW); the SSL.com signer must run there (its CodeSignTool is Java).
- The exact `latest.json` schema; how the binary knows its own install method when `install.json` is missing.
- Brew formula from zip vs tar.gz; the formula's test block.

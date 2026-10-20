# klack landing page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A one-page site at `https://dmytrobsp.github.io/klack/` — the approved punk-collage first screen with the 6 fps "BROWSER./BULLSHIT." loop, then features, measured numbers vs the official app, SSH, downloads and footer.

**Architecture:** Hand-written static files in `site/` (HTML, one CSS file, two small ES modules). Python helpers in `tools/site/` fill marked regions of `site/index.html` from data files (release, measurements, captures) and check the site for private strings; they run locally and in a Pages workflow, which deploys `site/` as is. Captures are terminal text from `kitty @ get-text --ansi`, converted to HTML.

**Tech Stack:** HTML/CSS/vanilla JS (ES modules, no framework, no bundler); Python 3.12 stdlib (`unittest`, `html.parser`, `urllib`); Node 24 built-in test runner for the JS modules; GitHub Actions (`actions/upload-pages-artifact`, `actions/deploy-pages`); headless Chrome (flatpak `com.google.Chrome`) + `tools/cdp.mjs` for page checks.

**Spec:** `docs/superpowers/specs/2026-10-21-landing-page-design.md` (read it first). Look and motion reference (not shipped): `.superpowers/brainstorm/403418-1792528225/content/hero-v17.html` (the animation: SVG + 6 fps timeline, keyframes `K` and `KP`, `peel`, `outline`, `jag`) and `first-screen-v3.html` (the first screen's layout).

## Global Constraints

- Approved copy, verbatim: "SLACK," · "MINUS THE" · "BULLSHIT." · "BROWSER." · "YOUR TERMINAL JUST LEARNED TO HUDDLE." · "Yes, really." · "NO CHROMIUM WAS HARMED" · "Quit with Ctrl+Q. We're not monsters." · "Chat over SSH: yes. Huddle over SSH: only if you shout really loud." · "SSH forwards ports, not vocal cords."
- Never jab at slk or "other terminal clients"; slk appears only as a thank-you in the footer. Every jab targets the official Slack app.
- Never present huddles as working over SSH.
- Every number is measured (Task 9) or read from the release (Task 4); none is typed in by hand. Sizes in decimal MB with one decimal (`12.7 MB`), like PROGRESS.
- The workspace used for measuring is never mentioned on the page. The page says everything was measured on Linux.
- No real names, handles, emails, Slack ids, channel or workspace names anywhere in `site/` — `tools/site/check_private.py` must pass before every commit that touches `site/`. The forbidden list lives OUTSIDE the repo (`~/.config/klack-site/forbidden.txt`, 0600) and is never printed, quoted or committed.
- Never post under `--profile test` (the user's work workspace). Demo posts go to rslack-testbed (`--profile default` = A, `--profile b` = B); they need not be deleted.
- Colours: aubergine `#4A154B`, pink `#E01E5A`, blue `#36C5F0`, green `#2EB67D`, yellow `#ECB22E`, background `#160a17`, paper cream `#f3e9d2`, scrap `#f6f1e6`. Fonts (self-hosted woff2, OFL): Rubik Mono One, Anton, Permanent Marker, Space Mono 400/700.
- Paper pieces: crease texture, ragged straight-cut edges, `3px 3px 0 #000` shadow, no outline. The hand: emoji-style, `#ECB22E`, finger lines `#b07e17` only, `drop-shadow(3px 3px 0 #000)`.
- Animation: 6 fps, loop 8.0 s; reduced motion → the scrap shown hanging, nothing moves.
- No third-party requests from the page (no CDN fonts, no analytics, no API calls); no Slack logo; footer says klack is not affiliated with Slack Technologies or Salesforce.
- Project rules: never screenshot the desktop (grim/shot.sh) — headless Chrome rendering our own page is fine; live klack only in its own kitty window (`--class lp-<n> -o allow_remote_control=socket-only --listen-on unix:@lp-<n>`), always with `--profile default` or `--profile b` and a fake home with sounds off; never build `target/release`; never `pkill -f`/`pkill klack`; never capture the real mic, camera or screen (Task 9's screen share is the user's own pick at the portal); no `cargo fmt`.
- House workflow: each task adds one row INSIDE PROGRESS.md's table, `| LP<n> — <what> | done | UNIT/LIVE/NOT: <how> | <sha> |`, in the same commit (the sha is filled afterwards by python by row prefix); push master after each task and watch CI.
- Tests: `python3 -m unittest discover -s tools/site -p 'test_*.py'` and `node --test tools/site/` — both green before every commit.

## Review Focus

1. A phone (360–430 px wide): the first screen scales as one picture, the page never scrolls sideways, the loop stays legible — Task 11's width check.
2. JavaScript off or a font that fails to load: the headline text, the pitch and working download links are still in the HTML (filled at publish, not by script) — Task 4's static-links test, Task 5's no-JS test.
3. `prefers-reduced-motion` and a hidden tab / off-screen headline: no timers run, the scrap hangs — Task 2's tests.
4. A release whose asset list changes (a new platform, a missing zip) or a GitHub API failure at publish: unknown platforms land under "all downloads", a failed fetch keeps the committed `release.json` and the workflow says so — Task 4's tests.
5. Private data in a capture that survives renaming: a name split across two colour runs, an OSC 8 link carrying a real workspace URL, a QR code encoding a real email — Task 6's tests and Task 7's checks.

---

### Task 1: The site skeleton, tokens and fonts

**Files:**
- Create: `site/index.html`, `site/css/site.css`, `site/fonts/*.woff2`, `site/fonts/OFL-*.txt`, `site/data/release.json`, `site/data/numbers.json`
- Test: `tools/site/test_structure.py`

**Interfaces:**
- Produces: section ids `#top`, `#features`, `#numbers`, `#ssh`, `#get`, `#footer` (nav links point at them); fill markers `<!-- fill:NAME -->…<!-- /fill:NAME -->` for NAME in `hero-capture`, `download-button`, `downloads`, `sheet`, `measured-note`, `capture-huddle`, `capture-signin`, `capture-keys`, `capture-ssh`; CSS custom properties `--aubergine --pink --blue --green --yellow --bg --cream --scrap`; component classes `.cut`, `.paper`, `.sticker`, `.scrawl`, `.jab`, `.term`.
- `site/data/release.json` (committed fallback, Task 4 refreshes it): `{"version": "0.8.0", "url": "https://github.com/dmytrobsp/klack/releases/tag/v0.8.0", "sums_url": "…/SHA256SUMS", "assets": [{"name": "klack-0.8.0-x86_64-linux.zip", "platform": "x86_64-linux", "url": "…", "size": 12667135}, …]}` with the four real v0.8.0 assets (from `gh release view v0.8.0 --json assets`).
- `site/data/numbers.json`: `{"rows": []}` until Task 9.

- [ ] **Step 1: Write the failing test** — `test_structure.py`: parse `site/index.html` with `html.parser`; assert `<html lang="en">`, a `<title>` of `klack — Slack, minus the browser`, a meta viewport, each section id above present once and in that order, every nav `href="#…"` resolves to a section, every fill marker above present exactly once and closed, and no `http(s)://` in any `src`/`href` of `<link>`/`<script>`/`@font-face` except `https://github.com/dmytrobsp/klack…` (no third-party requests).
- [ ] **Step 2: Run** `python3 -m unittest discover -s tools/site -p 'test_*.py'` — FAIL (no index.html).
- [ ] **Step 3: Implement** the skeleton: nav (`klack` wordmark; features · the numbers · download · github), empty sections with headings, `site.css` with the tokens, `@font-face` for the four fonts from `site/fonts/` (`font-display: swap`), body `#160a17`, the paper/cut/sticker components as in `first-screen-v3.html` (the crease texture as an inline SVG data URI). Fonts: the latin woff2 files from `@fontsource/*` (jsdelivr) or Google Fonts' CSS2 downloads, with each family's `OFL.txt`.
- [ ] **Step 4: Run** the tests — PASS. Preview: `python3 -m http.server -d site 8765` and render `http://localhost:8765/` in headless Chrome (`flatpak run --filesystem=<scratch> com.google.Chrome --headless=new --user-data-dir=<scratch>/prof --screenshot=<scratch>/s.png --window-size=1280,900 URL`); Read the PNG: fonts load, dark page, nav.
- [ ] **Step 5: Commit** `site/ tools/site/test_structure.py PROGRESS.md` — "site: the landing page's skeleton, colours and fonts (LP1)".

### Task 2: The headline and its 6 fps loop

**Files:**
- Create: `site/js/hero.js`
- Modify: `site/index.html` (the first screen), `site/css/site.css`
- Test: `tools/site/hero.test.mjs`

**Interfaces:**
- Produces (ES module `site/js/hero.js`):
  - `export const FPS = 6, LOOP = 8.0;`
  - `export function quantize(t, fps)` → `Math.floor(t * fps) / fps` of `t % LOOP`
  - `export function pose(t)` → `{ phase: 'still'|'peel'|'fall'|'hang'|'slap', angle, len, wid, wave, taper, roll, printLen, hand: {x, y, rot, visible, follow} }` from the keyframe tables of `hero-v17.html` (`K` for everything except 1.5–3.0 s, `KP` + `peel()` inside it)
  - `export function peelShape({P, R, dang, bulge}, W, H)` → `{ flat: [[x,y]…], roll: {top: [[x,y]…], bot: [[x,y]…]} | null }` (paper-local coordinates; the Sutherland–Hodgman clip and roll of `hero-v17.html`)
  - `export const HANG_POSE` (the pose at t = 4.25 s, used for reduced motion)
  - `export function startHero(svg, {reduced = matchMedia('(prefers-reduced-motion: reduce)').matches} = {})` → `{ stop() }`; draws `HANG_POSE` once when `reduced`; otherwise one `setTimeout` chain at `1000 / FPS`, paused while the headline is off screen (`IntersectionObserver`) or the tab is hidden (`visibilitychange`).
- [ ] **Step 1: Write the failing tests** (`node:test`, importing `../../site/js/hero.js`): `quantize(1.0, 6) === 1.0`, `quantize(1.1, 6) === 1.0`, `quantize(8.1, 6) ≈ 0.0`; `pose(0).phase === 'still'` and `angle === 0`; `pose(2.2).phase === 'peel'`; `pose(4.25).angle` within 80–100 (hanging); `pose(5.85).phase === 'slap'` and `angle % 360 === 0`; `pose(7.9)` equals `pose(0)` in shape (the loop closes); `peelShape({P:0,R:0,dang:34,bulge:0},590,132).flat` covers the whole rectangle and `roll === null`; with `P:215` the flat part excludes `[590,0]` (the top-right corner peels first) and keeps `[0,132]`; `HANG_POSE.angle` within 80–100. `startHero` with `{reduced:true}` on a stub svg schedules no timer (stub `setTimeout` counts calls).
- [ ] **Step 2: Run** `node --test tools/site/` — FAIL.
- [ ] **Step 3: Implement** `hero.js` by porting the mockup's timeline and drawing (cloth path via `jag`, the print group transform, the roll, folds, tape, hand with the collage shadow, the neon flicker) with the pure parts exported; the DOM part only sets attributes. Markup in `index.html`: the cut-outs and the neon word as real text inside an `<h1 aria-label="Slack, minus the browser — well, the bullshit.">`; the SVG layer `aria-hidden="true"`. The pitch block, download button area (fill marker), capture area (fill marker) and the sticker as in `first-screen-v3.html`.
- [ ] **Step 4: Run** the tests — PASS. Headless check: screenshots with `--virtual-time-budget` 900, 2300, 4300, 5900 (ms) — still, peeling, hanging, slap; Read them; and one with `--force-prefers-reduced-motion` (Chrome flag) shows the hanging scrap.
- [ ] **Step 5: Commit** — "site: the headline — the browser scrap peels off the bullshit at 6 fps (LP2)".

### Task 3: The download button picks your system

**Files:**
- Create: `site/js/download.js`
- Modify: `site/index.html` (button script hook)
- Test: `tools/site/download.test.mjs`

**Interfaces:**
- Consumes: `site/data/release.json` shape (Task 1); the static button HTML that Task 4 fills (`<a class="dl" data-os="linux" href="…">`).
- Produces: `export function detectOS({userAgent, platform, maxTouchPoints})` → `'linux'|'windows'|'macos'|'mobile'|'unknown'`; `export function choose(os, release)` → `{ label: string, assets: [{name, platform, url, size}] }`; `export function enhance(doc, release, nav = navigator)` swaps the static default (Linux) for the visitor's system.
- [ ] **Step 1: Tests:** a Linux Firefox UA → `'linux'`, Windows 11 Chrome → `'windows'`, macOS Safari → `'macos'`, iPadOS (`platform 'MacIntel'`, `maxTouchPoints 5`) → `'mobile'`, Android → `'mobile'`, empty → `'unknown'`; `choose('macos', rel).assets` is both macOS zips with labels "Apple Silicon" / "Intel"; `choose('windows', rel).label === 'DOWNLOAD FOR WINDOWS'`; `choose('mobile'|'unknown', rel)` → label `ALL DOWNLOADS` linking `#get`; an asset with an unknown platform (`riscv64-linux`) is never chosen but stays in `release.assets`.
- [ ] **Step 2: Run** — FAIL. **Step 3: Implement.** **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** — "site: the download button picks your system (LP3)".

### Task 4: Filling the page from data, and the release

**Files:**
- Create: `tools/site/fill.py`
- Modify: `site/index.html` (filled regions), `site/data/release.json`
- Test: `tools/site/test_fill.py`

**Interfaces:**
- Produces: `fill(html: str, name: str, content: str) -> str` (replaces between the markers, keeps them; `KeyError` naming the marker if absent); `fmt_size(n_bytes: int) -> str` (`12667135 → "12.7 MB"`); `render_download_button(release: dict) -> str` (Linux default + "v0.8.0 · 12.7 MB" + macOS · Windows · all downloads links); `render_downloads(release: dict, needs: dict) -> str` (the Get it table: one row per known platform in the order Linux, macOS Apple Silicon, macOS Intel, Windows, then unknown platforms; size; what it needs from the README's Install table; the SHA256SUMS link); `release_from_api(repo: str, token: str | None) -> dict`; `fill_page(site: Path, release: dict | None = None, numbers: dict | None = None) -> None` (Task 10 adds the sheet); CLI `python3 tools/site/fill.py site [--fetch-release]` — with `--fetch-release`, a failed fetch prints `release fetch failed: <reason>; keeping site/data/release.json` to stderr and exits 0, a fetched release overwrites `release.json`.
- [ ] **Step 1: Tests:** `fill` replaces only between its markers and is idempotent; missing marker → `KeyError('download-button')`; `fmt_size(12667135) == '12.7 MB'`, `fmt_size(999) == '0.0 MB'`; `render_download_button` of the v0.8.0 fixture contains `href="https://github.com/dmytrobsp/klack/releases/download/v0.8.0/klack-0.8.0-x86_64-linux.zip"` (works without JS) and `12.7 MB`; `render_downloads` with an extra `riscv64-linux` asset lists it last; `release_from_api` against a stub opener: a 500 raises `ReleaseFetchError`; the CLI with a failing fetch keeps the file byte-identical.
- [ ] **Step 2–4:** run (FAIL), implement, run (PASS); `python3 tools/site/fill.py site` fills the page; headless screenshot shows the button with the real size.
- [ ] **Step 5: Commit** — "site: download links and sizes filled from the release (LP4)".

### Task 5: The words — every section and its jab

**Files:**
- Modify: `site/index.html`, `site/css/site.css`
- Create: `site/img/sad-smiley.svg`
- Test: `tools/site/test_copy.py`

**Interfaces:**
- Consumes: section ids and components (Task 1), fill markers for captures (Task 7) and the sheet (Task 10).
- [ ] **Step 1: Tests:** every approved line of Global Constraints appears verbatim in the HTML text; the phrases `other terminal clients`, `other clients`, `the others can't` do not appear; `slk` appears only inside `#footer`; inside `#ssh`, any sentence with `huddle` also contains `shout` or `vocal` (the jokes) and none claims it works; each of `#features`, `#numbers`, `#ssh`, `#get` holds at least one `.jab` element; the features grid lists only items present in README.md (a list of `(item, README phrase)` pairs in the test, each phrase asserted in README.md); the text content with all `<script>` removed still contains the headline, the pitch and a download `href` (no-JS).
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Write the sections** per the spec's "The rest of the page": What it does (huddles + screen sharing; one-code sign-in; the keyboard with the `:q` sticker; `klack ctl` with a jab that the official app's automation is a browser; the everything-else grid), Over SSH (the two approved jokes, the sad-smiley graffiti beside the huddle one, chat/threads/search over SSH, sign-in by code), Get it (filled table, `klack auth login`, unsigned-build notes, per-platform honesty: huddles at home on Linux, camera/screen sharing less tested on macOS/Windows), footer (MIT or Apache-2.0, GitHub, not affiliated with Slack Technologies or Salesforce — Slack is their trademark, thanks to slk for the inspiration, "no chromium was harmed"). Draft copy is short, in the page's voice; jabs at the official app only.
- [ ] **Step 4: Run** — PASS; headless full-page screenshot (`--window-size=1280,5000`), Read it.
- [ ] **Step 5: Commit** — "site: every section's words, each with a jab (LP5)". The user reviews the copy at Task 11.

### Task 6: Terminal text to HTML, and the privacy check

**Files:**
- Create: `tools/site/ansi2html.py`, `tools/site/check_private.py`
- Test: `tools/site/test_ansi2html.py`, `tools/site/test_check_private.py`

**Interfaces:**
- Produces: `convert(ansi: str, rename: dict[str, str] | None = None) -> str` → `<pre class="term">…</pre>` with `<span style="…">` runs (SGR 0, 1, 2, 3, 4, 7, 9, 22–29, 30–37, 38;5;n, 38;2;r;g;b, 39, 40–47, 48;5;n, 48;2;r;g;b, 49, 90–97, 100–107; the xterm-256 palette), HTML-escaped, OSC sequences (OSC 8 links included) dropped entirely; `rename` is applied to the plain text of the whole screen before styling is re-applied (so a name split across runs is still renamed). `scan(root: Path, forbidden: list[str]) -> list[tuple[str, int, int]]` (path, line, index into `forbidden`; case-insensitive; also matches with spaces/dots/underscores removed); CLI `python3 tools/site/check_private.py site` reads `~/.config/klack-site/forbidden.txt` (or `$KLACK_SITE_FORBIDDEN`), prints only `path:line: forbidden string #N`, exits 1 on any hit and 2 if the list is missing.
- [ ] **Step 1: Tests:** bold red `\e[1;31mx\e[0m` → a bold `#…` span; `38;2;236;178;46` → `color:#ecb22e`; reverse video swaps colours; `<&>` escaped; an OSC 8 link `\e]8;;https://team.slack.com/archives/C1\e\\label\e]8;;\e\\` leaves `label` and no `slack.com`; `rename={'Alice Smith': 'Bob'}` renames `\e[1mAlice \e[0;36mSmith\e[0m`; `scan` finds `alice.smith` for forbidden `Alice Smith`, reports `(path, line, 0)` and never the string; the CLI output never contains a forbidden string (asserted on captured stdout).
- [ ] **Step 2–4:** FAIL, implement, PASS.
- [ ] **Step 5: Commit** — "tools/site: terminal captures to HTML; a privacy check for the site (LP6)".

### Task 7: Real captures from the test workspace (LIVE)

**Files:**
- Create: `site/captures/{hero,huddle,signin,keys,ssh}.html`, `tools/site/CAPTURES.md` (how each was made, no private data)
- Modify: `site/index.html` (via `fill.py`), `tools/site/fill.py` (fills `capture-*` and `hero-capture` from `site/captures/`)
- Outside the repo: `~/.config/klack-site/forbidden.txt` and `~/.config/klack-site/rename.json` (0600)

**Interfaces:**
- Consumes: `convert`, `scan` (Task 6), `fill` (Task 4).
- [ ] **Step 1: The private lists.** Build `forbidden.txt` from the test accounts' own profiles (`klack ctl --profile default whoami`, `--profile b whoami`, their real names, display names, handles, emails, user ids, the team id, the workspace name and domain, and every real channel name of rslack-testbed from `ctl channels`) and `rename.json` mapping each to a made-up value — written by a script straight to the files, never printed.
- [ ] **Step 2: Demo content** in rslack-testbed (`--profile default` / `--profile b`): demo channels (e.g. `#deploys`, `#coffee`) via `ctl api conversations.create`, a short made-up conversation between A and B with a thread, a reaction, a code block and an in-app link — made-up words only, nothing posted anywhere else.
- [ ] **Step 3: Capture** each screen in a test kitty window (fake home, sounds off, `kitty @ set-font-size` to fit ~110×32): the main view (hero), the which-key panel (keys), the sign-in guide's code screen started with a made-up email under a fake HOME (`klack auth login --email demo@example.com` up to the QR, then Ctrl+C before any request is confirmed; the QR then encodes only the example address — check by decoding it, e.g. `zbarimg` on a headless render of the capture), a huddle between A and B with B's synthetic camera and screen share (`ctl huddle-media … --video-test --share-test`; the video tiles are images, not text: render those cells from a synthetic `ffmpeg testsrc2` frame placed into the HTML capture), and the SSH screen (the same main view in a narrower window). `kitty @ get-text --extent screen --ansi > capture.ansi`, then `convert(…, rename)` into `site/captures/<name>.html`. Quit klack with Ctrl+Q; remove the test drafts and the fake home.
- [ ] **Step 4: Check:** `python3 tools/site/check_private.py site` exits 0; `python3 tools/site/fill.py site`; headless screenshot of each section, Read them; the unit tests pass.
- [ ] **Step 5: Commit** — "site: real captures from the test workspace, made-up names only (LP7)".

### Task 8: A measuring tool for an app and all its helpers

**Files:**
- Create: `tools/site/measure.py`
- Test: `tools/site/test_measure.py` (with a fake `/proc` tree under a temp dir)

**Interfaces:**
- Produces: `read_procs(proc: Path) -> dict[int, Proc]` (`Proc(pid, ppid, comm, exe)` from `stat`, `comm`, `exe` link — unreadable entries skipped); `tree(procs, roots: set[int]) -> set[int]` (roots and all descendants); `roots_matching(procs, comm: str) -> set[int]` (matching processes whose parent doesn't match); `pss_kb(pids, proc: Path) -> int` (sum of `Pss:` in `smaps_rollup`); `cpu_ticks(pids, proc: Path) -> int` (utime+stime); CLI `measure.py ram --comm NAME [--repeat N --every S]` and `measure.py cpu --comm NAME --seconds S` printing one JSON line each (`{"what": "ram", "comm": …, "pids": n, "pss_kb": …, "at": iso}`, `{"what": "cpu", "percent_of_one_core": …}`), and `measure.py startup --comm NAME --until-file PATH` (wall time from launch until a marker appears; used with klack's log line and the official app's first conversation load — Task 9 decides each app's marker).
- [ ] **Step 1: Tests:** a fake tree slack(10) → slack(11), slack(12) → zygote(13); `roots_matching(…, 'slack') == {10}`, `tree == {10,11,12,13}`; `pss_kb` sums 100+200+300+400; a process vanishing mid-read is skipped, not fatal; `cpu_ticks` over two samples gives the delta.
- [ ] **Step 2–4:** FAIL, implement, PASS; a live smoke run on the user's own running klack (read-only: `measure.py ram --comm klack`) prints a sane number.
- [ ] **Step 5: Commit** — "tools/site: measure an app's RAM and CPU over its whole process tree (LP8)".

### Task 9: Measuring klack and the official app (LIVE, with the user for the screen-share run)

**Files:**
- Create: `site/data/numbers.json`, `tools/site/MEASURING.md` (the procedure, versions, date, machine — no workspace facts)

**Interfaces:**
- Consumes: `measure.py` (Task 8).
- Produces: `numbers.json`: `{"measured_on": "YYYY-MM-DD", "machine": "<CPU>, <RAM>, Linux <kernel>", "versions": {"klack": "0.8.0", "slack": "4.52.178"}, "rows": [{"id": str, "label": str, "unit": "bytes"|"kB"|"percent"|"seconds", "klack": number, "slack": number}]}` with ids `download`, `disk`, `ram_rest`, `ram_busy`, `cpu_idle`, `startup`, `cpu_share`, `ram_share` — every row measured, none dropped.
- [ ] **Step 1: Sizes.** klack: the v0.8.0 Linux zip size and the unzipped binary (`release.json`, `du -b`). Official: the download of Slack's own Linux package (the size Slack's download page serves, `curl -sIL` Content-Length) and the installed app (`du -sb` of the flatpak's `files/`).
- [ ] **Step 2: Driving the official app** — launch `flatpak run com.slack.Slack --remote-debugging-port=<p>` and check `tools/cdp.mjs` can list its page; if not, fall back to `hyprctl dispatch sendshortcut` key events to its window. In the user's work account only open conversations (Ctrl+K + name of a conversation the user names, or the sidebar's next item) and scroll (PageUp); never type into a message box, send, react or change a setting.
- [ ] **Step 3: Rest and busy, both apps on the work workspace** (official: the user's own sign-in; klack: the downloaded v0.8.0 binary with `--profile test`, never signed in or out): launch, `startup` timing, settle 120 s, `ram` ×3 and `cpu --seconds 60` = rest; the same session in each (open the same 5 conversations, PageUp ×20 in each) then `ram` ×3 = busy. One app at a time; quit it fully before the next.
- [ ] **Step 4: Huddle + screen sharing, test workspace** (agreed: user): add rslack-testbed to the official app as A (the user signs in or Claude via the email code — ask), klack as B in its own kitty window. Before joining: a PipeWire null sink + its monitor made default source for the run (`pactl load-module module-null-sink …`, restore + unload in a trap), the official app's input checked on it; klack B muted. The official app shares one synthetic window (an `ffplay -f lavfi testsrc2` window) — tell the user just before the portal picker opens; the user picks it; if the user isn't there, cancel the picker (`hyprctl dispatch closewindow class:hyprland-share-picker`). Measure `cpu --seconds 60` + `ram` for the official app while sharing (klack B watching). Then the roles swap: klack shares as B with `klack ctl --profile b huddle-media <chan> 1 --share-test` (klack's own capture-and-encode path fed a synthetic source) while the official app watches, and the same measurements run on the klack process. `MEASURING.md` says plainly that the official app captured a real window and klack a synthetic source. Nothing is recorded. Afterwards remove the test workspace from the official app, restore audio defaults.
- [ ] **Step 5: Write** `numbers.json` and `MEASURING.md`; `check_private.py site` passes; commit — "site: the numbers, measured (LP9)".

### Task 10: The defaced spec sheet

**Files:**
- Modify: `tools/site/fill.py`, `site/index.html`, `site/css/site.css`
- Test: `tools/site/test_fill.py`

**Interfaces:**
- Consumes: `numbers.json` (Task 9), `fill` (Task 4).
- Produces: `render_sheet(numbers: dict) -> str` — two columns, "the official app" with each value in a `.crossed` span and klack's in a `.circled` span, a `.scrawl` per row with the ratio (`"8.4× less"`) — or, when the ratio is under 1.5, a self-mocking tie scrawl (`"ok, a tie. it's a chat app."`); `render_measured_note(numbers: dict) -> str` — versions, date, machine, "measured on Linux" with the freedom joke; never the workspace; `fill_page` fills `sheet` and `measured-note`.
- [ ] **Step 1: Tests:** a fixture with all eight rows renders eight rows in the fixture's order with both values formatted by unit (`kB` → `MB` one decimal, `percent` one decimal, `seconds` one decimal); a tie row gets the tie scrawl and no ratio; an empty `rows` renders a visible "measuring…" placeholder (never fake numbers); the note contains `Linux` and the versions and none of `workspace`, `team`.
- [ ] **Step 2–4:** FAIL, implement, PASS; fill; headless screenshot of `#numbers`, Read it.
- [ ] **Step 5: Commit** — "site: the official app's numbers, crossed out (LP10)".

### Task 11: Phones, motion, speed — then the user's review

**Files:**
- Modify: `site/css/site.css`, `site/index.html`, `site/js/hero.js` (only if a check fails)
- Create: `tools/site/pagecheck.mjs`

**Interfaces:**
- Produces: `node tools/site/pagecheck.mjs <url>` — drives headless Chrome over CDP (as `tools/cdp.mjs` does) at widths 360, 390, 768, 1280 and prints JSON: `scrollWidth <= innerWidth` per width, the first screen's stage scale, console errors, requests to other hosts (must be none), total transfer size.
- [ ] **Step 1:** the first-screen stage (836 px design) scales as one unit (`transform: scale()` from its container width, or an SVG `viewBox` for the whole stage) so nothing reflows inside the collage; sections stack on phones.
- [ ] **Step 2: Run** `pagecheck.mjs http://localhost:8765/` — every width passes, no console errors, no foreign requests; Read headless screenshots at 390 and 1280.
- [ ] **Step 3:** reduced motion + hidden tab verified (Task 2's tests plus a pagecheck run with `--force-prefers-reduced-motion`).
- [ ] **Step 4: The user's review** of a local preview (`python3 -m http.server -d site 8765`, the user opens `http://localhost:8765/`): the copy (drafts marked in the commit message), the captures, the numbers. Apply their changes in follow-up commits (`LP11 fix …` rows).
- [ ] **Step 5: Commit** — "site: phones, reduced motion and a page check (LP11)".

### Task 12: Publishing on GitHub Pages

**Files:**
- Create: `.github/workflows/pages.yml`
- Modify: `README.md` (a link to the page at the top), `PROGRESS.md`

**Interfaces:**
- Consumes: `fill.py --fetch-release` (Task 4), the tests.
- [ ] **Step 1:** `pages.yml`: on `push` to master with paths `site/**`, `tools/site/**`, `.github/workflows/pages.yml`; on `workflow_run` of workflow `Release` with `types: [completed]` (only if it succeeded); and `workflow_dispatch`. Jobs: `build` (checkout, `python3 -m unittest discover -s tools/site -p 'test_*.py'`, `node --test tools/site/`, `python3 tools/site/fill.py site --fetch-release` with `GITHUB_TOKEN`, `actions/upload-pages-artifact` of `site/`) → `deploy` (`actions/deploy-pages`, `permissions: pages: write, id-token: write`, environment `github-pages`). `release.yml` is not touched. The private check cannot run in CI (its list is not in the repo) — it runs locally before every commit (Global Constraints).
- [ ] **Step 2: Ask the user** to approve turning Pages on (repo settings → Pages → Source: GitHub Actions; or `gh api -X POST repos/dmytrobsp/klack/pages -f build_type=workflow` with their OK). Only after the user approved the page in Task 11.
- [ ] **Step 3:** push, watch the Pages run green, open `https://dmytrobsp.github.io/klack/` headlessly and run `pagecheck.mjs` against it; check the download link resolves (`curl -sIL` → 200).
- [ ] **Step 4:** README gets the link; PROGRESS's NEXT UP updated.
- [ ] **Step 5: Commit** — "site: published on GitHub Pages (LP12)".

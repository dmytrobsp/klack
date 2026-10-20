# klack's landing page — design

Asked by the user on 2026-10-20: "create a cool landing page and host it on github.io … specifically want it to
focus on streamlined login, huddles, screensharing, TUI, size, performance and RAM consumption especially when
compared with official app. let's take a couple of punches at original slack, it deserves that."

Brainstormed 2026-10-20/21 with the visual companion; the mockups are in
`.superpowers/brainstorm/403418-1792528225/content/` (not committed; the latest first screen is
`first-screen-v3.html`, the headline animation alone `hero-v17.html` — both are the reference for the look and the
motion, not code to ship as is). In this document **(user)** marks what the user decided, **(Claude)** marks
Claude's calls, which the user can overrule at review.

## Why, and for whom

The visitor is tech-savvy — a programmer or a sysadmin **(user)** — and comes for one of three reasons **(user)**:

- they love TUIs and all things terminal;
- they want to flip a finger at the official client, which is bloated and slow;
- they need a Slack client that works over SSH on a headless box.

The message **(user)**: klack may be small, but it does what the others can't — huddles, screen sharing, a
sign-in that takes one code. The page says it with attitude and humour, and takes jabs at the official Slack app
regularly through the page, not only in one section **(user)**.

Success **(Claude)**: a visitor gets the joke and the pitch in the first screen, finds a number that makes the
official app look silly, and has the right download in one click.

## Goals, non-goals, anti-goals

Goals:
- One page at `https://dmytrobsp.github.io/klack/` with the sections below, in that order **(user)**.
- Every number on the page is measured, not guessed, and says what was measured and how (versions, date, machine)
  **(user: "numbers must be measured")**.
- Works on a phone as well as a desktop (Claude).

Non-goals:
- No documentation site — the README stays the manual; the page links to it (Claude).
- No blog, no changelog page, no newsletter, no analytics (Claude).

Anti-goals — the page has failed if:
- it jabs at slk (getslk.sh) or any other terminal client, or says "other terminal clients" / "the others can't"
  in a way that points at them **(user: "no jabs at slk directly. inspiration acknowledgment at most"; "other
  terminal clients is a way too direct jab at slk")**. Every punch goes at the official Slack app.
- it pairs huddles with SSH anywhere — they do not work together **(user: "huddles over ssh do not work! we're
  not wizards")**.
- it looks corporate: slack.com, GitHub's pages, Wealthsimple and DoorDash are the examples of what the user finds
  cringe **(user)**. getslk.sh is a page the user likes (terse, developer voice, stat tiles, install tabs).
- it shows a number nobody measured, a real person's name, email or Slack id, or anything from the user's work
  workspace other than the totals of a measurement (Claude, from the project's hard rules).
- it impersonates Slack: no Slack logo, no Slack fonts, no "official" wording (Claude).

## The look **(user)**

Slack's colours (aubergine `#4A154B`, pink `#E01E5A`, blue `#36C5F0`, green `#2EB67D`, yellow `#ECB22E`) on a
dark, near-black aubergine background (`#160a17` in the mockups), with "that wild rock/punk/grunge aesthetic of
2000s but polished and modern". From the style rounds the user kept A (ransom-note zine: cut-out letters, tape,
stickers) and C (xerox gig poster: huge condensed type, pasted colour blocks), plus the line "no chromium was
harmed" and the defaced-spec-sheet idea. Fonts in the mockups: Rubik Mono One (cut-out letters), Anton (poster
type), Permanent Marker (scrawls, neon), Space Mono (body, terminal). Paper pieces: lightly crumpled paper (a
crease texture), ragged straight-cut edges, a hard 3 px black offset shadow, no outline — "SLACK," and
"MINUS THE" are the model the user called "very cool".

## The first screen **(user, approved in the mockups)**

- A slim nav: the `klack` wordmark; features · the numbers · download · github.
- The headline, ransom-note style: **"SLACK,"** on a pink paper cut-out and **"MINUS THE"** on a cream one; under
  them **"BULLSHIT."** in a glowing pink neon marker (Permanent Marker with a layered glow that flickers now and
  then). Over it a scrap of the same paper reading **"BROWSER."**, taped at its top-left corner, with a light brown
  dragged smear on it (a smear, not a dark stain).
- The loop, at **6 fps** (stop-motion; "6 fps is perfect"), about 8 s, repeating: the scrap sits still; the wind
  peels it from its **top-right corner** — an imperfect, lumpy roll travels **down and to the left** along a
  slightly bent fold until it reaches the tape; the roll drops, unrolls and the scrap **hangs limp from the tape
  like a sad piece of dirty cloth**, swaying; then a hand sweeps it back up and **slaps it flat** over the word.
  The hand: an emoji-style hand in Slack yellow, no outline, no paper texture, darker lines between the fingers
  only (no line between thumb and index finger), with the collage's hard black shadow. It is our own drawing —
  Apple's emoji (what Slack shows) cannot be copied onto a public page (Claude).
- The pitch: **"YOUR TERMINAL JUST LEARNED TO HUDDLE."** in poster type, with **"Yes, really."** scrawled under
  it in yellow marker. Short and sharp; the details live on the second screen **(user: "i want the pitch short and
  sharp. exact details can come on the second screen")**.
- A big download button for the visitor's system ("DOWNLOAD FOR LINUX"), with the version and the download size
  beside it and links to macOS, Windows and all downloads.
- A tilted klack window — a real capture (see Captures), not the mockup's stand-in.
- The sticker **"NO CHROMIUM WAS HARMED"**.

(Claude) With `prefers-reduced-motion`, the scrap is shown hanging (the joke still reads) and nothing moves; the
loop runs only while the headline is on screen. The headline's accessible text is "Slack, minus the browser" with
the struck word available to screen readers as part of the joke ("minus the browser — well, the bullshit"); the
builder may phrase it better.

## The rest of the page, in order **(user: "more or less, yes. but with humour and jabs at slack periodically")**

Each section carries at least one jab at the official app or a joke, in the collage language (a crossed-out line,
a scrawl, a sticker). Lines marked **(user)** are approved word for word; the rest are drafts the builder may
improve, and the user approves the final copy at review.

1. **What it does** — one block per feature, each with a real capture and one punchy line:
   - Huddles and screen sharing: join, talk, share your screen and watch others', drawn in the terminal.
   - Sign in with one code: scan the QR code with your phone, type the code from your email, done; no browser
     needed on that machine. (The guide also offers your browser's sign-in and pasting a cookie.)
   - The keyboard: NORMAL and INSERT modes, `j`/`k`, `g`/`G`, Ctrl+U/Ctrl+D, a Space leader with a which-key panel,
     and the mouse for anyone who wants it. With the `:q` meme **(user: "i like the ":q" meme")**: **"Quit with
     Ctrl+Q. We're not monsters."** as a sticker or aside.
   - Scripting: `klack ctl` — a command-line control channel to the same Slack (`whoami`, `channels`, `history`,
     `post`, `react`, `upload`, `search`, `listen`, …) — boast about it **(user: "might as well boast about that
     too")**, with a jab (the official app's automation story is a browser).
   - Everything else, as a quick grid: threads, reactions and custom emoji, pictures in the terminal, files, search,
     notifications and Do Not Disturb, several workspaces, themes, links into your Slack that open in klack — checked
     against the README before it ships, nothing listed that klack doesn't do.
2. **The numbers** — the defaced spec sheet: the official app's column crossed out in red marker, klack's circled,
   scrawled remarks. The figures (see Measuring): download size, size on disk, RAM at rest and when busy, CPU at
   idle, startup time, and a huddle with screen sharing (CPU and RAM) — "original slack is notorious for very
   CPU-heavy screen sharing" **(user)** — with a "how we measured" note. The note says plainly that everything was
   measured on Linux, with a joke on Linux superiority in the spirit of "can't hear you over the sound of my
   freedom" **(user)**; it never mentions the workspace used, not even that it exists **(user: "better not to
   mention it at all")**.
3. **Over SSH** — for the admin on a headless box: sign in with a code over SSH, then chat, threads and search.
   Headline joke **"Chat over SSH: yes. Huddle over SSH: only if you shout really loud."** and the aside **"SSH
   forwards ports, not vocal cords."** **(user: "#1 and #3 are both hilarious")**, with a **sad-smiley graffiti**
   beside the huddle joke **(user)**. Huddles and SSH never appear as working together.
4. **Get it** — the four downloads (one zip each: Linux x86_64, macOS Apple Silicon, macOS Intel, Windows x86_64),
   what each needs (from the README's Install table), the SHA256SUMS link, and the first command:
   `klack auth login`. Unsigned builds are said plainly (macOS quarantine note, SmartScreen). Honest per-platform
   notes from the README: huddles are at home on Linux; on macOS and Windows the camera and screen sharing are
   less tested; everything should work there, but everything was measured on Linux (user). The first screen's
   huddle claim stays platform-neutral.
5. **Footer** — the licence (MIT or Apache-2.0), the GitHub link, "klack is not affiliated with Slack Technologies or
   Salesforce; Slack is their trademark" (Claude), a small thank-you to slk for the inspiration **(user: "inspiration
   acknowledgment at most")**, and "no chromium was harmed" once more.

## Measuring **(user decided where; Claude the method)**

- **Where (user):** the user's real work workspace — the official desktop app (flatpak `com.slack.Slack`, 4.52.178
  on this machine) on the user's own sign-in, klack on the user's `test` profile. Both signed in to the same
  workspace, doing the same things. The user offered to open the official app and scroll through history to make
  the "busy" figure honest; Claude may open the official app (user: "you can open it, no problem").
- **Method (Claude):** the same machine, one app at a time; RAM as the sum of PSS over the whole process tree
  (Electron's helpers included) from `/proc/*/smaps_rollup`; "at rest" after a fixed settle time with nothing
  touched, "busy" after the same scripted-by-hand session in both (open the same conversations, scroll the same
  history); CPU at idle as the average over a fixed minute; startup as wall time from launch until the first
  conversation is shown; download size of the official Linux package from Slack's own download, size on disk of
  the installed app; klack's figures from the v0.8.0 release (the shipped binary, not a dev build). Record the
  versions, the date and the machine (Linux) on the page. Only totals are published; the workspace is not
  mentioned at all **(user)**.
- **Who drives (user: "can you drive both apps, for the sake of accuracy?")**: Claude drives both, the same session
  in each. klack through its kitty test window (`kitty @ send-key`) or `klack ctl`. The official app (Electron;
  Node's inspector fuses are off, so the routes are Chromium's `--remote-debugging-port` if the app honours it,
  else Hyprland's `sendshortcut` key events to its window) — probed once at the start of the measuring step. In
  the user's work account Claude only opens conversations and scrolls: never types into a message box, sends,
  reacts, or changes a setting.
- **Huddle and screen sharing (user: "i'm okay with huddle capture")**: measured in the test workspace, not the
  work one (Claude) — the official app needs the test workspace added for the run (and removed after), klack runs
  as the other test account. The project's rules still hold: the official app's microphone goes to a silent
  PipeWire null source made default for the run (restored by a trap) and checked before joining; screen sharing
  shares one synthetic window (an ffmpeg `testsrc` player) that the user picks at the portal picker themselves —
  Claude says just before it opens, never picks on their behalf, and cancels the picker if the user isn't there;
  nothing is recorded. The exact setup is confirmed with the user at the start of that run.
- **Rows where klack doesn't clearly win (Claude):** the metric list is fixed before measuring and every measured
  row is kept — the page is honest; a row where the two are close (CPU at idle may be ~0 % for both) gets a
  self-mocking scrawl instead of a circle (e.g. "ok, a tie. it's a chat app."), never a fudge. Copy for the sheet is
  written after the results are in. (The user: "you can bet there wont be any".)
- **Constraints:** klack's `test` profile is the user's daily work account: never sign it in or out, and opening
  conversations in either app marks them read, as in normal use (the user asked Claude to drive both). Never
  build `target/release` (the user runs it); measure the downloaded release binary.

## Captures **(Claude)**

- The klack windows on the page are real: screen text and colours taken with `kitty @ get-text --ansi` from a
  test window and turned into HTML (crisp, selectable, light). Never take desktop screenshots (grim / shot.sh once
  captured the user's own browser); a headless browser rendering our own page is fine.
- Captures come from the test workspace (rslack-testbed, accounts A and B), never the work workspace, and show
  **no names, no channel titles, no real messages (user)**: every name, channel, workspace name and message on
  the page is made up — staged demo content (posted in the test workspace for the capture and deleted afterwards)
  or rewritten in the capture-to-HTML step (Claude: the builder picks per capture). A check fails the build if any
  real name, handle, email, id, channel or workspace name of the test accounts appears anywhere in `site/`.
- Pictures inside klack (profile pictures, the huddle's video tiles, a shared screen) are not in the text capture;
  the builder composes them back in from synthetic sources only (the project's video test rules: synthetic
  senders, a virtual camera, `ffmpeg testsrc`; never the real mic, camera or screen).

## How it is built and hosted **(Claude)**

- Hand-written HTML, CSS and a little JavaScript, no framework and no build step beyond the publish workflow, in
  `site/` in the klack repo; fonts self-hosted (all four are OFL), no third-party requests, no tracking.
- Published to GitHub Pages by its own workflow (`actions/deploy-pages`) on pushes that touch `site/`, on
  `workflow_run` of the Release workflow completing (a release made by `gh release create` with the built-in token
  starts no `release` event, but the Release run itself was started by the tag push, so `workflow_run` fires), and
  by hand; it fills in the latest version, the asset links and their sizes from the release, so the download button
  never goes stale and makes no API calls from the visitor's browser. `release.yml` is not changed (Claude).
- The download button picks the visitor's system from the browser (Linux / Windows; macOS offers both builds,
  since Apple Silicon vs Intel can't be told reliably); unknown systems get "all downloads".
- Enabling Pages on the repo is a repo-settings change: ask the user before doing it. The user reviews the page
  as a local preview first; Pages is turned on (and the page published) only after the user approves it (Claude).

## Left to the builder

The page's exact layout below the first screen, the responsive behaviour (the 836 px first-screen stage must
scale down for phones), the draft copy marked as drafts, the sad-smiley's style, the art of each section's jab,
the "everything else" grid's look, how the captures are framed, and how the animation is implemented (the mockup
uses SVG + a 6 fps JavaScript timeline — the motion and the art in `hero-v17.html` are the reference).

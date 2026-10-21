# How the page's terminal captures were made (LP7, redrawn in LP7b, re-captured in LP12a and LP12b)

`site/captures/{hero,keys,huddle,signin}.html` are fragments that `python3 tools/site/fill.py site` puts into
the page's `hero-capture` and `capture-<name>` regions (a region without its file keeps what it has). They are
built by `python3 tools/site/captures.py RAW_DIR site/captures` from raw kitty captures (`<name>.ansi`, never
committed), `~/.config/klack-site/rename.json` and `~/.config/klack-site/fixes.json` (both outside the repo, 0600,
never printed or committed) and styled by the "terminal captures" block of `site/css/site.css`.

## Drawn like a terminal (LP7b)

Each fragment is a `<div class="term capture">` holding one SVG of the screen on an exact cell grid
(`tools/site/ansi2html.py`), scaled so `--cols` cells fill the box:

- **Cells.** A cell is 8 × 18 units: kitty's cell where the captures were made is 12 × 27 device pixels
  (JetBrains Mono 12 pt at 1.25×). Every cell of every row is kept, blank ones included; EL (`CSI K`) would clear
  with the current background (kitty's `get-text` writes cells out, so the captures have none).
- **Backgrounds** are whole cells, one path a colour, so rows are solid and nothing shows between them. What kitty
  holds is what is drawn: klack's selection background covers the selected message's text (ratatui paints a
  line's style on its characters), the code block's covers its padded box.
- **Box drawing** (U+2500–257F) and **block elements** (U+2580–259F) are geometry, the way kitty draws them
  itself: lines from the cell's edge through its centre (light 1.33, heavy 2.67 units — kitty's 1 pt and 2 pt),
  merged where they touch, so they join across cells and rows; block elements tile their cell together with its
  background (the QR code's half blocks have no seams).
- **Text** is real, selectable text in Space Mono (self-hosted, 0.6125 em advance at a size that makes one
  character one cell): each ASCII run is pinned to its first cell and stretched to exactly its cells
  (`textLength`), every other character is centred on its own cell(s). Wide characters (emoji, CJK, and narrow
  emoji followed by VS16, as kitty draws them) take two cells.
- **Colours** are those of the kitty the captures were made in (`kitty @ get-colors`: foreground `#cdd6f4`,
  background `#1e1e2e`, its 16-colour palette — so SGR 30 is `#45475a`, not black), dim is kitty's
  `dim_opacity` 0.4, and the screen's padding is the terminal's background, as in kitty.
- **Names.** `rename_cells` replaces the real strings cell by cell: each cell keeps its own style (the initials
  keep their coloured background, the name next to them doesn't get it); a value shorter or longer than what it
  replaces is padded or cut, so the layout never moves.
- OSC (links), DCS, APC (kitty pictures), PM and SOS payloads are dropped; kitty's picture placeholder cells are
  blanked and reported, so the page can put a picture over them.

## Names

Everything shown is made up. The conversation was posted for the page in the test workspace by its two test
accounts (A = `--profile default`, B = `--profile b`): channels made for it, `#deploys` and `#coffee`, a short exchange
with a code block, two reactions, a three-reply thread and a link from `#deploys` to a `#coffee` message. `#deploys` now
ends in two Slackbot "A huddle started" notes from earlier test huddles, which can't be deleted, so in LP12b the same
conversation (same messages, authors, reactions, thread and link) was posted again in a fresh channel, `#prod-deploys`,
that has never had a huddle; the captures show it under its own name (no rename).

What klack draws from the workspace itself — people, the workspace, its other channels — is replaced at conversion
time with `~/.config/klack-site/rename.json` (outside the repo, 0600). Its made-up values are exactly as wide as
the real strings; it also covers the two-letter initials drawn before a name and names klack cut short with `…`.
`~/.config/klack-site/forbidden.txt` (0600) lists the real strings: both accounts' names, display names, handles,
emails and user ids, the other members', the team id, the workspace name and domain, and every channel name except
the two demo channels. Both files were written by a script straight from `klack ctl whoami`, `team.info`,
`users.list`, `channels` and the profiles' session files; nothing from them is printed. `python3
tools/site/check_private.py site` checks the result, and each capture was also checked for any 5-character piece
of a listed string (only generic words share one: "channel", "search", "staging", Slack's own address).

## The live captures (hero, keys, huddle)

Re-captured in LP12a so that the person A talks to shows online and has the camera off, and again in LP12b
(windows `lp-c3`, `lp-c3b`) in `#prod-deploys`, so that no Slackbot huddle note shows. klack (debug build) as A
in its own kitty window (`--class lp-c2`, remote control on its own socket, floating, on a hidden special
workspace) and, at the same time, B's own klack in a second one (`lp-c2b`), so that B's presence is active and A's
sidebar draws B's dot green. Each had its own fake HOME: a copy of that account's session file, sounds off,
`[audio] input` and `KLACK_AUDIO_INPUT` naming a microphone that does not exist (klack never falls back to another
one), huddle volume 0, `KLACK_AVATARS=0` (initials instead of profile pictures). A's window was sized to 110 × 32
cells and each screen taken with `kitty @ get-text --extent screen --ansi`. The parsed grid
was checked against kitty's plain `get-text` of the same screen, row by row: equal (the picture's placeholder cells
aside). B's window was never captured.

- **hero** — the main view on `#prod-deploys` (reached with Ctrl+K), the last message, the one with the link,
  selected. The channel starts on screen ("beginning of conversation"), and nothing follows the link message.
- **keys** — then Space: the which-key panel.
- **huddle** — held in `#coffee`, never in the channel the other captures show, so it gets no note. B ran
  `klack ctl --profile b huddle-media coffee 300 --share-test` (a synthetic screen share only: no `--video-test`,
  so B's camera is off, and no microphone); A joined with `␣hh` (muted, as always) and opened the huddle view with
  `␣hv`; afterwards `␣hh` left. In LP12b B's ctl ran for a fixed 150 s and left the huddle by itself (its LEAVE),
nobody was killed, and the sidebar showed no huddle in `#coffee` afterwards. No other key was pressed in the huddle.
  The shared screen is a kitty picture, so the text capture has placeholder cells there: they are blanked and the
  page puts the Swan Lake still over them. Both tiles show a letter instead of a picture: the first letter of the
  real name. `~/.config/klack-site/fixes.json` (0600, next to `rename.json`) puts the made-up names' letters into
  those two cells (`{"huddle": {"row,col": letter}}`); it was written by a script that finds the lone letter in
  each tile (A's is the one labelled "(You)") and prints only the positions. `captures.py` refuses to build the
  huddle without it, refuses a fix whose cell is blank (the layout moved), and refuses a screen with more or fewer
  pictures than the share, so a re-capture can't ship a real letter or a camera by accident.

## The shared screen: Swan Lake

In August 1991 Soviet television showed *Swan Lake* over and over instead of the news; the huddle capture's shared
screen shows it too: one still of the four little swans under faint scanlines and grain (`.capture .tv` in
`site/css/site.css`). Nothing moves. It was downloaded from its Commons file page's original file (800 × 533) into
an empty scratch folder, then (LP12b) cropped to the cygnets with some stage around them (480 × 270 from x 242,
y 112), scaled to 640 × 360, made greyscale with a touch of contrast (`hue=s=0,eq=contrast=1.12`, baked in, so it
looks like old television) and saved as webp with ffmpeg — `site/captures/swan.webp`, 15 KB:

- [File:Swanlake010.jpg](https://commons.wikimedia.org/wiki/File:Swanlake010.jpg) — "Swan Lake ballet", the
  cygnets, State Ballet of Georgia, 2007, by Paata Vardanashvili,
  [CC BY 2.0](https://creativecommons.org/licenses/by/2.0/); cropped, scaled and made greyscale. Credited in the
  page's footer (title, author, links to the file page and the licence, "cropped, greyscale").

## The sign-in screen (signin)

Not a live run: `klack auth login --email …` asks Slack for its sign-in page (`CodeSignIn::start`, "Asking
Slack…") before it draws the code screen, and this capture was to send nothing to Slack. So the screen is klack's
own layout function, `qr::code_screen("demo@example.com", 120, 30)` (the "beside" layout), written to a file by a
throwaway test that was not committed, with the command line above it and the guide's hint line and
`Code: ___-___` prompt below, as `auth.rs` prints them. That file was printed in a second kitty window
(`lp-7bb`, 120 × 30) and taken with `get-text --ansi` like the others, so kitty laid it out.

The QR code: `signin_link("demo@example.com")` is
`https://app.slack.com/get-started?email=demo%40example.com#/createnew` — Slack's public "get started" page with
the made-up address. LP7 checked the printed half blocks against `qr::modules()` of that link, module for module
(45 × 45, error correction M, quiet zone 4); LP7b's capture reads back to the same 728 dark modules. The page now
draws the half blocks themselves (each cell's top half in its foreground, the bottom in its background), so it
scans the way the terminal's does.

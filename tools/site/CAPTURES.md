# How the page's terminal captures were made (LP7, redrawn in LP7b)

`site/captures/{hero,keys,huddle,ssh,signin}.html` are fragments that `python3 tools/site/fill.py site` puts into
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
accounts (A = `--profile default`, B = `--profile b`): two channels made for it, `#deploys` and `#coffee`, a short exchange
with a code block, two reactions, a three-reply thread and a link from `#deploys` to a `#coffee` message.

What klack draws from the workspace itself — people, the workspace, its other channels — is replaced at conversion
time with `~/.config/klack-site/rename.json` (outside the repo, 0600). Its made-up values are exactly as wide as
the real strings; it also covers the two-letter initials drawn before a name and names klack cut short with `…`.
`~/.config/klack-site/forbidden.txt` (0600) lists the real strings: both accounts' names, display names, handles,
emails and user ids, the other members', the team id, the workspace name and domain, and every channel name except
the two demo channels. Both files were written by a script straight from `klack ctl whoami`, `team.info`,
`users.list`, `channels` and the profiles' session files; nothing from them is printed. `python3
tools/site/check_private.py site` checks the result, and each capture was also checked for any 5-character piece
of a listed string (only generic words share one: "channel", "search", "staging", Slack's own address).

## The live captures (hero, keys, ssh, huddle)

klack (debug build) as A in its own kitty window (`--class lp-7b`, remote control on its own socket, floating, on a
hidden workspace), with a fake HOME: copies of the two session files, sounds off, `[audio] input` and
`KLACK_AUDIO_INPUT` naming a microphone that does not exist (klack never falls back to another one), huddle volume
0, `KLACK_AVATARS=0` (initials instead of profile pictures). The window was sized to 110 × 32 cells (80 × 26 for
the SSH one) and each screen taken with `kitty @ get-text --extent screen --ansi`. The parsed grid was checked
against kitty's plain `get-text` of the same screen, row by row: equal.

- **hero** — the main view on `#deploys`, ↑ twice: the last message but one selected.
- **keys** — then Space: the which-key panel.
- **ssh** — the same main view at 80 × 26, as in an SSH session (the window title says so; klack itself ran
  locally).
- **huddle** — B ran `klack ctl --profile b huddle-media deploys 300 --video-test --share-test` (a synthetic
  camera and a synthetic screen share, no microphone); A joined with `␣hh` (muted, as always) and opened the
  huddle view with `␣hv`; afterwards `␣hh` left. No other key was pressed in the huddle. The video tiles are
  kitty pictures, so the text capture has placeholder cells there: they are blanked; the camera tile gets
  `site/captures/tile.webp` (one frame of `ffmpeg -f lavfi -i testsrc2=size=640x360`, the kind of test pattern
  B sent) and the shared screen plays the Swan Lake loop below. A's own tile shows a letter instead of a picture:
  the made-up name's. `~/.config/klack-site/fixes.json` (0600, next to `rename.json`) puts it into that one cell
  (`{"huddle": {"row,col": letter}}`); `captures.py` refuses to build the huddle without it, and refuses a fix
  whose cell is blank (the layout moved), so a re-capture can't ship the real letter by accident.

## The shared screen: Swan Lake

In August 1991 Soviet television showed *Swan Lake* over and over instead of the news; the huddle capture's shared
screen does the same: four pictures, 4 s each, hard cuts (`step-end`, no fade), faint scanlines and a grain
that jumps at 6 fps (`.capture .tv` in `site/css/site.css`). Off screen it waits (`site/js/tv.js` pauses it from an
IntersectionObserver; the enlarged copy in the zoom always runs); with `prefers-reduced-motion` only the first
picture shows and nothing moves. Each was downloaded once from its Commons file page's "Original file" link into an empty scratch folder,
its licence read on that page, then scaled with ffmpeg to 640 × 360 webp (portraits pillarboxed on black,
landscapes cropped to fit) — `site/captures/swan-<n>.webp`, 10–50 KB each:

1. `swan-1.webp` — [File:The Soviet Union 1970 CPA 3938 stamp (Art. Scenes from Ballet 'Swan Lake' (Pyotr Ilyich Tchaikovsky)).jpg](https://commons.wikimedia.org/wiki/File:The_Soviet_Union_1970_CPA_3938_stamp_%28Art._Scenes_from_Ballet_%27Swan_Lake%27_%28Pyotr_Ilyich_Tchaikovsky%29%29.jpg)
   — a Soviet postage stamp, public domain (not an object of copyright, art. 1259 of the Civil Code of the
   Russian Federation, as the page states).
2. `swan-2.webp` — [File:Swanlakelegnani.jpg](https://commons.wikimedia.org/wiki/File:Swanlakelegnani.jpg) —
   Pierina Legnani as Odette, St. Petersburg 1895, unknown author; public domain (the page: published outside the
   US before 1909; public domain in the US).
3. `swan-3.webp` — [File:SwanLake 1895 set for second act.jpg](https://commons.wikimedia.org/wiki/File:SwanLake_1895_set_for_second_act.jpg)
   — the 1895 Mariinsky set for the second act by Mikhail Bocharov and Henrich Levogt; public domain (the page: author's life plus 70 years or fewer, published
   before 1931; Public Domain Mark 1.0).
4. `swan-4.webp` — [File:Swanlake010.jpg](https://commons.wikimedia.org/wiki/File:Swanlake010.jpg) — "Swan
   Lake ballet", the cygnets, State Ballet of Georgia, 2007, by Paata Vardanashvili,
   [CC BY 2.0](https://creativecommons.org/licenses/by/2.0/); cropped to 16 : 9 and scaled. Credited in the
   page's footer (title, author, links to the file page and the licence, "cropped"), next to the other three
   (the 1970 stamp, Legnani as Odette, the act II set by Bocharov and Levogt): "public domain, via Wikimedia
   Commons".

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

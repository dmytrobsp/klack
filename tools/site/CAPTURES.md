# How the page's terminal captures were made (LP7)

`site/captures/{hero,keys,huddle,ssh,signin}.html` are fragments that `python3 tools/site/fill.py site` puts into
the page's `hero-capture` and `capture-<name>` regions (a region without its file keeps what it has). Each is a
`<div class="term capture">` with the screen converted by `tools/site/ansi2html.py` and styled by the
"terminal captures" block of `site/css/site.css`: the grid is scaled so `--cols` cells fill the box, every run of
non-ASCII characters (box drawing, emoji) sits in an inline block exactly as many cells wide as the terminal drew
it, and pictures are placed over their cells by row and column.

## Names

Everything shown is made up. The conversation was posted for the page in the test workspace by its two test
accounts (A = `--profile default`, B = `--profile b`): two channels made for it, `#deploys` and `#coffee`, a short exchange
with a code block, two reactions, a three-reply thread and a link from `#deploys` to a `#coffee` message.

What klack draws from the workspace itself — people, the workspace, its other channels — is replaced at conversion
time by `convert(ansi, rename)` with `~/.config/klack-site/rename.json` (outside the repo, 0600). Its made-up
values are exactly as wide as the real strings, so the layout is unchanged; it also covers the two-letter
initials drawn before a name and names klack cut short with `…`. `~/.config/klack-site/forbidden.txt` (0600) lists
the real strings: both accounts' names, display names, handles, emails and user ids, the other members', the
team id, the workspace name and domain, and every channel name except the two demo channels. Both files were
written by a script straight from `klack ctl whoami`, `team.info`, `users.list`, `channels` and the profiles'
session files; nothing from them is printed. `python3 tools/site/check_private.py site` checks the result, and
each capture was also checked for any 5-character prefix of a listed string.

## The live captures (hero, keys, ssh, huddle)

klack (debug build) as A in its own kitty window (`--class lp-7`, remote control on its own socket), with a fake
HOME: copies of the two session files, sounds off, `[audio] input` and `KLACK_AUDIO_INPUT` naming a microphone
that does not exist (klack never falls back to another one), huddle volume 0, `KLACK_AVATARS=0` (initials
instead of profile pictures). The window was set to 110 × 32 cells (80 × 26 for the SSH one) and each screen
taken with `kitty @ get-text --extent screen --ansi`.

- **hero** — the main view on `#deploys`.
- **keys** — Space pressed on a message: the which-key panel.
- **ssh** — the same main view at 80 × 26, as in an SSH session (the window title says so; klack itself ran
  locally).
- **huddle** — B ran `klack ctl --profile b huddle-media deploys 300 --video-test --share-test` (a synthetic
  camera and a synthetic screen share, no microphone); A joined with `␣hh` (muted, as always) and opened the
  huddle view with `␣hv`. No other key was pressed in the huddle. The video tiles are kitty images, so the text
  capture has placeholder cells there: they are blanked and `site/captures/tile.webp` — one frame of
  `ffmpeg -f lavfi -i testsrc2=size=640x360` (the same kind of test pattern B sent) — is placed over each tile's
  cells. A's own tile shows a letter instead of a picture: the made-up name's.

## The sign-in screen (signin)

Not a live run: `klack auth login --email …` asks Slack for its sign-in page (`CodeSignIn::start`, "Asking
Slack…") before it draws the code screen, and this capture was to send nothing to Slack. So the screen is klack's
own layout function, `qr::code_screen("demo@example.com", 120, 30)` (the "beside" layout), printed by a
throwaway test that was not committed, with the command line above it and the guide's hint line and
`Code: ___-___` prompt below, as `auth.rs` prints them.

The QR code: no QR decoder is installed here (no zbarimg), so it was checked from the code path instead. The
code screen draws `qr::half_blocks(qr::modules(signin_link(email)))`, and `signin_link("demo@example.com")` is
`https://app.slack.com/get-started?email=demo%40example.com#/createnew` — Slack's public "get started" page with
the made-up address. The half blocks read back from the printed lines (top module from the foreground, bottom
from the background) were compared with `qr::modules()` of that link, module for module (45 × 45, error
correction M, quiet zone 4): equal. The page draws those same modules as an inline SVG over the code's cells,
so it scans the way the terminal's does.

#!/usr/bin/env python3
"""Builds the page's terminal captures, site/captures/<name>.html, from raw kitty captures (LP7b).

    python3 tools/site/captures.py RAW_DIR site/captures

RAW_DIR holds <name>.ansi (`kitty @ get-text --extent screen --ansi`). Outside the repo (0600, never printed or
committed): ~/.config/klack-site/rename.json, the made-up names, and ~/.config/klack-site/fixes.json, single cells
to overwrite after renaming ({"huddle": {"row,col": "letter"}} — a letter drawn from a real name). See
CAPTURES.md for how the raw captures were made.
"""
import html
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from ansi2html import convert, render_svg  # noqa: E402

HUDDLE_LABEL = ("klack's huddle view: the other person's shared screen on the stage (on this page it plays Swan "
                "Lake, like Soviet television in August 1991) and their camera in a tile, your own tile muted, the "
                "call bar at the bottom")

CAPTURES = {
    "hero": dict(cols=110, rows=32, title="klack", label=(
        "klack's main view: the sidebar of channels and direct messages, a channel with a short conversation, "
        "a code block, reactions, a thread and a link to another channel's message")),
    "keys": dict(cols=110, rows=32, title="klack", label=(
        "klack after pressing Space on a message: the which-key panel lists what each next key does")),
    "ssh": dict(cols=80, rows=26, title="ssh box — klack", label=(
        "The same main view of klack in an 80-column terminal, as in an SSH session")),
    # A's own tile shows the first letter of the real name: the capture is not built without its fix
    "huddle": dict(cols=110, rows=32, title="klack — huddle", label=HUDDLE_LABEL, pictures=("tv", "tile"),
                   needs_fixes=True),
    "signin": dict(cols=120, rows=29, title="klack auth login", label=(
        "klack's sign-in guide, the code way: a QR code for the phone, the link, and the ___-___ prompt for "
        "the emailed code, for demo@example.com")),
}

# the shared screen's loop: Commons pictures (sources and licences in CAPTURES.md and the page's footer)
SWANS = ["swan-1.webp", "swan-2.webp", "swan-3.webp", "swan-4.webp"]


def _box(p):
    return f'--r:{p["r"]};--c:{p["c"]};--h:{p["h"]};--w:{p["w"]}'


def picture_html(kind, box):
    """What the page puts over a picture's cells: the camera's test pattern, or the Swan Lake broadcast."""
    if kind == "tile":
        return f'<img class="tile" src="captures/tile.webp" alt="" style="{_box(box)}">'
    imgs = "".join(f'<img src="captures/{n}" alt="">' for n in SWANS)
    return f'<div class="tile tv" style="{_box(box)}" aria-hidden="true">{imgs}</div>'


def fragment(svg, cols, rows, title, label, overlays=""):
    return (f'<div class="term capture" style="--cols:{cols};--rows:{rows}" role="img" '
            f'aria-label="{html.escape(label)}"><div class="dots">● ● ●  {html.escape(title, quote=False)}</div>'
            f'<div class="screen"><div class="grid">{svg}{overlays}</div></div></div>\n')


def build(name, ansi, rename=None, fixes=None):
    """One capture's fragment. `fixes`: {(row, col): text} put into single cells after renaming (a letter
    drawn from a real name, say)."""
    c = CAPTURES[name]
    if c.get("needs_fixes") and not fixes:
        raise ValueError(f"{name}: needs its cell fixes (~/.config/klack-site/fixes.json) and has none")
    _, screen = convert(ansi, rename, c["cols"], c["rows"])
    for (r, col), ch in (fixes or {}).items():
        if not (0 <= r < screen.rows and 0 <= col < screen.cols) or not screen.cells[r][col][0].strip():
            raise ValueError(f"{name}: a fix at {r},{col} has no character to replace (the layout moved?)")
        screen.cells[r][col][0] = ch
    kinds = c.get("pictures", ())
    if len(screen.pictures) != len(kinds):
        raise ValueError(f"{name}: {len(screen.pictures)} pictures on the screen, {len(kinds)} expected")
    overlays = "".join(picture_html(k, p) for k, p in zip(kinds, screen.pictures))
    return fragment(render_svg(screen), c["cols"], c["rows"], c["title"], c["label"], overlays)


def main(argv):
    raw, out = Path(argv[0]), Path(argv[1])
    rename = json.loads((Path.home() / ".config/klack-site/rename.json").read_text())
    fx = Path.home() / ".config/klack-site/fixes.json"
    fixes = json.loads(fx.read_text()) if fx.is_file() else {}
    for name in CAPTURES:
        src = raw / f"{name}.ansi"
        if not src.is_file():
            continue
        f = {tuple(map(int, k.split(","))): v for k, v in fixes.get(name, {}).items()}
        (out / f"{name}.html").write_text(build(name, src.read_text(encoding="utf-8"), rename, f), encoding="utf-8")
        print(f"{name}: written")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

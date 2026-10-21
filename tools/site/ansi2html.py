"""Draw a terminal capture (kitty `get-text --extent screen --ansi`) the way kitty draws it: an SVG on an exact
cell grid (LP7b).

Every cell of every row is kept. Backgrounds are whole cells, box drawing (U+2500–257F) and block elements
(U+2580–259F) are geometry spanning the full cell — kitty draws those itself, so lines join across cells and rows —
and the rest is real, selectable text, each run pinned to its cells. Wide characters take two cells. OSC, DCS, APC,
PM and SOS payloads are dropped; kitty's image placeholder cells are blanked and reported as pictures.
"""
import html
import re
import unicodedata

# One cell, in SVG units: kitty's cell where the captures were made is 12 × 27 device pixels (JetBrains Mono 12 pt
# at 1.25×), so 8 × 18. Space Mono's advance is 0.6125 em: FONT makes one character exactly one cell wide.
CW, CH = 8, 18
FONT = round(CW / 0.6125, 3)
BASE = 13.5            # the baseline, from the top of a cell (kitty's sits at three quarters of the cell)
LIGHT, HEAVY = 1.33, 2.67   # kitty's box_drawing_scale (1 pt and 2 pt at 120 dpi: 2 and 4 of 12 device px)
DIM = 0.4              # kitty's dim_opacity: dim text is 40 % of the way from the background to its colour

# The colours of the kitty the captures were made in (`kitty @ get-colors`); the 216-colour cube and the greys
# are the standard ones except where that kitty overrides them (16 and 17).
KITTY = {
    "fg": "#cdd6f4", "bg": "#1e1e2e",
    "16": ["#45475a", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#bac2de",
           "#585b70", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#a6adc8"],
    "more": {16: "#fab387", 17: "#f5e0dc"},
}


def palette(theme=KITTY):
    p = list(theme["16"])
    steps = [0, 95, 135, 175, 215, 255]
    for r in steps:
        for g in steps:
            for b in steps:
                p.append("#%02x%02x%02x" % (r, g, b))
    for i in range(24):
        v = 8 + 10 * i
        p.append("#%02x%02x%02x" % (v, v, v))
    for i, c in theme.get("more", {}).items():
        p[i] = c
    return p


_TOKEN = re.compile(
    r"\x1b\[([0-9;:?<=>]*)([ -/]*[@-~])"   # CSI
    r"|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)?"  # OSC (BEL or ST terminated)
    r"|\x1b[P_^X][^\x1b\x07]*(?:\x07|\x1b\\)?"  # DCS, APC, PM, SOS payloads
    r"|\x1b[ -/]*[0-~]?"                     # other escapes (a bare ST included)
)

# a style: (fg, bg, bold, dim, italic, underline, strike, reverse); colours are "#rrggbb" or None (the default)
FRESH = (None, None, False, False, False, False, False, False)
PLACEHOLDER = "\U0010EEEE"  # kitty's Unicode placeholder for a picture cell


def _sgr(params, st, pal):
    fg, bg, bold, dim, ital, ul, strike, rev = st
    flat = [g.split(":") for g in params.split(";")] if params else [[""]]

    def num(s):
        return int(s) if s.isdigit() else 0

    i = 0
    while i < len(flat):
        g = flat[i]
        n = num(g[0])
        if n in (38, 48, 58):
            col = None
            if len(g) > 1:  # colon form
                sub = [num(x) if x else None for x in g[1:]]
                if sub[0] == 5 and len(sub) >= 2:
                    col = pal[min(sub[1] or 0, 255)]
                elif sub[0] == 2 and len(sub) >= 4:
                    rgb = sub[-3:]  # tolerates the empty colourspace slot
                    col = "#%02x%02x%02x" % tuple(min(x or 0, 255) for x in rgb)
            else:  # semicolon form
                mode = num(flat[i + 1][0]) if i + 1 < len(flat) else 0
                if mode == 5 and i + 2 < len(flat):
                    col = pal[min(num(flat[i + 2][0]), 255)]
                    i += 2
                elif mode == 2 and i + 4 < len(flat):
                    col = "#%02x%02x%02x" % tuple(min(num(flat[i + k][0]), 255) for k in (2, 3, 4))
                    i += 4
                else:
                    i = len(flat)
            if col and n == 38:
                fg = col
            elif col and n == 48:
                bg = col
        elif n == 0: fg, bg, bold, dim, ital, ul, strike, rev = FRESH
        elif n == 1: bold = True
        elif n == 2: dim = True
        elif n == 3: ital = True
        elif n == 4: ul = not (len(g) > 1 and g[1] == "0")
        elif n == 7: rev = True
        elif n == 9: strike = True
        elif n == 21: ul = True
        elif n == 22: bold = dim = False
        elif n == 23: ital = False
        elif n == 24: ul = False
        elif n == 27: rev = False
        elif n == 29: strike = False
        elif 30 <= n <= 37: fg = pal[n - 30]
        elif n == 39: fg = None
        elif 40 <= n <= 47: bg = pal[n - 40]
        elif n == 49: bg = None
        elif 90 <= n <= 97: fg = pal[n - 90 + 8]
        elif 100 <= n <= 107: bg = pal[n - 100 + 8]
        i += 1
    return (fg, bg, bold, dim, ital, ul, strike, rev)


def _zero_width(ch):
    return (unicodedata.category(ch) in ("Mn", "Me") or ch in "​‌‍︎️"
            or 0xE0100 <= ord(ch) <= 0xE01EF)


def char_width(ch):
    """Cells a character takes: 0 for marks that join the cell before, 2 for wide ones, else 1."""
    if _zero_width(ch):
        return 0
    return 2 if unicodedata.east_asian_width(ch) in ("W", "F") else 1


class Screen:
    """rows × cols cells; each cell is [text, style, width]: width 1, 2 (a wide character) or 0 (the second
    half of the wide character before it, text ""). `pictures` are kitty pictures by their placeholder cells."""

    def __init__(self, cells, pictures):
        self.cells, self.pictures = cells, pictures
        self.rows, self.cols = len(cells), len(cells[0]) if cells else 0

    def text(self, r):
        return "".join(c[0] if c[2] else "" for c in self.cells[r])


def parse(ansi, cols, rows=None, theme=KITTY):
    """The screen of a capture: lines longer than `cols` cells continue on the next row; every row is `cols`
    cells (what a line leaves blank stays blank, default colours). EL (CSI K) clears with the current background
    and leaves the cursor where it was."""
    pal = palette(theme)
    ansi = ansi.replace("\r\n", "\n").replace("\r", "\n")
    lines, cur, x, st = [], [], 0, FRESH

    def blank(style):
        return [" ", (None, style[1], False, False, False, False, False, style[7]), 1]

    def newline():
        nonlocal cur, x
        lines.append(cur)
        cur, x = [], 0

    def write(cell):
        nonlocal x
        if x < len(cur):
            cur[x] = cell
        else:
            cur.append(cell)
        x += 1

    def put(ch):
        if ch == "\n":
            return newline()
        w = char_width(ch)
        if w == 0:
            head = next((c for c in reversed(cur[:x]) if c[2]), None)
            if head is not None:
                head[0] += ch
                if ch == "\ufe0f" and head[2] == 1 and ord(head[0][0]) >= 0x2000 and head[0][0] != PLACEHOLDER:
                    head[2] = 2  # an emoji made wide by VS16, as kitty draws it
                    write(["", head[1], 0])
            return
        if x + w > cols:
            newline()
        write([ch, st, w])
        if w == 2:
            write(["", st, 0])

    pos = 0
    for m in _TOKEN.finditer(ansi):
        for ch in ansi[pos:m.start()]:
            put(ch)
        pos = m.end()
        params, final = m.group(1), m.group(2)
        if final == "m" and params is not None and not params.startswith(("?", "<", "=", ">")):
            st = _sgr(params, st, pal)
        elif final == "K" and params in ("", "0", "1", "2"):
            lo, hi = {"": (x, cols), "0": (x, cols), "1": (0, x + 1), "2": (0, cols)}[params]
            cur.extend(blank(FRESH) for _ in range(len(cur), hi))
            for i in range(lo, min(hi, cols)):
                cur[i] = blank(st)
    for ch in ansi[pos:]:
        put(ch)
    lines.append(cur)
    while len(lines) > max(rows or 0, 1) and not any(c[0].strip() or c[1] != FRESH for c in lines[-1]):
        lines.pop()
    if rows:
        lines = (lines + [[] for _ in range(rows)])[:rows]
    cells = [l[:cols] + [[" ", FRESH, 1] for _ in range(cols - len(l[:cols]))] for l in lines]
    return Screen(cells, _pictures(cells))


def _pictures(cells):
    """Picture placeholder cells, blanked: [{"r", "c", "h", "w"}] per picture (by its colour), in reading order."""
    boxes = {}
    for r, row in enumerate(cells):
        for c, cell in enumerate(row):
            if cell[0].startswith(PLACEHOLDER):
                key = cell[1][0]
                r0, c0, r1, c1 = boxes.get(key, (r, c, r, c))
                boxes[key] = (min(r0, r), min(c0, c), max(r1, r), max(c1, c))
                cell[0], cell[1] = " ", (None,) + cell[1][1:2] + (False,) * 6
    out = [{"r": r0, "c": c0, "h": r1 - r0 + 1, "w": c1 - c0 + 1} for r0, c0, r1, c1 in boxes.values()]
    return sorted(out, key=lambda p: (p["r"], p["c"]))


def rename_cells(screen, rename):
    """Replaces each key of `rename` found in a row with its value cell by cell: every cell keeps its own style.
    A value shorter than the cells it replaces is padded with spaces, a longer one cut, so the layout stays."""
    keys = sorted((k for k in (rename or {}) if k), key=len, reverse=True)
    if not keys:
        return screen
    pat = re.compile("|".join(re.escape(k) for k in keys))
    for row in screen.cells:
        at = [i for i, c in enumerate(row) if c[2]]   # text index -> cell (wide cells count once)
        text = "".join(row[i][0][:1] or " " for i in at)
        for m in pat.finditer(text):
            first, last = at[m.start()], at[m.end() - 1]
            span = list(range(first, last + row[last][2]))
            value = list(rename[m.group(0)][:len(span)])
            value += [" "] * (len(span) - len(value))
            for i, ch in zip(span, value):
                row[i][0], row[i][2] = ch, 1
    return screen


# ---- drawing

def _mix(a, b, t):
    """a → b by t (0..1), "#rrggbb"."""
    pa, pb = [int(a[i:i + 2], 16) for i in (1, 3, 5)], [int(b[i:i + 2], 16) for i in (1, 3, 5)]
    return "#%02x%02x%02x" % tuple(round(x + (y - x) * t) for x, y in zip(pa, pb))


def colours(style, theme=KITTY):
    """(foreground, background) as kitty paints a cell of `style`: reverse swaps, dim fades the foreground."""
    fg, bg = style[0] or theme["fg"], style[1] or theme["bg"]
    if style[7]:
        fg, bg = bg, fg
    if style[3]:
        fg = _mix(bg, fg, DIM)
    return fg, bg


_W = {"LIGHT": 1, "HEAVY": 2, "DOUBLE": 3, "SINGLE": 1}
_DIRS = {"UP": "u", "DOWN": "d", "LEFT": "l", "RIGHT": "r", "VERTICAL": "ud", "HORIZONTAL": "lr"}


def box_arms(ch):
    """{"u"|"d"|"l"|"r": 1 light, 2 heavy, 3 double} for a box-drawing character's lines, from its Unicode
    name; None for the arcs, diagonals and dashes (drawn on their own)."""
    name = unicodedata.name(ch, "")
    if not name.startswith("BOX DRAWINGS ") or any(w in name for w in ("ARC", "DIAGONAL", "DASH")):
        return None
    groups = [g.split() for g in name[len("BOX DRAWINGS "):].split(" AND ")]
    own = [next((_W[w] for w in g if w in _W), None) for g in groups]
    arms, last = {}, next((w for w in own if w), 1)
    for g, w in zip(groups, own):
        last = w or last
        for word in g:
            for d in _DIRS.get(word, ""):
                arms[d] = last
    return arms


class _Ink:
    """Rectangles and strokes by colour; touching rectangles of one row or one column are merged, and each colour
    is one path, so nothing shows between cells."""

    def __init__(self):
        self.rects, self.strokes = {}, {}

    def rect(self, colour, x0, y0, x1, y1):
        if x1 > x0 and y1 > y0:
            self.rects.setdefault(colour, []).append((round(x0, 2), round(y0, 2), round(x1, 2), round(y1, 2)))

    def stroke(self, colour, width, d):
        self.strokes.setdefault((colour, width), []).append(d)

    @staticmethod
    def merged(rects):
        rects = sorted(set(rects), key=lambda r: (r[1], r[3], r[0]))
        out = []
        for r in rects:  # along rows
            if out and out[-1][1] == r[1] and out[-1][3] == r[3] and r[0] <= out[-1][2] + 1e-6:
                out[-1] = (out[-1][0], r[1], max(out[-1][2], r[2]), r[3])
            else:
                out.append(r)
        out.sort(key=lambda r: (r[0], r[2], r[1]))
        res = []
        for r in out:  # then down columns
            if res and res[-1][0] == r[0] and res[-1][2] == r[2] and r[1] <= res[-1][3] + 1e-6:
                res[-1] = (r[0], res[-1][1], r[2], max(res[-1][3], r[3]))
            else:
                res.append(r)
        return res

    def svg(self, cls):
        out = []
        f = lambda v: ("%.2f" % v).rstrip("0").rstrip(".")
        for colour, rects in self.rects.items():
            d = "".join(f"M{f(x0)} {f(y0)}H{f(x1)}V{f(y1)}H{f(x0)}Z" for x0, y0, x1, y1 in self.merged(rects))
            out.append(f'<path class="{cls}" fill="{colour}" d="{d}"/>')
        for (colour, width), ds in self.strokes.items():
            out.append(f'<path fill="none" stroke="{colour}" stroke-width="{width}" d="{"".join(ds)}"/>')
        return "\n".join(out)


def _box(ink, ch, fg, x, y):
    """Box drawing in the cell at (x, y). Each line runs from the cell's edge to the centre and on as far as the
    lines across it, so corners are square and neighbouring cells join."""
    cx, cy = x + CW / 2, y + CH / 2
    arms = box_arms(ch)
    if arms is not None:
        g = LIGHT  # a double line's two lines sit this far either side of the centre line
        thick = lambda w: HEAVY if w == 2 else LIGHT
        for d, w in arms.items():
            across = [arms.get(p, 0) for p in ("ud" if d in "lr" else "lr")]
            reach = max((thick(a) for a in across if a), default=0) / 2
            lines = []
            if w == 3:
                for o in (-g, g):
                    if 3 in across:  # inner lines stop at the near line across, outer ones reach the far one
                        s = g - LIGHT / 2 if across[o > 0] == 3 else -(g + LIGHT / 2)
                    else:
                        s = -reach
                    lines.append((o, s, LIGHT))
            else:
                lines.append((0, -(g + LIGHT / 2) if 3 in across else -reach, thick(w)))
            for o, s, th in lines:
                if d == "r":
                    ink.rect(fg, cx + s, cy + o - th / 2, x + CW, cy + o + th / 2)
                elif d == "l":
                    ink.rect(fg, x, cy + o - th / 2, cx - s, cy + o + th / 2)
                elif d == "d":
                    ink.rect(fg, cx + o - th / 2, cy + s, cx + o + th / 2, y + CH)
                else:
                    ink.rect(fg, cx + o - th / 2, y, cx + o + th / 2, cy - s)
        return
    name = unicodedata.name(ch, "")
    if "ARC" in name:
        r = CW / 2
        a = {"╭": (x + CW, cy, cx, y + CH, 0), "╮": (x, cy, cx, y + CH, 1), "╯": (x, cy, cx, y, 0),
             "╰": (x + CW, cy, cx, y, 1)}[ch]
        hx, hy, vx, vy, sweep = a
        sx = cx + (r if hx > cx else -r)
        sy = cy + (r if vy > cy else -r)
        ink.stroke(fg, LIGHT, f"M{hx} {cy}H{sx}A{r} {r} 0 0 {sweep} {cx} {sy}V{vy}")
    elif "DIAGONAL" in name:
        if ch in "╱╳":
            ink.stroke(fg, LIGHT, f"M{x} {y + CH}L{x + CW} {y}")
        if ch in "╲╳":
            ink.stroke(fg, LIGHT, f"M{x} {y}L{x + CW} {y + CH}")
    elif "DASH" in name:
        n = 2 if "DOUBLE DASH" in name else 3 if "TRIPLE" in name else 4
        th = HEAVY if "HEAVY" in name else LIGHT
        if "HORIZONTAL" in name:
            step = CW / n
            for i in range(n):
                ink.rect(fg, x + i * step + step / 4, cy - th / 2, x + (i + 1) * step - step / 4, cy + th / 2)
        else:
            step = CH / n
            for i in range(n):
                ink.rect(fg, cx - th / 2, y + i * step + step / 4, cx + th / 2, y + (i + 1) * step - step / 4)


_QUAD = {"▖": "3", "▗": "4", "▘": "1", "▙": "134", "▚": "14", "▛": "123", "▜": "124", "▝": "2", "▞": "23",
         "▟": "234"}


def block_parts(ch):
    """A block element as parts that tile its cell: [((x0, y0, x1, y1) in cell fractions, "fg" | "bg" | shade)].
    The background is a part too, so a cell never has one colour drawn over another (no seams between rows)."""
    o = ord(ch)
    if ch == "▀":
        return [((0, 0, 1, .5), "fg"), ((0, .5, 1, 1), "bg")]
    if 0x2581 <= o <= 0x2588:  # lower 1/8 .. full
        h = (o - 0x2580) / 8
        return [((0, 1 - h, 1, 1), "fg"), ((0, 0, 1, 1 - h), "bg")]
    if 0x2589 <= o <= 0x258F:  # left 7/8 .. 1/8
        w = (0x2590 - o) / 8
        return [((0, 0, w, 1), "fg"), ((w, 0, 1, 1), "bg")]
    if ch == "▐":
        return [((.5, 0, 1, 1), "fg"), ((0, 0, .5, 1), "bg")]
    if ch in "░▒▓":
        return [((0, 0, 1, 1), {"░": .25, "▒": .5, "▓": .75}[ch])]
    if ch == "▔":
        return [((0, 0, 1, 1 / 8), "fg"), ((0, 1 / 8, 1, 1), "bg")]
    if ch == "▕":
        return [((7 / 8, 0, 1, 1), "fg"), ((0, 0, 7 / 8, 1), "bg")]
    if ch in _QUAD:
        return [(((0 if q in "13" else .5), (0 if q in "12" else .5), (.5 if q in "13" else 1), (.5 if q in "12" else 1)),
                 "fg" if q in _QUAD[ch] else "bg") for q in "1234"]
    return [((0, 0, 1, 1), "bg")]


def _block(paint, ch, fg, bg, x, y):
    """Block elements in the cell at (x, y), background included."""
    for (x0, y0, x1, y1), what in block_parts(ch):
        col = fg if what == "fg" else bg if what == "bg" else _mix(bg, fg, what)
        paint.rect(col, x + x0 * CW, y + y0 * CH, x + x1 * CW, y + y1 * CH)


def drawn(ch):
    """Characters drawn as geometry instead of text."""
    return bool(ch) and 0x2500 <= ord(ch[0]) <= 0x259F


def render_svg(screen, theme=KITTY, label=None):
    """The screen as an <svg>: the terminal background, the cells' colours (backgrounds and block elements, one
    path a colour), lines (box drawing, underlines), then one <text> per row."""
    bgs, ink, rows_svg = _Ink(), _Ink(), []
    f = lambda v: ("%.2f" % v).rstrip("0").rstrip(".")
    for r, row in enumerate(screen.cells):
        y = r * CH
        spans, run = [], None
        for c, (txt, style, w) in enumerate(row):
            fg, bg = colours(style, theme)
            x = c * CW
            block = w == 1 and txt[:1] and 0x2580 <= ord(txt[0]) <= 0x259F
            if block:
                _block(bgs, txt[0], fg, bg, x, y)
            elif bg != theme["bg"]:
                bgs.rect(bg, x, y, x + CW, y + CH)
            if w == 0:
                continue
            base = txt[:1]
            if style[5]:
                ink.rect(fg, x, y + BASE + 1.5, x + CW * w, y + BASE + 1.5 + LIGHT)
            if style[6]:
                ink.rect(fg, x, y + CH / 2 - LIGHT / 2 + 0.5, x + CW * w, y + CH / 2 + LIGHT / 2 + 0.5)
            if drawn(base):
                if ord(base) < 0x2580:
                    _box(ink, base, fg, x, y)
                run = None
                continue
            if base in (" ", " ", ""):
                if run is not None and run["text"].endswith(" "):
                    run = None  # two spaces: the next text is placed anew
                elif run is not None:
                    run["text"] += " "; run["n"] += 1
                continue
            key = (fg, style[2], style[4])
            if 32 < ord(base) < 127 and run is not None and run["key"] == key and run["ascii"]:
                run["text"] += txt; run["n"] += 1
                continue
            run = {"x": x, "n": w, "text": txt, "key": key, "ascii": 32 < ord(base) < 127, "w": w}
            spans.append(run)
            if not run["ascii"]:
                run = None
        out = []
        for s in spans:
            text = s["text"].rstrip(" ")
            n = s["n"] - (len(s["text"]) - len(text))
            fg, bold, ital = s["key"]
            attrs = [f'fill="{fg}"'] if fg != theme["fg"] else []
            if bold: attrs.append('font-weight="700"')
            if ital: attrs.append('font-style="italic"')
            if s["ascii"]:
                attrs.insert(0, f'x="{f(s["x"])}"')
                if n > 1:
                    attrs.append(f'textLength="{f(n * CW)}"')
            else:
                attrs.insert(0, f'x="{f(s["x"] + CW * s["w"] / 2)}" text-anchor="middle"')
            out.append(f'<tspan {" ".join(attrs)}>{html.escape(text, quote=False)}</tspan>')
        if out:
            rows_svg.append(f'<text y="{f(y + BASE)}">{"".join(out)}</text>')
    W, H = screen.cols * CW, screen.rows * CH
    aria = f' role="img" aria-label="{html.escape(label)}"' if label else ""
    return (f'<svg class="cells" viewBox="0 0 {W} {H}" width="{W}" height="{H}"{aria} '
            f'font-family="\'Space Mono\', monospace" font-size="{FONT}" fill="{theme["fg"]}">\n'
            f'<rect width="{W}" height="{H}" fill="{theme["bg"]}"/>\n'
            + bgs.svg("bg") + ("\n" if bgs.rects else "")
            + ink.svg("ink") + ("\n" if ink.rects or ink.strokes else "")
            + "\n".join(rows_svg) + "\n</svg>")


def convert(ansi, rename=None, cols=None, rows=None, theme=KITTY):
    """Capture → (svg, screen). Without `cols`, the widest line decides."""
    if cols is None:
        probe = parse(ansi, 10 ** 6, theme=theme)
        cols = max(1, max((sum(1 for c in row if c[0] != " " or c[1] != FRESH) and
                           max(i + 1 for i, c in enumerate(row) if c[0] != " " or c[1] != FRESH)
                           for row in probe.cells), default=1))
    screen = rename_cells(parse(ansi, cols, rows, theme), rename)
    return render_svg(screen, theme), screen

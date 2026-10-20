"""Turn a terminal capture (kitty get-text --ansi) into <pre class="term"> HTML."""
import html
import re

DEFAULT_FG = "#d8d8d8"  # the page's .term colours
DEFAULT_BG = "#0b0b0d"

_BASE = ["#000000", "#800000", "#008000", "#808000", "#000080", "#800080", "#008080", "#c0c0c0",
         "#808080", "#ff0000", "#00ff00", "#ffff00", "#0000ff", "#ff00ff", "#00ffff", "#ffffff"]


def _palette():
    p = list(_BASE)
    steps = [0, 95, 135, 175, 215, 255]
    for r in steps:
        for g in steps:
            for b in steps:
                p.append("#%02x%02x%02x" % (r, g, b))
    for i in range(24):
        v = 8 + 10 * i
        p.append("#%02x%02x%02x" % (v, v, v))
    return p


PALETTE = _palette()

_TOKEN = re.compile(
    r"\x1b\[([0-9;:?<=>]*)([ -/]*[@-~])"   # CSI
    r"|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)?"  # OSC (BEL or ST terminated)
    r"|\x1b[P_^X][^\x1b\x07]*(?:\x07|\x1b\\)?"  # DCS, APC, PM, SOS payloads
    r"|\x1b[ -/]*[0-~]?"                     # other escapes (a bare ST included)
)

_FRESH = (None, None, False, False, False, False, False, False)  # fg bg bold dim italic ul strike rev


def _sgr(params, st):
    fg, bg, bold, dim, ital, ul, strike, rev = st
    # split into groups, keeping colon sub-parameters together
    groups = [g.split(":") for g in params.split(";")] if params else [[""]]
    flat = []
    for g in groups:
        flat.append(g)
    i = 0

    def num(s):
        return int(s) if s.isdigit() else 0

    while i < len(flat):
        g = flat[i]
        n = num(g[0])
        if n in (38, 48):
            if len(g) > 1:  # colon form
                sub = [num(x) if x else None for x in g[1:]]
                col = None
                if sub[0] == 5 and len(sub) >= 2:
                    col = PALETTE[min(sub[1] or 0, 255)]
                elif sub[0] == 2 and len(sub) >= 4:
                    rgb = sub[-3:]  # tolerates the empty colourspace slot
                    col = "#%02x%02x%02x" % tuple(min(x or 0, 255) for x in rgb)
            else:  # semicolon form
                col = None
                mode = num(flat[i + 1][0]) if i + 1 < len(flat) else 0
                if mode == 5 and i + 2 < len(flat):
                    col = PALETTE[min(num(flat[i + 2][0]), 255)]
                    i += 2
                elif mode == 2 and i + 4 < len(flat):
                    col = "#%02x%02x%02x" % tuple(min(num(flat[i + k][0]), 255) for k in (2, 3, 4))
                    i += 4
                else:
                    i = len(flat)
            if col:
                if n == 38:
                    fg = col
                else:
                    bg = col
        elif n == 0:
            fg, bg, bold, dim, ital, ul, strike, rev = _FRESH
        elif n == 1: bold = True
        elif n == 2: dim = True
        elif n == 3: ital = True
        elif n == 4: ul = True
        elif n == 7: rev = True
        elif n == 9: strike = True
        elif n == 22: bold = dim = False
        elif n == 23: ital = False
        elif n == 24: ul = False
        elif n == 27: rev = False
        elif n == 29: strike = False
        elif 30 <= n <= 37: fg = PALETTE[n - 30]
        elif n == 39: fg = None
        elif 40 <= n <= 47: bg = PALETTE[n - 40]
        elif n == 49: bg = None
        elif 90 <= n <= 97: fg = PALETTE[n - 90 + 8]
        elif 100 <= n <= 107: bg = PALETTE[n - 100 + 8]
        i += 1
    return (fg, bg, bold, dim, ital, ul, strike, rev)


def _css(st):
    fg, bg, bold, dim, ital, ul, strike, rev = st
    if rev:
        fg, bg = (bg or DEFAULT_BG), (fg or DEFAULT_FG)
    parts = []
    if fg: parts.append("color:" + fg)
    if bg: parts.append("background:" + bg)
    if bold: parts.append("font-weight:700")
    if ital: parts.append("font-style:italic")
    deco = ("underline " if ul else "") + ("line-through" if strike else "")
    if deco.strip(): parts.append("text-decoration:" + deco.strip())
    if dim: parts.append("opacity:.6")
    return ";".join(parts)


def _cells(ansi):
    """List of lines; each line a list of [char, style]."""
    ansi = ansi.replace("\r\n", "\n").replace("\r", "\n")
    lines, cur, st, pos = [], [], _FRESH, 0
    for m in _TOKEN.finditer(ansi):
        for ch in ansi[pos:m.start()]:
            if ch == "\n":
                lines.append(cur); cur = []
            else:
                cur.append([ch, st])
        pos = m.end()
        if m.group(2) == "m" and m.group(1) is not None:
            st = _sgr(m.group(1), st) if not m.group(1).startswith(("?", "<", "=", ">")) else st
    for ch in ansi[pos:]:
        if ch == "\n":
            lines.append(cur); cur = []
        else:
            cur.append([ch, st])
    lines.append(cur)
    return lines


def _rename(lines, rename):
    if not rename:
        return lines
    keys = sorted((k for k in rename if k), key=len, reverse=True)
    if not keys:
        return lines
    pat = re.compile("|".join(re.escape(k) for k in keys))
    flat, bounds = [], []
    for li, line in enumerate(lines):
        for c in line:
            flat.append(c)
        flat.append(["\n", None])
    text = "".join(c[0] for c in flat)
    out, pos = [], 0
    for m in pat.finditer(text):
        out.extend(flat[pos:m.start()])
        st = flat[m.start()][1]
        out.extend([ch, st] for ch in rename[m.group(0)])
        pos = m.end()
    out.extend(flat[pos:])
    res, cur = [], []
    for ch, st in out:
        if ch == "\n":
            res.append(cur); cur = []
        else:
            cur.append([ch, st])
    res.append(cur)
    return res


def convert(ansi, rename=None):
    lines = _rename(_cells(ansi), rename)
    out_lines = []
    for line in lines:
        while line and line[-1][0] in " \t" and not _css(line[-1][1]):
            line.pop()
        out_lines.append(line)
    while out_lines and not out_lines[-1]:
        out_lines.pop()
    rendered = []
    for line in out_lines:
        buf, i = [], 0
        while i < len(line):
            css = _css(line[i][1])
            j = i
            txt = []
            while j < len(line) and _css(line[j][1]) == css:
                txt.append(line[j][0]); j += 1
            s = html.escape("".join(txt), quote=False)
            buf.append('<span style="%s">%s</span>' % (css, s) if css else s)
            i = j
        rendered.append("".join(buf))
    return '<pre class="term">' + "\n".join(rendered) + "</pre>"

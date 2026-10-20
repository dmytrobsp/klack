"""Scan the site for strings that must never ship. Never prints a forbidden string."""
import os
import re
import sys
from pathlib import Path

SKIP_EXT = {".woff", ".woff2", ".ttf", ".otf", ".png", ".jpg", ".jpeg", ".gif", ".webp", ".avif",
            ".ico", ".mp4", ".webm", ".ogg", ".mp3", ".wav", ".pdf", ".zip", ".gz"}
_SEP = re.compile(r"[\s._-]+")


def _norm(s):
    return _SEP.sub("", s.lower())


def scan(root, forbidden):
    """Return [(path, line, index into forbidden)] for every hit."""
    root = Path(root)
    keys = [(i, f.lower(), _norm(f)) for i, f in enumerate(forbidden)]
    keys = [k for k in keys if k[2]]
    hits = []
    files = [root] if root.is_file() else sorted(p for p in root.rglob("*") if p.is_file())
    for p in files:
        if p.suffix.lower() in SKIP_EXT:
            continue
        try:
            data = p.read_bytes()
        except OSError:
            continue
        if b"\x00" in data:
            continue
        text = data.decode("utf-8", errors="replace")
        for n, line in enumerate(text.splitlines(), 1):
            low, nl = line.lower(), _norm(line)
            for i, raw, nk in keys:
                if raw in low or nk in nl:
                    hits.append((str(p), n, i))
    return hits


def _load(path):
    out = []
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        s = line.strip()
        if s and not s.startswith("#"):
            out.append(s)
    return out


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    if len(argv) != 1:
        print("usage: check_private.py <dir>", file=sys.stderr)
        return 2
    lst = os.environ.get("KLACK_SITE_FORBIDDEN") or os.path.expanduser("~/.config/klack-site/forbidden.txt")
    if not os.path.isfile(lst):
        print("forbidden list missing: expected at " + lst, file=sys.stderr)
        return 2
    forbidden = _load(lst)
    hits = scan(argv[0], forbidden)
    for path, line, idx in hits:
        print("%s:%d: forbidden string #%d" % (path, line, idx + 1))
    return 1 if hits else 0


if __name__ == "__main__":
    sys.exit(main())

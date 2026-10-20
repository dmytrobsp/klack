"""Scan the site for strings that must never ship. Never prints a forbidden string."""
import html
import os
import re
import sys
from pathlib import Path

SKIP_EXT = {".woff", ".woff2", ".ttf", ".otf", ".png", ".jpg", ".jpeg", ".gif", ".webp", ".avif",
            ".ico", ".mp4", ".webm", ".ogg", ".mp3", ".wav", ".pdf", ".zip", ".gz"}
_TAG = re.compile(r"<[^>]*>")
_SEP = re.compile(r"[\s._-]+")


def _norm(s):
    return _SEP.sub("", s.lower())


def _matches(line, keys):
    low, nl = line.lower(), _norm(line)
    return [i for i, raw, nk in keys if raw in low or nk in nl]


def scan_counted(root, forbidden):
    """Return (hits, number of text files scanned). Paths are relative to root;
    a hit in a file NAME is reported as ("<name hidden>", 0, index)."""
    root = Path(root)
    keys = [(i, f.lower(), _norm(f)) for i, f in enumerate(forbidden)]
    keys = [k for k in keys if k[2]]
    hits, count = [], 0
    base = root.parent if root.is_file() else root
    files = [root] if root.is_file() else sorted(p for p in root.rglob("*") if p.is_file())
    for p in files:
        rel = p.relative_to(base).as_posix()
        for i in sorted({i for part in rel.split("/") for i in _matches(part, keys)}):
            hits.append(("<name hidden>", 0, i))
        if p.suffix.lower() in SKIP_EXT:
            continue
        try:
            data = p.read_bytes()
        except OSError:
            raise OSError("cannot read a file under the scan root")
        if b"\x00" in data:
            continue
        count += 1
        text = data.decode("utf-8", errors="replace")
        for n, line in enumerate(text.splitlines(), 1):
            found = set(_matches(line, keys))
            if "<" in line or "&" in line:
                found |= set(_matches(html.unescape(_TAG.sub("", line)), keys))
            for i in sorted(found):
                hits.append((rel, n, i))
    return hits, count


def scan(root, forbidden):
    """Return [(path, line, index into forbidden)] for every hit."""
    return scan_counted(root, forbidden)[0]


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
    if not os.path.exists(argv[0]):
        print("scan root does not exist", file=sys.stderr)
        return 2
    try:
        forbidden = _load(lst)
        hits, count = scan_counted(argv[0], forbidden)
    except (OSError, UnicodeError):
        print("could not read the forbidden list or a site file", file=sys.stderr)
        return 2
    if count == 0:
        print("no text files were scanned under the given root", file=sys.stderr)
        return 2
    for path, line, idx in hits:
        print("%s:%d: forbidden string #%d" % (path, line, idx + 1))
    return 1 if hits else 0


if __name__ == "__main__":
    sys.exit(main())

"""Scan the site for strings that must never ship. Never prints a forbidden string."""
import html
import os
import re
import subprocess
import sys
from pathlib import Path

SKIP_EXT = {".woff", ".woff2", ".ttf", ".otf", ".png", ".jpg", ".jpeg", ".gif", ".webp", ".avif",
            ".ico", ".mp4", ".webm", ".ogg", ".mp3", ".wav", ".pdf", ".zip", ".gz"}
_TAG = re.compile(r"<[^>]*>")
_SEP = re.compile(r"[\s._-]+")


# The repository's own public address may name its owner; the owner is read at run time, never written here.
_GAP = "\x00"  # what an allowed address leaves behind: no separator, so nothing around it joins up


def owner_from_remote(url):
    """The GitHub owner in a remote URL (https, ssh or scp form); None for anything else."""
    m = re.match(r"\s*(?:https?://|ssh://)?(?:[^@/\s]+@)?github\.com[:/]([A-Za-z0-9-]+)/", url or "")
    return m.group(1) if m else None


def allowed_forms(owner):
    """Patterns of the allowed addresses, the bare prefixes only (whatever follows is scanned): ttyak.sh, and with an
    owner github.com/<owner>/ttyak (also scoop-ttyak, homebrew-ttyak), <owner>.github.io/ttyak, <owner>.github.io,
    and the install commands' names: the repo <owner>/ttyak (`gh -R`, the Homebrew tap) and the winget id
    <owner>.ttyak. Case-insensitive."""
    end = r"(?![\w.-])"
    site = re.compile(rf"(?<![\w.-])ttyak\.sh{end}", re.I)
    if not owner:
        return (site,)
    o = re.escape(owner)
    return (site, re.compile(rf"(?<![\w.-])github\.com/{o}/(?:scoop-|homebrew-)?ttyak{end}", re.I),
            re.compile(rf"(?<![\w-]){o}\.github\.io(?:/ttyak{end})?{end}", re.I),
            re.compile(rf"(?<![\w./-]){o}(?:/|\.)ttyak{end}", re.I))


def _allow(line, allow):
    for pat in allow:
        line = pat.sub(_GAP, line)
    return line


def _norm(s):
    return _SEP.sub("", s.lower())


def _matches(line, keys):
    low, nl = line.lower(), _norm(line)
    return [i for i, raw, nk in keys if raw in low or nk in nl]


def scan_counted(root, forbidden, allow=()):
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
            found = set(_matches(_allow(line, allow), keys))
            if "<" in line or "&" in line:
                found |= set(_matches(_allow(html.unescape(_TAG.sub("", line)), allow), keys))
            for i in sorted(found):
                hits.append((rel, n, i))
    return hits, count


def scan(root, forbidden, allow=()):
    """Return [(path, line, index into forbidden)] for every hit."""
    return scan_counted(root, forbidden, allow)[0]


def _origin():
    try:
        r = subprocess.run(["git", "-C", str(Path(__file__).resolve().parent), "remote", "get-url", "origin"],
                           capture_output=True, text=True, timeout=10)
    except (OSError, subprocess.SubprocessError):
        return None
    return r.stdout if r.returncode == 0 else None


def _load(path):
    out = []
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        s = line.strip()
        if s and not s.startswith("#"):
            out.append(s)
    return out


def main(argv=None, remote=_origin):
    argv = list(sys.argv[1:] if argv is None else argv)
    arg_owner = None
    if "--allow-owner" in argv:
        i = argv.index("--allow-owner")
        if i + 1 >= len(argv):
            print("usage: check_private.py <dir> [--allow-owner NAME]", file=sys.stderr)
            return 2
        arg_owner = argv[i + 1]
        del argv[i:i + 2]
    if len(argv) != 1:
        print("usage: check_private.py <dir> [--allow-owner NAME]", file=sys.stderr)
        return 2
    allow = allowed_forms(owner_from_remote(remote()) or arg_owner)
    lst = os.environ.get("TTYAK_SITE_FORBIDDEN") or os.path.expanduser("~/.config/ttyak-site/forbidden.txt")
    if not os.path.isfile(lst):
        print("forbidden list missing: expected at " + lst, file=sys.stderr)
        return 2
    if not os.path.exists(argv[0]):
        print("scan root does not exist", file=sys.stderr)
        return 2
    try:
        forbidden = _load(lst)
        hits, count = scan_counted(argv[0], forbidden, allow)
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

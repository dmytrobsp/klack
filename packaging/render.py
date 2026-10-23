#!/usr/bin/env python3
"""Renders every package recipe and the site's latest.json from one release's SHA256SUMS.

Usage: python3 packaging/render.py --version V --sums SHA256SUMS --out DIR [--date YYYY-MM-DD]

Writes DIR/homebrew/ttyak.rb, DIR/scoop/ttyak.json, DIR/chocolatey/{ttyak.nuspec,tools/*.ps1},
DIR/aur/{PKGBUILD,.SRCINFO}, DIR/winget/manifests/<p>/<Publisher>/<Name>/<V>/*.yaml, DIR/nfpm.yaml and
DIR/latest.json. A platform missing from SHA256SUMS (only aarch64-linux may be) is left out of every output.
The release's other files listed in SHA256SUMS (packages, the recipes zip, signatures) are skipped. Refuses (exit 1,
nothing written) a V that is not a release version, a malformed or repeated SHA256SUMS line, a path in it, a
`ttyak-….zip` that is not one of V's archives, a missing required platform, a bad --date, a non-empty --out.

Templates (packaging/templates) hold `{{key}}`, replaced by the value quoted for the template's format, and
`{{key:raw}}` (comment lines only); a line `{{if <platform>}}` up to a line `{{end}}` is kept only when SHA256SUMS
has that platform, `{{unless <platform>}}` … `{{end}}` only when it has not. --out must be empty or not there yet.
Python 3 standard library only."""
import argparse
import datetime
import json
import os
import re
import sys
import unicodedata
from xml.sax.saxutils import escape as xml_escape

HERE = os.path.dirname(os.path.abspath(__file__))
TEMPLATE_DIR = os.path.join(HERE, "templates")
IDENTITY_FILE = os.path.join(HERE, "identity.json")
REPO = "dmytrobsp/ttyak"
# The release platforms, in this order everywhere; every one but aarch64-linux must be in SHA256SUMS.
PLATFORMS = ["x86_64-linux", "aarch64-linux", "x86_64-macos", "aarch64-macos", "x86_64-windows"]
REQUIRED = ["x86_64-linux", "x86_64-macos", "aarch64-macos", "x86_64-windows"]
IDENTITY_FIELDS = ["name", "email", "winget_id", "homepage", "license"]
# The AUR package's fields (PKGBUILD and .SRCINFO): depends are what `ldd ttyak` lists, as Arch packages (namcap
# names libgcc and libstdc++, split from gcc-libs); the release binary goes in as it is (no strip, no -debug package).
AUR = {
    "pkgname": "ttyak-bin",
    "pkgdesc": "Slack client for the terminal: threads, images, notifications, search and huddles with audio",
    "depends": ["alsa-lib", "glibc", "libgcc", "libstdc++"],
    "provides": ["ttyak"],
    "conflicts": ["ttyak"],
    "options": ["!debug", "!strip"],
}
AUR_ARCH = {"x86_64-linux": "x86_64", "aarch64-linux": "aarch64"}
# template file, its format, where it goes under --out ({winget} is the winget manifest folder, {id} the winget id).
TEMPLATES = [
    ("ttyak.rb", "ruby", "homebrew/ttyak.rb"),
    ("ttyak.json", "json", "scoop/ttyak.json"),
    ("ttyak.nuspec", "xml", "chocolatey/ttyak.nuspec"),
    ("chocolateyInstall.ps1", "powershell", "chocolatey/tools/chocolateyInstall.ps1"),
    ("chocolateyUninstall.ps1", "powershell", "chocolatey/tools/chocolateyUninstall.ps1"),
    ("PKGBUILD", "shell", "aur/PKGBUILD"),
    ("winget.yaml", "yaml", "{winget}/{id}.yaml"),
    ("winget.installer.yaml", "yaml", "{winget}/{id}.installer.yaml"),
    ("winget.locale.en-US.yaml", "yaml", "{winget}/{id}.locale.en-US.yaml"),
    ("nfpm.yaml", "yaml", "nfpm.yaml"),
]
FORMATS = ["ruby", "json", "xml", "powershell", "shell", "yaml"]
# Semantic Versioning 2.0.0 without build metadata (`+…`, which the install scripts refuse too).
SEMVER = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
                    r"(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)(\.(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?")
SUMS_LINE = re.compile(r"([0-9A-Fa-f]{64})(?:  | \*)(\S+)")


class Refused(Exception):
    """Input render.py will not turn into recipes."""


def check_version(v):
    """`v` when it is a release version, else Refused."""
    if not SEMVER.fullmatch(v):
        raise Refused("%r is not a release version (X.Y.Z or X.Y.Z-pre, semver)" % v)
    return v


def check_date(d):
    """`d` when it is a real YYYY-MM-DD date, else Refused."""
    try:
        if re.fullmatch(r"[0-9]{4}-[0-9]{2}-[0-9]{2}", d) and datetime.date.fromisoformat(d):
            return d
    except ValueError:
        pass
    raise Refused("--date %r is not a YYYY-MM-DD date" % d)


def parse_sums(text, version):
    """{platform: lower-case sha256} of `version`'s archives from the release's SHA256SUMS. Every line must be
    `<64 hex>  <file name>`, each name once; the release's other files (packages, `ttyak-V-recipes.zip`, signatures)
    are skipped, but a `ttyak-….zip` that is not one of `version`'s archives is refused."""
    out, seen = {}, set()
    prefix, suffix = "ttyak-%s-" % version, ".zip"
    for n, line in enumerate(text.split("\n"), 1):
        line = line[:-1] if line.endswith("\r") else line
        if not line:
            continue
        m = SUMS_LINE.fullmatch(line)
        name = m.group(2) if m else ""
        if not m or "/" in name or "\\" in name or name in seen:
            raise Refused("SHA256SUMS line %d is not `<64 hex>  <file name>` for a file not seen yet: %r" % (n, line))
        seen.add(name)
        if not (name.startswith("ttyak-") and name.endswith(suffix)):
            continue
        platform = name[len(prefix):-len(suffix)] if name.startswith(prefix) else None
        if platform == "recipes":
            continue
        if platform not in PLATFORMS:
            raise Refused("SHA256SUMS line %d names a zip that is not one of ttyak-%s-<platform>.zip: %r" % (n, version, line))
        out[platform] = m.group(1).lower()
    for p in REQUIRED:
        if p not in out:
            raise Refused("SHA256SUMS has no archive for " + p)
    return out


def load_identity(path=IDENTITY_FILE):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def check_identity(identity):
    for k in IDENTITY_FIELDS:
        if not isinstance(identity.get(k), str) or not identity[k]:
            raise Refused("identity.json: no %s" % k)
        plain(identity[k])
    if not re.fullmatch(r"[A-Za-z0-9-]+\.[A-Za-z0-9.-]+", identity["winget_id"]):
        raise Refused("identity.json: winget_id %r is not Publisher.Name" % identity["winget_id"])
    return identity


def plain(s):
    """`s` when it holds no control, format or line-separator character (nothing that could end a line or hide)."""
    if any(unicodedata.category(c) in ("Cc", "Cf", "Zl", "Zp") for c in s):
        raise Refused("a control character in %r" % s)
    return s


def quote(fmt, value):
    """`value` as a literal of `fmt`: a quoted string (a list: quoted strings, for shell and Ruby)."""
    if isinstance(value, list):
        sep = {"shell": " ", "ruby": ", "}.get(fmt)
        if sep is None:
            raise Refused("a list in a %s template" % fmt)
        return sep.join(quote(fmt, v) for v in value)
    plain(value)
    if fmt == "ruby":
        return '"' + value.replace("\\", "\\\\").replace('"', '\\"').replace("#", "\\#") + '"'
    if fmt in ("json", "yaml"):
        # A JSON string is a YAML double-quoted scalar.
        return json.dumps(value, ensure_ascii=False)
    if fmt == "xml":
        return xml_escape(value, {'"': "&quot;", "'": "&apos;"})
    if fmt == "powershell":
        # PowerShell takes the typographic single quotes as quotes too.
        return "'" + re.sub("['‘’‚‛]", lambda m: m.group() * 2, value) + "'"
    if fmt == "shell":
        return "'" + value.replace("'", "'\\''") + "'"
    raise Refused("no format " + fmt)


PLACEHOLDER = re.compile(r"\{\{([a-z0-9_]+)(?::([a-z]+))?\}\}")
BLOCK = re.compile(r"\s*\{\{(?:(if|unless) ([a-z0-9_-]+)|(end))\}\}\s*")


def fill(text, fmt, values, present):
    """The template `text` with its placeholders filled; an `if` block kept only when its platform is `present`, an
    `unless` block only when it is not."""
    out, block = [], None
    for line in text.splitlines(keepends=True):
        m = BLOCK.fullmatch(line)
        if m and m.group(1):
            if block is not None or m.group(2) not in PLATFORMS:
                raise Refused("a nested block or an unknown platform: %r" % line)
            block = (m.group(1), m.group(2))
            continue
        if m:
            if block is None:
                raise Refused("{{end}} without {{if}} or {{unless}}")
            block = None
            continue
        if block is not None and (block[1] in present) != (block[0] == "if"):
            continue

        def sub(p):
            key, how = p.group(1), p.group(2)
            if key not in values:
                raise Refused("no value for {{%s}}" % key)
            if how is None:
                return quote(fmt, values[key])
            if how == "raw" and line.lstrip().startswith("#") and isinstance(values[key], str):
                return plain(values[key])
            raise Refused("{{%s:%s}} (only :raw, on a comment line)" % (key, how))

        line = PLACEHOLDER.sub(sub, line)
        if "{{" in line:
            raise Refused("a placeholder not understood: %r" % line)
        out.append(line)
    if block is not None:
        raise Refused("{{%s %s}} without {{end}}" % block)
    return "".join(out)


def asset_url(version, platform):
    return "https://github.com/%s/releases/download/v%s/ttyak-%s-%s.zip" % (REPO, version, version, platform)


def values_for(version, date, sha, identity):
    """What the templates fill in."""
    present = [p for p in PLATFORMS if p in sha]
    v = {
        "version": version,
        "date": date,
        "notes": "https://github.com/%s/releases/tag/v%s" % (REPO, version),
        "name": identity["name"],
        "maintainer": "%s <%s>" % (identity["name"], identity["email"]),
        "homepage": identity["homepage"],
        "license": identity["license"],
        "license_ids": identity["license"].split(" OR "),
        "scoop_license": identity["license"].replace(" OR ", "|"),
        "winget_id": identity["winget_id"],
        "pkgver": version.replace("-", ""),
        "aur_arch": [AUR_ARCH[p] for p in present if p in AUR_ARCH],
        "aur_license": [identity["license"]],
    }
    for k in AUR:
        v["aur_" + k] = AUR[k]
    for p in present:
        k = p.replace("-", "_")
        v["url_" + k] = asset_url(version, p)
        v["sha256_" + k] = sha[p]
        v["sha256_upper_" + k] = sha[p].upper()
        v["source_" + k] = "ttyak-%s-%s.zip::%s" % (version, p, asset_url(version, p))
    return v


def srcinfo(v, sha):
    """The .SRCINFO `makepkg --printsrcinfo` writes for the rendered PKGBUILD."""
    lines = ["pkgbase = " + v["aur_pkgname"]]
    for key in ["pkgdesc", "pkgver"]:
        lines.append("\t%s = %s" % (key, v["aur_" + key] if key == "pkgdesc" else v[key]))
    lines += ["\tpkgrel = 1", "\turl = " + v["homepage"]]
    for key, items in [("arch", v["aur_arch"]), ("license", v["aur_license"]), ("depends", v["aur_depends"]),
                       ("provides", v["aur_provides"]), ("conflicts", v["aur_conflicts"]), ("options", v["aur_options"])]:
        lines += ["\t%s = %s" % (key, i) for i in items]
    for p, arch in AUR_ARCH.items():
        if p in sha:
            k = p.replace("-", "_")
            lines += ["\tsource_%s = %s" % (arch, v["source_" + k]), "\tsha256sums_%s = %s" % (arch, v["sha256_" + k])]
    lines += ["", "pkgname = " + v["aur_pkgname"]]
    return "\n".join(lines) + "\n"


def latest_json(version, date, sha):
    """The site's latest.json, which ttyak's update check reads."""
    feed = {
        "name": "ttyak",
        "version": version,
        "date": date,
        "notes": "https://github.com/%s/releases/tag/v%s" % (REPO, version),
        "assets": {p: {"url": asset_url(version, p), "sha256": sha[p]} for p in PLATFORMS if p in sha},
    }
    return json.dumps(feed, separators=(",", ":")) + "\n"


def render_all(version, sums_text, out, date, identity):
    """Writes every recipe and latest.json under `out`; returns the paths written. Checks everything first, so a
    refusal writes nothing."""
    check_version(version)
    check_date(date)
    check_identity(identity)
    if os.path.isdir(out) and os.listdir(out):
        raise Refused("--out %s is not empty" % out)
    sha = parse_sums(sums_text, version)
    v = values_for(version, date, sha, identity)
    publisher, name = identity["winget_id"].split(".", 1)
    winget = "/".join(["winget", "manifests", publisher[0].lower(), publisher, name, version])
    files = {}
    for template, fmt, dest in TEMPLATES:
        with open(os.path.join(TEMPLATE_DIR, template), encoding="utf-8", newline="") as f:
            files[dest.format(winget=winget, id=identity["winget_id"])] = fill(f.read(), fmt, v, set(sha))
    files["aur/.SRCINFO"] = srcinfo(v, sha)
    files["latest.json"] = latest_json(version, date, sha)
    written = []
    for rel, text in files.items():
        path = os.path.join(out, *rel.split("/"))
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8", newline="") as f:
            f.write(text)
        written.append(path)
    return written


def main(argv=None):
    ap = argparse.ArgumentParser(description="Render the package recipes and latest.json from a release's SHA256SUMS.")
    ap.add_argument("--version", required=True, help="the release version, X.Y.Z or X.Y.Z-pre")
    ap.add_argument("--sums", required=True, help="the release's SHA256SUMS")
    ap.add_argument("--out", required=True, help="the folder to write into")
    ap.add_argument("--date", help="the release date, YYYY-MM-DD (default: today, UTC)")
    a = ap.parse_args(argv)
    date = a.date or datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d")
    try:
        with open(a.sums, encoding="utf-8") as f:
            text = f.read()
        written = render_all(a.version, text, a.out, date, load_identity())
    except (Refused, OSError, UnicodeDecodeError, ValueError) as e:
        print("render.py: %s" % e, file=sys.stderr)
        return 1
    for path in written:
        print(path)
    return 0


if __name__ == "__main__":
    sys.exit(main())

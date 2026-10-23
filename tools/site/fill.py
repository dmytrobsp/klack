#!/usr/bin/env python3
"""Fills the generated regions of site/index.html from data (stdlib only).

    python3 tools/site/fill.py site [--fetch-release [--strict]]
"""
import html as _html
import json
import os
import re
import sys
import urllib.error
import urllib.request
from pathlib import Path

REPO = "dmytrobsp/ttyak"
ORDER = ["x86_64-linux", "aarch64-linux", "aarch64-macos", "x86_64-macos", "x86_64-windows"]
NAMES = {"x86_64-linux": "Linux", "aarch64-linux": "Linux (arm64)", "aarch64-macos": "macOS · Apple Silicon",
         "x86_64-macos": "macOS · Intel", "x86_64-windows": "Windows"}


class ReleaseFetchError(Exception):
    pass


def fill(html, name, content):
    start, end = f"<!-- fill:{name} -->", f"<!-- /fill:{name} -->"
    i = html.find(start)
    j = html.find(end, i + len(start)) if i >= 0 else -1
    if i < 0 or j < 0:
        raise KeyError(name)
    return html[:i + len(start)] + content + html[j:]


def fmt_size(n):
    return f"{n / 1e6:.1f} MB"


esc = lambda s: _html.escape(str(s), quote=True)
txt = lambda s: _html.escape(str(s), quote=False)  # text nodes: apostrophes stay readable


def _find(release, platform):
    return next((a for a in release["assets"] if a["platform"] == platform), None)


def render_download_button(release):
    lin = _find(release, "x86_64-linux") or release["assets"][0]
    mac = _find(release, "aarch64-macos")
    win = _find(release, "x86_64-windows")
    link = lambda a, n: f'<a href="{esc(a["url"])}">{n}</a>' if a else n
    return (f'<a class="dl" data-os="linux" href="{esc(lin["url"])}">DOWNLOAD FOR LINUX</a>'
            f'<span class="dl-meta">v{esc(release["version"])} · {fmt_size(lin["size"])} · '
            f'<span class="dl-others">{link(mac, "macOS")} · {link(win, "Windows")} · '
            f'<a href="#get">all downloads</a></span></span>')


def _plain(s):
    s = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", s)
    return s.replace("`", "").strip()


def readme_needs(text):
    out, inside = {}, False
    for line in text.splitlines():
        if line.startswith("## "):
            inside = line.strip() == "## Install"
            continue
        if not inside or not line.startswith("|"):
            continue
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) < 3:
            continue
        m = re.fullmatch(r"`?([\w.-]+)\.zip`?", cells[0])
        if m:
            out[m.group(1)] = (_plain(cells[1]), _plain(cells[2]))
    return out


def render_downloads(release, needs):
    rank = lambda a: ORDER.index(a["platform"]) if a["platform"] in ORDER else len(ORDER)
    rows = []
    for a in sorted(release["assets"], key=rank):  # stable: unknown ones keep release order
        runs, need = needs.get(a["platform"], ("", ""))
        rows.append(f'<tr><th scope="row"><a href="{esc(a["url"])}">{esc(NAMES.get(a["platform"], a["platform"]))}</a>'
                    f'<code>{esc(a["platform"])}.zip</code></th><td>{fmt_size(a["size"])}</td>'
                    f'<td>{esc(runs)}</td><td>{esc(need)}</td></tr>')
    return ('<table class="downloads"><thead><tr><th>Download</th><th>Size</th><th>Runs on</th><th>Needs</th></tr></thead>'
            f'<tbody>{"".join(rows)}</tbody></table>'
            f'<p class="sums"><a href="{esc(release["sums_url"])}">SHA256SUMS</a> · '
            f'<a href="{esc(release["url"])}">release v{esc(release["version"])}</a></p>')


def release_from_api(repo, token, urlopen=urllib.request.urlopen):
    req = urllib.request.Request(f"https://api.github.com/repos/{repo}/releases/latest",
                                 headers={"Accept": "application/vnd.github+json", "User-Agent": "ttyak-site"})
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    try:
        with urlopen(req, timeout=20) as r:
            data = json.loads(r.read().decode())
    except (urllib.error.URLError, OSError, ValueError) as e:
        raise ReleaseFetchError(str(e)) from e
    try:
        version = data["tag_name"].lstrip("v")
        assets, sums = [], None
        for a in data["assets"]:
            if a["name"] == "SHA256SUMS":
                sums = a["browser_download_url"]
                continue
            m = re.fullmatch(rf"ttyak-{re.escape(version)}-(.+)\.zip", a["name"])
            if m and m.group(1) != "recipes":  # the package recipes, no download
                assets.append({"name": a["name"], "platform": m.group(1),
                               "url": a["browser_download_url"], "size": a["size"]})
        if not assets or not sums:
            raise ReleaseFetchError("release has no zips or no SHA256SUMS")
        return {"version": version, "url": data["html_url"], "sums_url": sums, "assets": assets}
    except (KeyError, TypeError) as e:
        raise ReleaseFetchError(f"unexpected reply: {e!r}") from e


# --- the spec sheet (LP10): the official app's numbers crossed out only where ttyak wins by WIN or more
WIN = 1.5
PLACEHOLDER = ('<p class="standin-note">measuring… no number goes up here until '
               "it's been measured.</p>")
TIES = {"startup": "a draw. not bad for a weekend.",
        "cpu_share": "video costs what it costs. even in a terminal."}
TIE = "ok, a tie. it's a chat app."
# cpu_idle's loss points at the row after it (cpu_scroll, a win: test_fill checks the order in numbers.json)
LOSSES = {"cpu_idle": "yeah, okay. now look at the next one"}
LOSS = "fine, this one's theirs."
ZERO = "it's text."


def _mb(mb):
    return f"{mb / 1e3:.1f} GB" if round(mb, 1) >= 1000 else f"{mb:.1f} MB"


def fmt_value(v, unit, small=False):
    """small: a percent row with a side under 1 % — two decimals, so a gap like 0.45 vs 0.63 isn't rounded away."""
    if v == 0:
        return "none"
    if unit == "bytes":
        return _mb(v / 1e6)
    if unit == "kB":
        return _mb(v / 1e3)
    if unit == "percent":
        return f"{v:.2f}%" if small else f"{v:.1f}%"
    if unit == "seconds":
        return f"{v:.2f} s" if v < 10 else f"{v:.1f} s"
    raise ValueError(f"unknown unit {unit!r}")


def fmt_ratio(r):
    return f"{round(r)}" if round(r, 1) >= 10 else f"{r:.1f}"


def classify(r):
    k, o = r["ttyak"], r["slack"]
    if k == 0:
        return "zero" if o > 0 else "tie"
    if o / k >= WIN:
        return "win"
    return "loss" if k > o else "tie"


def _row(r):
    k, o, unit = r["ttyak"], r["slack"], r["unit"]
    cpu = unit == "percent"  # CPU share depends on the machine; the ratio between the apps carries over
    small = cpu and min(k, o) < 1  # both cells of the row share one precision
    kv, ov = txt(fmt_value(k, unit, small)), txt(fmt_value(o, unit, small))
    kind = classify(r)
    marks = kind in ("zero", "win")
    what = " CPU" if cpu else ""
    if kind == "win":
        word = "faster" if unit == "seconds" else "less"
        scrawl = f'<span class="scrawl ratio">{fmt_ratio(o / k)}× {word}{what}</span>'
    else:  # a bespoke scrawl only ever in its own class: TIES for a tie, LOSSES for a loss
        line = {"zero": ZERO, "loss": LOSSES.get(r["id"], LOSS), "tie": TIES.get(r["id"], TIE)}[kind]
        scrawl = f'<span class="scrawl">{txt(line)}</span>'
        if kind == "loss" and cpu and o > 0:  # a CPU loss leads with how much, honestly
            scrawl = f'<span class="scrawl ratio more">{fmt_ratio(k / o)}× more CPU</span>' + scrawl
    them = f'<span class="crossed">{ov}</span>' if marks else f'<span class="plain">{ov}</span>'
    us = f'<span class="circled">{kv}</span>' if marks else f'<span class="plain">{kv}</span>'
    ev = " evidence" if cpu else ""  # the raw percentages sit small beside the ratio
    return (f'<tr class="row" data-id="{esc(r["id"])}"><th scope="row">{txt(r["label"])}</th>'
            f'<td class="them{ev}" data-side="the official app">{them}</td>'
            f'<td class="us{ev}" data-side="ttyak">{us}</td><td class="remark">{scrawl}</td></tr>')


def render_sheet(numbers):
    rows = numbers.get("rows") or []
    if not rows:
        return PLACEHOLDER
    return ('<div class="sheet paper"><table><thead><tr><th scope="col"><span>lower is better</span></th>'
            '<th scope="col" class="them">the official app</th><th scope="col" class="us">ttyak</th>'
            '<td></td></tr></thead>'
            f'<tbody>{"".join(_row(r) for r in rows)}</tbody></table></div>')


def render_measured_note(numbers):
    if not numbers.get("rows"):
        return ""
    v = numbers["versions"]
    return (f'<p class="measured">ttyak (version {txt(v["ttyak"])}) against Slack\'s desktop app {txt(v["slack"])}, '
            f'measured on Linux on {txt(numbers["measured_on"])}: {txt(numbers["machine"])}. '
            "App against app — the terminal ttyak runs in isn't counted. "
            "Slack's RAM figures are a lower bound: one of its helper processes couldn't be read. "
            f'<a href="https://github.com/{REPO}/blob/master/tools/site/MEASURING.md">How it was measured</a>.</p>')


# fill region -> site/captures/<name>.html (Task 7); a region whose capture is missing keeps what it has
CAPTURES = {"hero-capture": "hero", "capture-huddle": "huddle", "capture-signin": "signin",
            "capture-keys": "keys"}


def fill_captures(html, captures):
    for region, name in CAPTURES.items():
        p = Path(captures) / f"{name}.html"
        if p.is_file():
            html = fill(html, region, p.read_text(encoding="utf-8").strip())
    return html


def fill_page(site, release=None, numbers=None, needs=None):
    site = Path(site)
    if release is None:
        release = json.loads((site / "data/release.json").read_text())
    if needs is None:
        readme = site.resolve().parent / "README.md"
        needs = readme_needs(readme.read_text()) if readme.exists() else {}
    page = site / "index.html"
    html = page.read_text()
    html = fill(html, "download-button", render_download_button(release))
    html = fill(html, "downloads", render_downloads(release, needs))
    html = fill_captures(html, site / "captures")
    if numbers is None and (site / "data/numbers.json").is_file():
        numbers = json.loads((site / "data/numbers.json").read_text())
    if numbers is not None:
        html = fill(html, "sheet", render_sheet(numbers))
        html = fill(html, "measured-note", render_measured_note(numbers))
    page.write_text(html)


def main(argv, urlopen=urllib.request.urlopen, stderr=sys.stderr, readme=None):
    args = [a for a in argv if not a.startswith("--")]
    site = Path(args[0])
    release = None
    if "--fetch-release" in argv:
        try:
            release = release_from_api(REPO, os.environ.get("GITHUB_TOKEN"), urlopen=urlopen)
            (site / "data/release.json").write_text(json.dumps(release, indent=2) + "\n")
        except ReleaseFetchError as e:
            if "--strict" in argv:  # a deploy must not quietly go out with stale links: the last good one stays live
                print(f"release fetch failed: {e}", file=stderr)
                return 1
            print(f"release fetch failed: {e}; keeping site/data/release.json", file=stderr)
            release = None
    needs = readme_needs(Path(readme).read_text()) if readme else None
    fill_page(site, release, needs=needs)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

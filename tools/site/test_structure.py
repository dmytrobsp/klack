import json
import re
import unittest
from html.parser import HTMLParser
from pathlib import Path

SITE = Path(__file__).resolve().parents[2] / "site"
SECTIONS = ["top", "features", "numbers", "get", "footer"]
FILLS = ["hero-capture", "download-button", "downloads", "sheet", "measured-note",
         "capture-huddle", "capture-signin", "capture-keys"]
OWN = "https://github.com/dmytrobsp/ttyak"


class Page(HTMLParser):
    def __init__(self):
        super().__init__()
        self.html_lang = None
        self.title = ""
        self._in_title = False
        self.viewport = False
        self.section_ids = []
        self.nav_hrefs = []
        self._nav = 0
        self.urls = []  # src/href of <link>/<script>
        self.fill_open = []
        self.fill_close = []

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if tag == "html":
            self.html_lang = a.get("lang")
        elif tag == "title":
            self._in_title = True
        elif tag == "meta" and a.get("name") == "viewport":
            self.viewport = True
        elif tag == "nav":
            self._nav += 1
        elif tag == "a" and self._nav and a.get("href", "").startswith("#"):
            self.nav_hrefs.append(a["href"][1:])
        elif tag in ("section", "footer") and a.get("id"):
            self.section_ids.append(a["id"])
        if tag in ("link", "script"):
            for k in ("src", "href"):
                if k in a:
                    self.urls.append(a[k])

    def handle_endtag(self, tag):
        if tag == "title":
            self._in_title = False
        elif tag == "nav":
            self._nav -= 1

    def handle_data(self, data):
        if self._in_title:
            self.title += data

    def handle_comment(self, data):
        d = data.strip()
        m = re.fullmatch(r"fill:([a-z-]+)", d)
        if m:
            self.fill_open.append(m.group(1))
        m = re.fullmatch(r"/fill:([a-z-]+)", d)
        if m:
            self.fill_close.append(m.group(1))


def parse():
    p = Page()
    p.feed((SITE / "index.html").read_text())
    return p


class Structure(unittest.TestCase):
    def test_head(self):
        p = parse()
        self.assertEqual(p.html_lang, "en")
        self.assertEqual(p.title.strip(), "ttyak — Slack, minus the browser")
        self.assertTrue(p.viewport)

    def test_wordmark_and_favicon(self):
        page = (SITE / "index.html").read_text()
        self.assertIn('<a class="wordmark" href="#top">ttyak</a>', page)
        self.assertIn('<link rel="icon" href="img/favicon.svg" type="image/svg+xml">', page)
        self.assertTrue((SITE / "img" / "favicon.svg").read_text().startswith("<svg"))

    def test_no_old_name_outside_marked_lines(self):
        # the release links of builds made before the rename carry old-name-ok; nothing else says the old name
        for p in sorted(SITE.rglob("*")):
            if p.suffix not in (".html", ".css", ".js", ".json", ".svg"):
                continue
            for n, line in enumerate(p.read_text().splitlines(), 1):
                if re.search("(?i)k" + "lack", line):
                    self.assertIn("old-name-ok", line, "%s:%d" % (p.relative_to(SITE), n))

    def test_sections_in_order(self):
        self.assertEqual(parse().section_ids, SECTIONS)

    def test_nav_resolves(self):
        p = parse()
        self.assertTrue(p.nav_hrefs)
        for h in p.nav_hrefs:
            self.assertIn(h, p.section_ids)

    def test_fill_markers(self):
        p = parse()
        for name in FILLS:
            self.assertEqual(p.fill_open.count(name), 1, name)
            self.assertEqual(p.fill_close.count(name), 1, name)
        self.assertEqual(sorted(p.fill_open), sorted(FILLS))

    def test_no_third_party_requests(self):
        p = parse()
        for u in p.urls:
            self.assertFalse(re.match(r"(https?:)?//", u) and not u.startswith(OWN), u)
        css = (SITE / "css" / "site.css").read_text()
        for u in re.findall(r"url\(\s*['\"]?([^'\")]+)", css):
            if u.startswith("data:"):
                continue
            self.assertFalse(re.match(r"(https?:)?//", u), u)
        self.assertNotIn("@import", css)

    def test_fonts_exist(self):
        css = (SITE / "css" / "site.css").read_text()
        files = re.findall(r"url\(\s*['\"]?(\.\./fonts/[^'\")]+\.woff2)", css)
        self.assertEqual(len(files), 5)
        for f in files:
            self.assertTrue((SITE / "css" / f).resolve().is_file(), f)
        self.assertEqual(css.count("font-display: swap"), 5)

    def test_tokens(self):
        css = (SITE / "css" / "site.css").read_text()
        for tok in ("aubergine", "pink", "blue", "green", "yellow", "bg", "cream", "scrap"):
            self.assertIn("--%s:" % tok, css)
        for cls in ("cut", "paper", "sticker", "scrawl", "jab", "term"):
            self.assertIn(".%s" % cls, css)

    def test_release_json(self):
        r = json.loads((SITE / "data" / "release.json").read_text())
        v = r["version"]
        self.assertRegex(v, r"^\d+\.\d+\.\d+$")
        self.assertEqual(len(r["assets"]), 4)
        for a in r["assets"]:
            self.assertIn(a["name"], ("ttyak-%s-%s.zip" % (v, a["platform"]),
                                      "klack-%s-%s.zip" % (v, a["platform"])))  # old-name-ok: before 0.10
            self.assertTrue(a["url"].startswith(OWN + "/releases/download/v%s/" % v))
            self.assertIsInstance(a["size"], int)
        self.assertTrue(r["sums_url"].endswith("/SHA256SUMS"))

    def test_numbers_json(self):
        n = json.loads((SITE / "data" / "numbers.json").read_text())
        for key in ("measured_on", "machine", "versions", "rows"):
            self.assertIn(key, n)
        self.assertEqual(n["versions"]["ttyak"], "0.8.0")
        ids = [r["id"] for r in n["rows"]]
        for want in ("download", "disk", "ram_rest", "ram_busy", "cpu_idle", "startup", "cpu_share", "ram_share"):
            self.assertIn(want, ids)
        self.assertEqual(len(ids), len(set(ids)))
        for r in n["rows"]:
            self.assertIn(r["unit"], ("bytes", "kB", "percent", "seconds"))
            self.assertIsInstance(r["label"], str)
            for side in ("ttyak", "slack"):
                self.assertIsInstance(r[side], (int, float))
                self.assertGreaterEqual(r[side], 0)


if __name__ == "__main__":
    unittest.main()

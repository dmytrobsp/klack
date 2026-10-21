"""The page's words: approved lines verbatim, jabs only at Slack, honest claims (LP5, the user's reviews LP12a/b)."""
import re
import unittest
from html.parser import HTMLParser
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SITE = ROOT / "site"
README = (ROOT / "README.md").read_text()

APPROVED = ["SLACK,", "MINUS THE", "BULLSHIT.", "BROWSER.", "YOUR TERMINAL JUST LEARNED TO HUDDLE.",
            "Yes, really.", "NO CHROMIUM WAS HARMED", "Quit with Ctrl+Q. We're not monsters.",
            # the user's second review (LP12a)
            "Three ways in. Pick one.",
            "We can even lift your session straight out of your browser. Just don't tell the cops.",
            "You bet it's keyboard-driven!", "Yes, you can use the mouse. But should you?",
            "Pipe it, cron it, grep it.", "because automation is for cool kids",
            "All of it in one terminal tab. Not a Chromium in sight.",
            "WORKS ON MY MACHINE™", "CERTIFIED",
            # the user's third review (LP12b), and the fourth (LP12c)
            "Windows XP ran in 64 MB.",
            "Because a messenger should not require a gaming rig."]
# the user took these out (LP12a); "Slack Desktop™" and "the Electron thing" are too cheesy
GONE = ["SSH forwards ports, not vocal cords.", "Chat over SSH: yes.", "You join muted", "Opening your browser",
        "modal dialog", "wire it into CI", "And the rest", "Measured, not guessed", "sound of my freedom",
        "Every number on this page was measured on Linux", "Slack Desktop", "the Electron thing", "Grow the beans",
        # and these in the third (LP12b)
        "trench coat", "In this economy", "how much RAM", "WHO HURT YOU", "No installer", "auto-updater",
        "Slack Helper (Renderer)",
        # and these in the fourth (LP12c)
        "Slack needs ten of those", "Of course it works over SSH", "Only if you shout really loud"]
BANNED = ["other terminal clients", "other clients", "the others can't"]

VOID = {"area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr"}
BLOCK = {"p", "div", "li", "ul", "ol", "h1", "h2", "h3", "h4", "section", "footer", "aside", "figure",
         "figcaption", "pre", "td", "th", "tr", "table", "blockquote", "nav", "header"}


class Node:
    def __init__(self, tag, attrs=None, parent=None, kind="el", data=""):
        self.tag, self.attrs, self.parent, self.kind, self.data = tag, dict(attrs or {}), parent, kind, data
        self.children = []

    @property
    def classes(self):
        return (self.attrs.get("class") or "").split()

    def elements(self):
        for c in self.children:
            if c.kind == "el":
                yield c
                yield from c.elements()

    def text(self, skip_fills=False):
        out = []
        self._text(out, skip_fills, [False])
        return re.sub(r"[ \t\r\f\v]+", " ", "".join(out))

    def _text(self, out, skip_fills, inside_fill):
        for c in self.children:
            if c.kind == "comment":
                d = c.data.strip()
                if re.fullmatch(r"fill:[a-z-]+", d):
                    inside_fill[0] = True
                elif re.fullmatch(r"/fill:[a-z-]+", d):
                    inside_fill[0] = False
            elif c.kind == "text":
                if not (skip_fills and inside_fill[0]):
                    out.append(c.data)
            elif c.tag in ("script", "style"):
                continue
            elif c.tag == "br":
                out.append(" ")
            else:
                if c.tag in BLOCK:
                    out.append("\n")
                c._text(out, skip_fills, inside_fill)
                if c.tag in BLOCK:
                    out.append("\n")


class Tree(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.root = self.cur = Node("#root")

    def handle_starttag(self, tag, attrs):
        n = Node(tag, attrs, self.cur)
        self.cur.children.append(n)
        if tag not in VOID:
            self.cur = n

    def handle_startendtag(self, tag, attrs):
        self.cur.children.append(Node(tag, attrs, self.cur))

    def handle_endtag(self, tag):
        n = self.cur
        while n is not self.root and n.tag != tag:
            n = n.parent
        if n is not self.root:
            self.cur = n.parent

    def handle_data(self, data):
        self.cur.children.append(Node("#text", parent=self.cur, kind="text", data=data))

    def handle_comment(self, data):
        self.cur.children.append(Node("#comment", parent=self.cur, kind="comment", data=data))


def page():
    t = Tree()
    t.feed((SITE / "index.html").read_text())
    return t.root


def by_id(root, i):
    return next(e for e in root.elements() if e.attrs.get("id") == i)


def flat(s):
    return re.sub(r"\s+", " ", s).strip()


def sentences(s):
    return [x.strip() for x in re.split(r"(?<=[.!?])\s+|\n", s) if x.strip()]


class Copy(unittest.TestCase):
    def setUp(self):
        self.root = page()
        self.all = flat(self.root.text())

    def test_approved_lines_verbatim(self):
        for line in APPROVED:
            self.assertIn(line, self.all, line)

    def test_removed_lines_stay_out(self):
        for line in GONE:
            self.assertNotIn(line, self.all, line)

    def test_the_official_app_is_not_a_refrain(self):
        # once in the sheet's column header (its one clear name) and its phone labels, nowhere in the prose
        prose = flat(self.root.text(skip_fills=True)).lower()
        self.assertNotIn("the official app", prose)
        sheet = (SITE / "index.html").read_text().split("<!-- fill:sheet -->")[1].split("<!-- /fill:sheet -->")[0]
        self.assertIn('<th scope="col" class="them">the official app</th>', sheet)

    def test_no_jabs_at_terminal_clients(self):
        low = self.all.lower()
        for b in BANNED:
            self.assertNotIn(b, low, b)

    def test_slk_only_in_footer(self):
        footer = flat(by_id(self.root, "footer").text())
        n_all = len(re.findall(r"\bslk\b", self.all, re.I))
        n_footer = len(re.findall(r"\bslk\b", footer, re.I))
        self.assertGreater(n_footer, 0)
        self.assertEqual(n_all, n_footer)

    def test_huddles_never_over_ssh(self):
        # page-wide: a block that mentions both huddles and SSH must not say it works
        for line in (flat(l) for l in self.root.text(skip_fills=True).split("\n")):
            low = line.lower()
            if "huddle" in low and "ssh" in low:
                self.assertNotRegex(low, r"\bworks?\b|\bsupported\b|\bfine\b", line)

    def test_every_section_has_a_jab(self):
        for sid in ("features", "numbers"):
            jabs = [e for e in by_id(self.root, sid).elements() if "jab" in e.classes]
            self.assertTrue(jabs, sid)
            for j in jabs:
                self.assertTrue(flat(j.text()), sid)
        # Get it lost its jab in the third review: the download is the point there
        self.assertFalse([e for e in by_id(self.root, "get").elements() if "jab" in e.classes])

    def jab(self, sid, needle):
        js = [e for e in by_id(self.root, sid).elements() if "jab" in e.classes and needle in e.text()]
        self.assertEqual(len(js), 1, needle)
        return js[0]

    def test_numbers_jab_counts_xp_memory(self):
        j = self.jab("numbers", "Windows XP")
        self.assertEqual(flat(j.text()), "Windows XP ran in 64 MB.")
        self.assertIs(j.parent, by_id(self.root, "numbers"))

    def test_huddles_jab_links_the_gaming_rig_to_the_numbers(self):
        j = self.jab("features", "gaming rig")
        self.assertEqual(flat(j.text()), "Because a messenger should not require a gaming rig.")
        links = [e for e in j.elements() if e.tag == "a"]
        self.assertEqual([(flat(a.text()), a.attrs.get("href")) for a in links], [("gaming rig", "#numbers")])

    def test_closer_keeps_sight_with_its_word(self):
        # at 390 px "SIGHT." must not sit alone on the last line
        raw = (SITE / "index.html").read_text()
        self.assertIn("Not a Chromium in&nbsp;sight.", raw)

    def test_no_grid_and_a_closer(self):
        f = by_id(self.root, "features")
        self.assertFalse([e for e in f.elements() if "everything" in e.classes or "rest" in e.classes])
        closer = [e for e in f.elements() if "closer" in e.classes]
        self.assertEqual(len(closer), 1)
        self.assertEqual(flat(closer[0].text()), "All of it in one terminal tab. Not a Chromium in sight.")
        self.assertIn("paper", closer[0].classes)  # a pasted strip with the collage's shadow
        self.assertIs(closer[0].parent, f)  # the section's last word, not inside a feature
        last = [e for e in f.children if e.kind == "el"][-1]
        self.assertIs(last, closer[0])

    def test_no_js_keeps_headline_pitch_and_download(self):
        # .text() drops <script>; links are checked outside any script element
        for s in ("SLACK,", "MINUS THE", "BULLSHIT.", "YOUR TERMINAL JUST LEARNED TO HUDDLE."):
            self.assertIn(s, self.all)
        hrefs = [e.attrs.get("href", "") for e in self.root.elements() if e.tag == "a"]
        self.assertTrue(any(h.endswith("-x86_64-linux.zip") for h in hrefs))
        dl = [e for e in self.root.elements() if e.tag == "a" and "dl" in e.classes]
        self.assertTrue(dl and dl[0].attrs["href"].startswith("https://github.com/dmytrobsp/klack/releases/download/"))

    def test_keyboard_sticker(self):
        f = by_id(self.root, "features")
        st = [e for e in f.elements() if "sticker" in e.classes and "Ctrl+Q" in e.text()]
        self.assertEqual(len(st), 1)
        self.assertEqual(flat(st[0].text()), "Quit with Ctrl+Q. We're not monsters.")
        art = st[0]
        while art.tag != "article":
            art = art.parent
        words = next(e for e in art.elements() if "words" in e.classes)
        self.assertFalse([e for e in words.elements() if e.tag == "kbd"])  # no documentation-style key list
        self.assertLess(len(flat(words.text())), 160, "short")

    def test_ctl_boasts_with_its_commands(self):
        f = flat(by_id(self.root, "features").text())
        for c in ("klack ctl", "whoami", "channels", "history", "post", "react", "upload", "search", "listen"):
            self.assertIn(c, f, c)
        cool = [e for e in by_id(self.root, "features").elements() if "cool-kids" in e.classes]
        self.assertEqual(len(cool), 1)
        self.assertIn("scrawl", cool[0].classes)

    def test_no_ssh_section(self):
        self.assertFalse([e for e in self.root.elements() if e.attrs.get("id") == "ssh"])
        raw = (SITE / "index.html").read_text()
        for s in ("ssh-shout", "graffiti", "sad-smiley", "capture-ssh"):
            self.assertNotIn(s, raw, s)
        self.assertFalse((SITE / "img" / "sad-smiley.svg").exists())
        self.assertFalse((SITE / "captures" / "ssh.html").exists())

    def test_numbers_section(self):
        n = flat(by_id(self.root, "numbers").text())
        self.assertIn("measured on Linux", n)
        self.assertNotRegex(n.lower(), r"workspace|\bteam\b")

    def test_get_section(self):
        g = flat(by_id(self.root, "get").text())
        for s in ("klack auth login", "SHA256SUMS", "quarantine", "SmartScreen", "Linux", "macOS", "Windows",
                  "screen sharing", "camera"):
            self.assertIn(s, g, s)

    def test_footer(self):
        f = by_id(self.root, "footer")
        t = flat(f.text())
        for s in ("MIT", "Apache-2.0", "not affiliated with Slack Technologies or Salesforce",
                  "Slack is their trademark", "WORKS ON MY MACHINE™", "CERTIFIED", "slk"):
            self.assertIn(s, t, s)
        self.assertNotIn("NO CHROMIUM", t)  # the hero has it; the footer has the stamp
        self.assertTrue([e for e in f.elements() if "stamp" in e.classes])
        self.assertIn("https://github.com/dmytrobsp/klack", [e.attrs.get("href") for e in f.elements()])

    def test_drafts_marked_and_state_no_figures(self):
        drafts = []
        for parent in [self.root, *self.root.elements()]:
            kids = [c for c in parent.children if not (c.kind == "text" and not c.data.strip())]
            for i, c in enumerate(kids):
                if c.kind == "comment" and c.data.strip() == "draft":
                    self.assertTrue(i + 1 < len(kids) and kids[i + 1].kind == "el", "draft marks no element")
                    drafts.append(flat(kids[i + 1].text()))
        self.assertGreater(len(drafts), 10)
        for d in drafts:
            self.assertTrue(d)
            self.assertNotRegex(d.replace("Apache-2.0", ""), r"\d", d)  # figures live in the spec sheet only


class DeadCss(unittest.TestCase):
    def test_no_rules_for_markup_that_is_gone(self):
        css = (SITE / "css" / "site.css").read_text()
        html = (SITE / "index.html").read_text()
        self.assertNotIn("<kbd", html)
        self.assertNotRegex(css, r"\bkbd\b")
        self.assertNotRegex(css, r"\.jab (s|em)\b")
        self.assertNotIn("#get .jab", css)


if __name__ == "__main__":
    unittest.main()

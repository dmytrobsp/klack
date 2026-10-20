"""The page's words: approved lines verbatim, jabs only at the official app, honest claims (LP5)."""
import re
import unittest
import xml.dom.minidom
from html.parser import HTMLParser
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SITE = ROOT / "site"
README = (ROOT / "README.md").read_text()

APPROVED = ["SLACK,", "MINUS THE", "BULLSHIT.", "BROWSER.", "YOUR TERMINAL JUST LEARNED TO HUDDLE.",
            "Yes, really.", "NO CHROMIUM WAS HARMED", "Quit with Ctrl+Q. We're not monsters.",
            "Chat over SSH: yes. Huddle over SSH: only if you shout really loud.",
            "SSH forwards ports, not vocal cords."]
BANNED = ["other terminal clients", "other clients", "the others can't"]

# The "everything else" grid: each item and a phrase that says so in README.md.
GRID = [
    ("threads", "every thread you are in"),
    ("reactions", "react · edit · delete"),
    ("custom emoji", "custom emoji as pictures"),
    ("pictures in the terminal", "kitty, WezTerm, Ghostty, iTerm2, foot or any sixel terminal"),
    ("video with sound", "video with\nsound"),
    ("files", "upload a file"),
    ("search", "search messages"),
    ("notifications", "desktop notifications"),
    ("do not disturb", "pause notifications for 30 minutes"),
    ("several workspaces", "Alt+1 … Alt+9"),
    ("themes", "dark, light, terminal and your own themes"),
    ("links into your Slack", "A plain click opens it in klack"),
    ("drafts", "Drafts also survive quitting"),
    ("slash commands", "run by Slack, as its own clients do"),
    ("your keys", "Rebinding keys"),
]

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
        ssh = by_id(self.root, "ssh").text(skip_fills=True)  # captures (Task 7) show klack's own UI hints
        found = 0
        for s in sentences(ssh):
            if "huddle" in s.lower():
                found += 1
                self.assertRegex(s.lower(), r"shout|vocal", s)
                self.assertNotRegex(s.lower(), r"\bworks?\b|\bsupported\b|\bfine\b", s)
        self.assertGreater(found, 0)

    def test_every_section_has_a_jab(self):
        for sid in ("features", "numbers", "ssh", "get"):
            jabs = [e for e in by_id(self.root, sid).elements() if "jab" in e.classes]
            self.assertTrue(jabs, sid)
            for j in jabs:
                self.assertTrue(flat(j.text()), sid)

    def test_grid_lists_only_readme_features(self):
        for item, phrase in GRID:
            self.assertIn(phrase, README, item)
        grid = next(e for e in by_id(self.root, "features").elements() if "everything" in e.classes)
        items = []
        for li in (e for e in grid.elements() if e.tag == "li"):
            b = next(e for e in li.elements() if e.tag == "b")
            items.append(flat(b.text()))
        self.assertEqual(items, [i for i, _ in GRID])

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
        for k in ("NORMAL", "INSERT", "which-key", "Ctrl+U", "Ctrl+D"):
            self.assertIn(k, flat(f.text()), k)

    def test_ctl_boasts_with_its_commands(self):
        f = flat(by_id(self.root, "features").text())
        for c in ("klack ctl", "whoami", "channels", "history", "post", "react", "upload", "search", "listen"):
            self.assertIn(c, f, c)

    def test_ssh_section(self):
        ssh = by_id(self.root, "ssh")
        h2 = next(e for e in ssh.elements() if e.tag == "h2")
        self.assertEqual(flat(h2.text()), "Chat over SSH: yes. Huddle over SSH: only if you shout really loud.")
        smiley = [e for e in h2.parent.elements() if e.tag == "img" and e.attrs.get("src") == "img/sad-smiley.svg"]
        self.assertEqual(len(smiley), 1)
        self.assertTrue(smiley[0].attrs.get("alt"))
        text = flat(ssh.text(skip_fills=True))
        for s in ("klack auth login", "threads", "search", "code"):
            self.assertIn(s, text, s)
        svg = (SITE / "img" / "sad-smiley.svg").read_text()
        xml.dom.minidom.parseString(svg)  # well-formed
        self.assertNotRegex(svg, r"(https?:)?//(?!www\.w3\.org/)")

    def test_numbers_section(self):
        n = flat(by_id(self.root, "numbers").text())
        self.assertIn("Linux", n)
        self.assertIn("freedom", n)
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
                  "Slack is their trademark", "NO CHROMIUM WAS HARMED"):
            self.assertIn(s, t, s)
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


if __name__ == "__main__":
    unittest.main()

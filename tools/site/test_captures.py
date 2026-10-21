import re
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import captures as C

ROOT = Path(__file__).resolve().parents[2]
PIC = "\x1b[38:2:0:0:%dm" + "\U0010EEEE" * 3 + "\x1b[39m"


def screen(rows, cols, pics):
    """A capture with picture placeholder blocks: pics = [(id, row, col, h)] 3 cells wide."""
    lines = [[" "] * cols for _ in range(rows)]
    for pid, r, c, h in pics:
        for k in range(h):
            lines[r + k][c] = PIC % pid
            del lines[r + k][c + 1:c + 3]
    return "\n".join("".join(l) for l in lines)


class Build(unittest.TestCase):
    def test_huddle_gets_the_broadcast_and_no_camera(self):
        old = C.CAPTURES["huddle"]
        C.CAPTURES["huddle"] = dict(old, cols=12, rows=6)
        try:
            out = C.build("huddle", "D" + screen(6, 12, [(1, 1, 1, 3)])[1:], fixes={(0, 0): "H"})
        finally:
            C.CAPTURES["huddle"] = old
        self.assertTrue(out.startswith('<div class="term capture" style="--cols:12;--rows:6" role="img"'))
        tv = re.search(r'<div class="tile tv" style="--r:1;--c:1;--h:3;--w:3" aria-hidden="true">(.*?)</div>', out)
        self.assertTrue(tv, out)
        self.assertEqual(re.findall(r'src="captures/([^"]+)"', tv.group(1)), [C.SWAN])  # one still, no loop
        self.assertNotIn("tile.webp", out)  # the other person's camera is off
        self.assertNotIn("\U0010EEEE", out)

    def test_a_missing_or_extra_picture_is_an_error(self):
        old = C.CAPTURES["huddle"]
        C.CAPTURES["huddle"] = dict(old, cols=12, rows=6)
        try:
            with self.assertRaises(ValueError):  # no share
                C.build("huddle", "D" + screen(6, 12, [])[1:], fixes={(0, 0): "H"})
            with self.assertRaises(ValueError):  # a camera came back on
                C.build("huddle", "D" + screen(6, 12, [(1, 1, 1, 3), (2, 4, 7, 2)])[1:], fixes={(0, 0): "H"})
        finally:
            C.CAPTURES["huddle"] = old

    def test_a_capture_that_needs_fixes_fails_without_them(self):
        old = C.CAPTURES["huddle"]
        C.CAPTURES["huddle"] = dict(old, cols=12, rows=6)
        try:
            pics = "D" + screen(6, 12, [(1, 1, 1, 3)])[1:]
            for fixes in (None, {}):
                with self.assertRaises(ValueError):
                    C.build("huddle", pics, fixes=fixes)
            with self.assertRaises(ValueError):  # a fix on a blank cell: the layout moved
                C.build("huddle", pics, fixes={(3, 3): "H"})
        finally:
            C.CAPTURES["huddle"] = old

    def test_fixes_and_renames(self):
        old = C.CAPTURES["keys"]
        C.CAPTURES["keys"] = dict(old, cols=10, rows=1)
        try:
            out = C.build("keys", "\x1b[1mReal\x1b[0m D", {"Real": "Fake"}, {(0, 5): "H"})
        finally:
            C.CAPTURES["keys"] = old
        self.assertNotIn("Real", out)
        self.assertIn("Fake", out)
        self.assertIn(">H<", out)
        self.assertNotIn(">D<", out)

    def test_one_swan_on_the_site_and_small(self):
        p = ROOT / "site/captures" / C.SWAN
        self.assertTrue(p.is_file())
        self.assertLessEqual(p.stat().st_size, 60_000)
        pictures = sorted(q.name for q in (ROOT / "site/captures").glob("*.webp"))
        self.assertEqual(pictures, [C.SWAN])  # the loop's other pictures and the camera's pattern are gone

    def test_the_tv_does_not_move(self):
        css = (ROOT / "site/css/site.css").read_text()
        tv = "\n".join(l for l in css.splitlines() if ".tv" in l)
        self.assertTrue(tv)
        self.assertNotIn("animation", tv)
        self.assertNotIn("tv-swan", css)
        self.assertNotIn("tv.js", (ROOT / "site/index.html").read_text())
        self.assertFalse((ROOT / "site/js/tv.js").exists())

    def test_the_page_credits_the_photo(self):
        page = (ROOT / "site/index.html").read_text()
        foot = page[page.index('<footer id="footer">'):]
        self.assertIn("Paata Vardanashvili", foot)
        self.assertIn('href="https://creativecommons.org/licenses/by/2.0/"', foot)
        self.assertIn('href="https://commons.wikimedia.org/wiki/File:Swanlake010.jpg"', foot)
        for s in ("Swan Lake ballet", "(cropped, greyscale)"):  # CC BY: say what was changed
            self.assertIn(s, foot, s)
        for s in ("public domain", "Mikhail Bocharov", "Henrich Levogt", "Legnani", "stamp of 1970"):
            self.assertNotIn(s, foot, s)  # the pictures that were removed are not credited


if __name__ == "__main__":
    unittest.main()

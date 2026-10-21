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
    def test_huddle_gets_the_broadcast_and_the_camera(self):
        old = C.CAPTURES["huddle"]
        C.CAPTURES["huddle"] = dict(old, cols=12, rows=6)
        try:
            out = C.build("huddle", screen(6, 12, [(1, 1, 1, 3), (2, 4, 7, 2)]))
        finally:
            C.CAPTURES["huddle"] = old
        self.assertTrue(out.startswith('<div class="term capture" style="--cols:12;--rows:6" role="img"'))
        tv = re.search(r'<div class="tile tv" style="--r:1;--c:1;--h:3;--w:3" aria-hidden="true">(.*?)</div>', out)
        self.assertTrue(tv, out)
        self.assertEqual(re.findall(r'src="captures/([^"]+)"', tv.group(1)), C.SWANS)
        self.assertIn('<img class="tile" src="captures/tile.webp" alt="" style="--r:4;--c:7;--h:2;--w:3">', out)
        self.assertNotIn("\U0010EEEE", out)

    def test_a_missing_picture_is_an_error(self):
        old = C.CAPTURES["huddle"]
        C.CAPTURES["huddle"] = dict(old, cols=12, rows=6)
        try:
            with self.assertRaises(ValueError):
                C.build("huddle", screen(6, 12, [(1, 1, 1, 3)]))
        finally:
            C.CAPTURES["huddle"] = old

    def test_fixes_and_renames(self):
        old = C.CAPTURES["ssh"]
        C.CAPTURES["ssh"] = dict(old, cols=10, rows=1)
        try:
            out = C.build("ssh", "\x1b[1mReal\x1b[0m D", {"Real": "Fake"}, {(0, 5): "H"})
        finally:
            C.CAPTURES["ssh"] = old
        self.assertNotIn("Real", out)
        self.assertIn("Fake", out)
        self.assertIn(">H<", out)
        self.assertNotIn(">D<", out)

    def test_the_swans_are_on_the_site_and_small(self):
        for n in C.SWANS:
            p = ROOT / "site/captures" / n
            self.assertTrue(p.is_file(), n)
            self.assertLessEqual(p.stat().st_size, 60_000, n)

    def test_the_page_credits_the_photo(self):
        page = (ROOT / "site/index.html").read_text()
        foot = page[page.index('<footer id="footer">'):]
        self.assertIn("Paata Vardanashvili", foot)
        self.assertIn('href="https://creativecommons.org/licenses/by/2.0/"', foot)
        self.assertIn('href="https://commons.wikimedia.org/wiki/File:Swanlake010.jpg"', foot)
        self.assertIn("public domain", foot)


if __name__ == "__main__":
    unittest.main()

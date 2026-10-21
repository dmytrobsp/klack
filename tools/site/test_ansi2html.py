import re
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from ansi2html import (CH, CW, KITTY, _Ink, block_parts, box_arms, char_width, colours, convert, palette, parse,
                       render_svg, rename_cells)


def svg(ansi, cols=10, rows=None, rename=None):
    return convert(ansi, rename, cols, rows)[0]


def paths(out, cls):
    """{fill colour: [rects (x0, y0, x1, y1)]} of the <path class=cls> elements."""
    res = {}
    for fill, d in re.findall(rf'<path class="{cls}" fill="([^"]+)" d="([^"]+)"', out):
        rects = re.findall(r"M([\d.]+) ([\d.]+)H([\d.]+)V([\d.]+)H[\d.]+Z", d)
        res.setdefault(fill, []).extend(tuple(float(v) for v in r) for r in rects)
    return res


def text_of(out):
    return re.sub(r"<[^>]+>", "", "".join(re.findall(r"<text[^>]*>.*?</text>", out)))


class Colours(unittest.TestCase):
    def cell(self, ansi, c=0):
        return parse(ansi, 10).cells[0][c]

    def test_bold_red(self):
        st = self.cell("\x1b[1;31mx\x1b[0m")[1]
        self.assertTrue(st[2])
        self.assertEqual(st[0], KITTY["16"][1])
        self.assertIn('font-weight="700"', svg("\x1b[1;31mx"))

    def test_rgb_semicolon_and_colon(self):
        for a in ("\x1b[38;2;236;178;46mx", "\x1b[38:2:236:178:46mx", "\x1b[38:2::236:178:46mx"):
            self.assertEqual(self.cell(a)[1][0], "#ecb22e", a)
            self.assertIn('fill="#ecb22e"', svg(a))
        self.assertEqual(self.cell("\x1b[48:2:1:2:3mx")[1][1], "#010203")

    def test_256(self):
        self.assertEqual(self.cell("\x1b[38;5;70mx")[1][0], "#5faf00")
        self.assertEqual(self.cell("\x1b[38:5:70mx")[1][0], "#5faf00")
        self.assertEqual(self.cell("\x1b[38;5;255mx")[1][0], "#eeeeee")
        self.assertEqual(self.cell("\x1b[48;5;1mx")[1][1], KITTY["16"][1])
        self.assertEqual(palette()[16], KITTY["more"][16])

    def test_bright(self):
        self.assertEqual(self.cell("\x1b[91mx")[1][0], KITTY["16"][9])
        self.assertEqual(self.cell("\x1b[101mx")[1][1], KITTY["16"][9])

    def test_reverse_swaps_and_dim_fades(self):
        fg, bg = colours(self.cell("\x1b[31;42;7mx")[1])
        self.assertEqual((fg, bg), (KITTY["16"][2], KITTY["16"][1]))
        fg, _ = colours(self.cell("\x1b[2;38:2:255:255:255mx")[1])
        self.assertEqual(fg, "#787882")  # 40 % of the way from #1e1e2e to white, as kitty dims

    def test_attrs_and_resets(self):
        st = self.cell("\x1b[3;4;9;2mx\x1b[23;24;29;22my", 0)[1]
        self.assertEqual(st[2:7], (False, True, True, True, True))
        self.assertEqual(self.cell("\x1b[3;4;9;2mx\x1b[23;24;29;22my", 1)[1], self.cell("y")[1])
        self.assertFalse(self.cell("\x1b[4:0mx")[1][5])
        self.assertTrue(self.cell("\x1b[4:3mx")[1][5])

    def test_underline_colour_skipped(self):
        st = self.cell("\x1b[58;2;1;2;3;1mx")[1]
        self.assertTrue(st[2])
        self.assertFalse(st[3])


class Escapes(unittest.TestCase):
    def test_escape(self):
        self.assertIn("&lt;&amp;&gt;", svg("<&>"))

    def test_osc8_dropped(self):
        out = svg("\x1b]8;;https://team.slack.com/archives/C1\x1b\\label\x1b]8;;\x1b\\", 20)
        self.assertIn("label", out)
        self.assertNotIn("slack.com", out)
        self.assertNotIn("\x1b", out)
        self.assertEqual(text_of(svg("\x1b]0;title\x07hi")), "hi")

    def test_other_csi_dropped(self):
        self.assertEqual(text_of(svg("a\x1b[?25lb")), "ab")

    def test_string_sequences_dropped(self):
        for esc in ("\x1b_Gf=100;SECRETPAYLOAD\x1b\\", "\x1bPqSECRET\x1b\\", "\x1b^pm\x1b\\", "\x1bXsos\x1b\\"):
            out = svg("a" + esc + "b")
            self.assertEqual(text_of(out), "ab")
            self.assertNotIn("SECRET", out)
        self.assertEqual(text_of(svg("a\x1b\\b")), "ab")

    def test_kitty_fixture(self):
        raw = (Path(__file__).parent / "fixtures" / "kitty-sample.ansi").read_text()
        out = svg(raw, 60)
        self.assertIn("bold red", out)
        self.assertIn('fill="#ecb22e"', out)
        self.assertIn("link", out)
        self.assertNotIn("example.com", out)
        self.assertNotIn("\x1b", out)
        self.assertIn("#010203", paths(out, "bg"))


class Grid(unittest.TestCase):
    def test_every_cell_of_every_row(self):
        s = parse("ab\n\x1b[44m  \x1b[0m", 6, 3)
        self.assertEqual((s.rows, s.cols), (3, 6))
        self.assertTrue(all(len(r) == 6 for r in s.cells))
        self.assertEqual(s.cells[1][1][1][1], KITTY["16"][4], "styled trailing blanks are kept")
        out = render_svg(s)
        self.assertIn(f'viewBox="0 0 {6 * CW} {3 * CH}"', out)

    def test_backgrounds_are_whole_cells_and_rows_solid(self):
        out = svg("\x1b[48:2:1:2:3mab  \n\x1b[48:2:1:2:3mcd", 8, 2)
        rects = paths(out, "bg")["#010203"]
        # two rows of four and two cells, merged: one rect a row, full height, touching rows
        self.assertEqual(sorted(rects), [(0, 0, 4 * CW, CH), (0, CH, 2 * CW, 2 * CH)])
        self.assertEqual(out.count('class="bg"'), 1, "one path a colour")

    def test_erase_to_the_edge_paints_the_background(self):
        s = parse("ab\x1b[48:2:1:2:3m\x1b[Kc", 6)
        self.assertEqual("".join(c[0] for c in s.cells[0]), "abc   ")
        self.assertEqual([c[1][1] for c in s.cells[0]], [None, None] + ["#010203"] * 4)
        self.assertEqual(parse("abc\x1b[2K", 4).cells[0][0][0], " ")

    def test_wide_characters_take_two_cells(self):
        self.assertEqual(char_width("🚀"), 2)
        self.assertEqual(char_width("界"), 2)
        self.assertEqual(char_width("a"), 1)
        self.assertEqual(char_width("́"), 0)
        s = parse("🚀x│", 5)
        self.assertEqual([c[2] for c in s.cells[0]], [2, 0, 1, 1, 1])
        self.assertEqual(s.cells[0][2][0], "x")
        s = parse("🖥️x", 4)  # a narrow emoji made wide by VS16
        self.assertEqual([c[2] for c in s.cells[0]], [2, 0, 1, 1])
        out = render_svg(parse("🚀x", 4))
        self.assertIn(f'x="{CW}" text-anchor="middle">🚀', out, "centred on its two cells")
        self.assertIn(f'x="{2 * CW}"', out)

    def test_text_runs_are_pinned_to_their_cells(self):
        out = render_svg(parse("ok  14 · go", 12))
        self.assertIn('<tspan x="0" textLength="16">ok</tspan>', out)
        self.assertIn(f'x="{4 * CW}" textLength="{2 * CW}">14</tspan>', out)
        self.assertIn(f'x="{7 * CW + CW / 2:g}" text-anchor="middle">·</tspan>', out)

    def test_long_lines_continue(self):
        s = parse("abcdef", 4, 2)
        self.assertEqual((s.text(0), s.text(1)), ("abcd", "ef  "))

    def test_pictures_are_blanked_and_boxed(self):
        p = "\U0010EEEE̅̅"
        row = f"\x1b[38:2:0:0:7m{p}\U0010EEEE\U0010EEEE\x1b[39m."
        s = parse(f".{row}\n.{row}", 6, 2)
        self.assertEqual(s.pictures, [{"r": 0, "c": 1, "h": 2, "w": 3}])
        self.assertEqual(s.text(0), ".   . ")
        self.assertNotIn("\U0010EEEE", render_svg(s))


class Geometry(unittest.TestCase):
    def test_box_arms_from_names(self):
        self.assertEqual(box_arms("─"), {"l": 1, "r": 1})
        self.assertEqual(box_arms("┃"), {"u": 2, "d": 2})
        self.assertEqual(box_arms("┌"), {"d": 1, "r": 1})
        self.assertEqual(box_arms("╞"), {"u": 1, "d": 1, "r": 3})
        self.assertEqual(box_arms("╿"), {"u": 2, "d": 1})
        self.assertIsNone(box_arms("╭"))

    def test_box_drawing_is_geometry_not_text(self):
        out = svg("┌─┐│x│", 3, 2)
        self.assertEqual(text_of(out), "x")
        self.assertIn('class="ink"', out)

    def test_lines_join_across_cells(self):
        out = svg("\x1b[38:2:9:9:9m────", 4)
        rects = paths(out, "ink")["#090909"]
        self.assertEqual(len(rects), 1, rects)  # one continuous line
        x0, y0, x1, y1 = rects[0]
        self.assertEqual((x0, x1), (0, 4 * CW))
        self.assertAlmostEqual((y0 + y1) / 2, CH / 2)

    def test_lines_join_across_rows(self):
        out = svg("\x1b[38:2:9:9:9m│\n│\n│", 1, 3)
        rects = paths(out, "ink")["#090909"]
        self.assertEqual(len(rects), 1, rects)
        self.assertEqual((rects[0][1], rects[0][3]), (0, 3 * CH))

    def test_a_box_is_closed(self):
        out = svg("\x1b[38:2:9:9:9m┌─┐\n│ │\n└─┘", 3, 3)
        rects = paths(out, "ink")["#090909"]
        ink = set()
        for x0, y0, x1, y1 in rects:  # sample the centre line of every border cell
            for c in range(3):
                for r in range(3):
                    cx, cy = c * CW + CW / 2, r * CH + CH / 2
                    if x0 <= cx <= x1 and y0 <= cy <= y1:
                        ink.add((r, c))
        self.assertEqual(ink, {(r, c) for r in range(3) for c in range(3)} - {(1, 1)})
        # the top line runs from corner to corner in one piece, through the corners' centres
        self.assertTrue(any(y0 < CH / 2 < y1 and x0 < CW / 2 and x1 > 2 * CW + CW / 2 for x0, y0, x1, y1 in rects))

    def test_heavy_is_thicker(self):
        light = paths(svg("\x1b[38:2:9:9:9m│", 1), "ink")["#090909"][0]
        heavy = paths(svg("\x1b[38:2:9:9:9m┃", 1), "ink")["#090909"][0]
        self.assertGreater(heavy[2] - heavy[0], light[2] - light[0])

    def test_blocks_tile_their_cell(self):
        for ch in "▀▄█▌▐▎▏▍░▔▕▖▙▚":
            area = sum((x1 - x0) * (y1 - y0) for (x0, y0, x1, y1), _ in block_parts(ch))
            self.assertAlmostEqual(area, 1, msg=ch)

    def test_half_blocks_draw_both_halves(self):
        out = svg("\x1b[38:2:0:0:0;48:2:255:255:255m▀\x1b[38:2:255:255:255;48:2:0:0:0m▀", 2)
        p = paths(out, "bg")
        self.assertEqual(sorted(p["#000000"]), [(0, 0, CW, CH / 2), (CW, CH / 2, 2 * CW, CH)])
        self.assertEqual(sorted(p["#ffffff"]), [(0, CH / 2, CW, CH), (CW, 0, 2 * CW, CH / 2)])
        self.assertEqual(text_of(out), "")

    def test_merge(self):
        self.assertEqual(_Ink.merged([(0, 0, 1, 1), (1, 0, 2, 1), (0, 1, 2, 2)]), [(0, 0, 2, 2)])


class Rename(unittest.TestCase):
    def test_rename_across_runs_keeps_each_cells_style(self):
        s = parse("\x1b[1;48:2:1:2:3mAS\x1b[0m \x1b[38:2:9:9:9mAlice Smith\x1b[0m!", 20)
        rename_cells(s, {"AS Alice Smith": "BJ Bobby Jones"})
        self.assertEqual(s.text(0).rstrip(), "BJ Bobby Jones!")
        styles = [c[1] for c in s.cells[0]]
        self.assertEqual(styles[0][1], "#010203")   # the initials keep their background
        self.assertEqual(styles[1][1], "#010203")
        self.assertIsNone(styles[2][1])              # the gap doesn't get it
        self.assertEqual(styles[3][0], "#090909")    # the name keeps its colour, no background
        self.assertIsNone(styles[3][1])
        out = render_svg(s)
        self.assertNotIn("Alice", out)
        self.assertEqual(paths(out, "bg")["#010203"], [(0, 0, 2 * CW, CH)])

    def test_rename_pads_and_cuts_to_keep_the_layout(self):
        s = rename_cells(parse("[Alice]", 8), {"Alice": "Bo"})
        self.assertEqual(s.text(0), "[Bo   ] ")
        s = rename_cells(parse("[Al]", 5), {"Al": "Bobby"})
        self.assertEqual(s.text(0), "[Bo] ")

    def test_rename_after_a_wide_character(self):
        s = rename_cells(parse("🔒secret-room│", 14), {"secret-room": "public-hall"})
        self.assertEqual(s.text(0), "🔒public-hall│")
        self.assertEqual(s.cells[0][-1][0], "│")


if __name__ == "__main__":
    unittest.main()

import sys, unittest
from pathlib import Path
sys.path.insert(0, str(Path(__file__).parent))
from ansi2html import convert


class Convert(unittest.TestCase):
    def test_bold_red(self):
        out = convert("\x1b[1;31mx\x1b[0m")
        self.assertTrue(out.startswith('<pre class="term">'))
        self.assertIn("font-weight:700", out)
        self.assertRegex(out, r"color:#[0-9a-f]{6}")

    def test_rgb_semicolon_and_colon(self):
        self.assertIn("color:#ecb22e", convert("\x1b[38;2;236;178;46mx"))
        self.assertIn("color:#ecb22e", convert("\x1b[38:2:236:178:46mx"))
        self.assertIn("color:#ecb22e", convert("\x1b[38:2::236:178:46mx"))
        self.assertIn("background:#010203", convert("\x1b[48:2:1:2:3mx"))

    def test_256(self):
        self.assertIn("color:#5faf00", convert("\x1b[38;5;70mx"))
        self.assertIn("color:#5faf00", convert("\x1b[38:5:70mx"))
        self.assertIn("color:#eeeeee", convert("\x1b[38;5;255mx"))
        self.assertIn("background:#800000", convert("\x1b[48;5;1mx"))

    def test_bright(self):
        self.assertRegex(convert("\x1b[91mx"), r"color:#[0-9a-f]{6}")
        self.assertRegex(convert("\x1b[101mx"), r"background:#[0-9a-f]{6}")

    def test_reverse_swaps(self):
        out = convert("\x1b[31;42;7mx")
        a = convert("\x1b[31mx")
        b = convert("\x1b[42mx")
        fg = a.split("color:")[1][:7]
        bg = b.split("background:")[1][:7]
        self.assertIn("color:" + bg, out)
        self.assertIn("background:" + fg, out)

    def test_attrs_and_resets(self):
        out = convert("\x1b[3;4;9;2mx\x1b[23;24;29;22my")
        self.assertIn("font-style:italic", out)
        self.assertIn("underline", out)
        self.assertIn("line-through", out)
        self.assertIn("opacity", out)
        self.assertEqual(convert("\x1b[1mx\x1b[22my").count("<span"), 1)
        self.assertEqual(convert("\x1b[31mx\x1b[39my").count("<span"), 1)

    def test_default_has_no_style(self):
        self.assertEqual(convert("plain"), '<pre class="term">plain</pre>')
        self.assertEqual(convert("\x1b[0mplain"), '<pre class="term">plain</pre>')

    def test_escape(self):
        self.assertIn("&lt;&amp;&gt;", convert("<&>"))

    def test_osc8_dropped(self):
        out = convert("\x1b]8;;https://team.slack.com/archives/C1\x1b\\label\x1b]8;;\x1b\\")
        self.assertIn("label", out)
        self.assertNotIn("slack.com", out)
        self.assertNotIn("\x1b", out)
        out = convert("\x1b]0;title\x07hi")
        self.assertTrue(out.endswith(">hi</pre>"))

    def test_other_csi_dropped(self):
        self.assertEqual(convert("a\x1b[2Kb\x1b[?25l"), '<pre class="term">ab</pre>')

    def test_string_sequences_dropped(self):
        for esc in ("\x1b_Gf=100;SECRETPAYLOAD\x1b\\", "\x1bPqSECRET\x1b\\",
                    "\x1b^pm\x1b\\", "\x1bXsos\x1b\\"):
            self.assertEqual(convert("a" + esc + "b"), '<pre class="term">ab</pre>')
        self.assertEqual(convert("a\x1b\\b"), '<pre class="term">ab</pre>')

    def test_rename_across_runs(self):
        out = convert("\x1b[1mAlice \x1b[0;36mSmith\x1b[0m", {"Alice Smith": "Bob"})
        self.assertNotIn("Alice", out)
        self.assertNotIn("Smith", out)
        self.assertIn("Bob", out)
        self.assertIn("font-weight:700", out)

    def test_trim(self):
        out = convert("a  \r\nb   \n\n  \n")
        self.assertEqual(out, '<pre class="term">a\nb</pre>')

    def test_kitty_fixture(self):
        raw = (Path(__file__).parent / "fixtures" / "kitty-sample.ansi").read_text()
        out = convert(raw)
        self.assertIn("bold red", out)
        self.assertIn("color:#ecb22e", out)
        self.assertIn("link", out)
        self.assertNotIn("example.com", out)
        self.assertNotIn("\x1b", out)
        self.assertIn("background:#010203", out)


if __name__ == "__main__":
    unittest.main()

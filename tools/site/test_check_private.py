import contextlib, io, os, subprocess, sys, tempfile, unittest
from pathlib import Path
HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
from check_private import scan, main


class Scan(unittest.TestCase):
    def setUp(self):
        self.d = tempfile.TemporaryDirectory()
        self.root = Path(self.d.name)

    def tearDown(self):
        self.d.cleanup()

    def test_finds_variants(self):
        (self.root / "a.html").write_text("ok\nby alice.smith here\nALICE_SMITH\nAlice-Smith\n")
        hits = scan(self.root, ["Alice Smith"])
        self.assertEqual([(h[1], h[2]) for h in hits], [(2, 0), (3, 0), (4, 0)])
        self.assertTrue(hits[0][0].endswith("a.html"))

    def test_clean_and_index(self):
        (self.root / "a.txt").write_text("nothing here\nBob Jones\n")
        self.assertEqual(scan(self.root, ["Alice Smith"]), [])
        self.assertEqual(scan(self.root, ["Alice Smith", "bob jones"])[0][2], 1)

    def test_skips_binary(self):
        (self.root / "f.woff2").write_bytes(b"alice smith")
        (self.root / "g.dat").write_bytes(b"\x00alice smith")
        self.assertEqual(scan(self.root, ["Alice Smith"]), [])

    def test_blank_forbidden_ignored(self):
        (self.root / "a.txt").write_text("x\n")
        self.assertEqual(scan(self.root, ["", " . "]), [])


class Cli(unittest.TestCase):
    def run_cli(self, listfile, text):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "site"
            root.mkdir()
            (root / "i.html").write_text(text)
            env = dict(os.environ, KLACK_SITE_FORBIDDEN=str(Path(d) / listfile))
            if listfile == "list.txt":
                (Path(d) / listfile).write_text("# c\n\nAlice Smith\nzed.q\n")
            return subprocess.run([sys.executable, str(HERE / "check_private.py"), str(root)],
                                  capture_output=True, text=True, env=env)

    def test_hit_never_prints_string(self):
        r = self.run_cli("list.txt", "hello\nAlice_Smith\n")
        self.assertEqual(r.returncode, 1)
        out = (r.stdout + r.stderr).lower()
        self.assertIn("i.html:2: forbidden string #1", out)
        self.assertNotIn("alice", out)
        self.assertNotIn("smith", out)

    def test_clean(self):
        self.assertEqual(self.run_cli("list.txt", "hello\n").returncode, 0)

    def test_missing_list(self):
        r = self.run_cli("nope.txt", "hello\n")
        self.assertEqual(r.returncode, 2)
        self.assertIn("nope.txt", r.stderr + r.stdout)


if __name__ == "__main__":
    unittest.main()

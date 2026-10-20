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


class Hardening(unittest.TestCase):
    def test_markup_between_words(self):
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "a.html").write_text("<b>Alice</b> <span>Smith</span>\nAlice&nbsp;Smith\nAlice&#32;Smith\n")
            hits = scan(d, ["Alice Smith"])
            self.assertEqual([h[1] for h in hits], [1, 2, 3])

    def test_filename_hit_hidden(self):
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "alice_smith.txt").write_text("x")
            hits = scan(d, ["Alice Smith"])
            self.assertEqual(hits, [("<name hidden>", 0, 0)])

    def test_relative_paths(self):
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "sub").mkdir()
            (Path(d) / "sub" / "a.txt").write_text("Alice Smith")
            self.assertEqual(scan(d, ["Alice Smith"]), [("sub/a.txt", 1, 0)])


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

    def run_raw(self, make):
        with tempfile.TemporaryDirectory() as d:
            lst = Path(d) / "list.txt"
            lst.write_text("Alice Smith\n")
            root = make(Path(d))
            env = dict(os.environ, KLACK_SITE_FORBIDDEN=str(lst))
            return subprocess.run([sys.executable, str(HERE / "check_private.py"), str(root)],
                                  capture_output=True, text=True, env=env)

    def test_missing_root(self):
        self.assertEqual(self.run_raw(lambda d: d / "nope").returncode, 2)

    def test_nothing_scanned(self):
        def mk(d):
            (d / "e").mkdir()
            (d / "e" / "f.woff2").write_bytes(b"x")
            return d / "e"
        self.assertEqual(self.run_raw(mk).returncode, 2)

    def test_filename_hit_not_printed(self):
        def mk(d):
            (d / "s").mkdir()
            (d / "s" / "alice.smith.html").write_text("ok")
            return d / "s"
        r = self.run_raw(mk)
        self.assertEqual(r.returncode, 1)
        self.assertIn("<name hidden>:0: forbidden string #1", r.stdout)
        self.assertNotIn("alice", (r.stdout + r.stderr).lower())

    def test_bad_list_encoding(self):
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "s").mkdir(); (Path(d) / "s" / "a.txt").write_text("x")
            (Path(d) / "l").write_bytes(b"\xff\xfe\x80")
            env = dict(os.environ, KLACK_SITE_FORBIDDEN=str(Path(d) / "l"))
            r = subprocess.run([sys.executable, str(HERE / "check_private.py"), str(Path(d) / "s")],
                               capture_output=True, text=True, env=env)
            self.assertEqual(r.returncode, 2)

    def test_missing_list(self):
        r = self.run_cli("nope.txt", "hello\n")
        self.assertEqual(r.returncode, 2)
        self.assertIn("nope.txt", r.stderr + r.stdout)


if __name__ == "__main__":
    unittest.main()

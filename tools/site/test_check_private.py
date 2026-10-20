import contextlib, io, os, subprocess, sys, tempfile, unittest
from pathlib import Path
HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
from check_private import scan, main, owner_from_remote, allowed_forms


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


class RepoAddress(unittest.TestCase):
    """The repository's own public address is allowed; the owner's name anywhere else is not (made-up owner)."""
    LIST = ["alicesmith", "Alice"]

    def hits(self, text, owner="alicesmith"):
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "a.html").write_text(text)
            return sorted({h[1] for h in scan(d, self.LIST, allow=allowed_forms(owner) if owner else ())})

    def test_the_repo_address_passes(self):
        text = ("https://github.com/alicesmith/klack/releases/download/v1.0.0/klack-1.0.0-x86_64-linux.zip\n"
                "<a href=\"https://github.com/alicesmith/klack\">github.com/alicesmith/klack</a>\n"
                "https://GitHub.com/AliceSmith/klack/issues?q=x#top\n"
                "https://alicesmith.github.io/klack/\nalicesmith.github.io\n")
        self.assertEqual(self.hits(text), [])

    def test_the_owner_elsewhere_is_a_hit(self):
        text = ("by alicesmith\nhttps://github.com/alicesmith/other\ngithub.com/alicesmith/klackfoo\n"
                "https://github.com/alicesmith/klack and Alice\nxalicesmith.github.io\n"
                "alicesmith.github.io.evil.example\n")
        self.assertEqual(self.hits(text), [1, 2, 3, 4, 5, 6])

    def test_nothing_is_allowed_without_an_owner(self):
        self.assertEqual(self.hits("https://github.com/alicesmith/klack\n", owner=None), [1])

    def test_removing_the_address_joins_nothing(self):
        # "Ali" + address + "ce" must not become "Alice" once the address is taken out
        self.assertEqual(self.hits("Ali https://github.com/alicesmith/klack ce\n"), [])
        self.assertEqual(self.hits("Ali github.com/alicesmith/klack ce\n"), [])

    def test_owner_from_remote(self):
        for url in ("git@github.com:alicesmith/klack.git", "https://github.com/alicesmith/klack.git",
                    "https://github.com/alicesmith/klack", "ssh://git@github.com/alicesmith/klack.git\n"):
            self.assertEqual(owner_from_remote(url), "alicesmith", url)
        for url in (None, "", "https://gitlab.com/alicesmith/klack.git", "/srv/git/klack.git"):
            self.assertIsNone(owner_from_remote(url), url)

    def run_main(self, text, argv=(), remote=None):
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "s").mkdir()
            (Path(d) / "s" / "i.html").write_text(text)
            (Path(d) / "l").write_text("\n".join(self.LIST) + "\n")
            out = io.StringIO()
            old = os.environ.get("KLACK_SITE_FORBIDDEN")
            os.environ["KLACK_SITE_FORBIDDEN"] = str(Path(d) / "l")
            try:
                with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
                    code = main([str(Path(d) / "s"), *argv], remote=lambda: remote)
            finally:
                if old is None:
                    os.environ.pop("KLACK_SITE_FORBIDDEN")
                else:
                    os.environ["KLACK_SITE_FORBIDDEN"] = old
            return code, out.getvalue()

    def test_cli_reads_the_owner_from_origin(self):
        code, out = self.run_main("https://github.com/alicesmith/klack\n", remote="git@github.com:alicesmith/klack.git")
        self.assertEqual(code, 0)
        code, out = self.run_main("alicesmith\n", remote="git@github.com:alicesmith/klack.git")
        self.assertEqual(code, 1)
        self.assertNotIn("alicesmith", out.lower())

    def test_cli_falls_back_to_allow_owner(self):
        self.assertEqual(self.run_main("https://github.com/alicesmith/klack\n", ["--allow-owner", "alicesmith"])[0], 0)
        self.assertEqual(self.run_main("https://github.com/alicesmith/klack\n")[0], 1)
        code, out = self.run_main("x\n", ["--allow-owner"])
        self.assertEqual(code, 2)


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

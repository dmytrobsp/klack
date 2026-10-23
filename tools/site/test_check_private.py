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
        text = ("https://github.com/alicesmith/ttyak/releases/download/v1.0.0/ttyak-1.0.0-x86_64-linux.zip\n"
                "<a href=\"https://github.com/alicesmith/ttyak\">github.com/alicesmith/ttyak</a>\n"
                "https://GitHub.com/AliceSmith/ttyak/issues?q=x#top\n"
                "https://alicesmith.github.io/ttyak/\nalicesmith.github.io\n")
        self.assertEqual(self.hits(text), [])

    def test_the_owner_elsewhere_is_a_hit(self):
        text = ("by alicesmith\nhttps://github.com/alicesmith/other\ngithub.com/alicesmith/ttyakfoo\n"
                "https://github.com/alicesmith/ttyak and Alice\nxalicesmith.github.io\n"
                "alicesmith.github.io.evil.example\n")
        self.assertEqual(self.hits(text), [1, 2, 3, 4, 5, 6])

    def test_what_follows_the_address_is_still_scanned(self):
        text = ("https://github.com/alicesmith/ttyak/tree/Alice\n"
                "https://github.com/alicesmith/ttyak?x=alicesmith\n"
                "https://github.com/alicesmith/ttyak#alicesmith\n"
                "<a href=\"https://github.com/alicesmith/ttyak/issues/1&amp;by=alicesmith\">x</a>\n"
                "https://github.com/alicesmith/ttyak/blob/main/Alice%20Smith.md\n"
                "https://alicesmith.github.io/ttyak/Alice\n")
        self.assertEqual(self.hits(text), [1, 2, 3, 4, 5, 6])

    def test_the_site_address_passes(self):
        # without its dot "ttyak.sh" runs into other words ("ttyaksh"): the address itself is allowed, owner or not
        text = ("https://ttyak.sh/latest.json\n<a href=\"https://ttyak.sh\">ttyak.sh</a>\nttyak.shx\n"
                "https://ttyak.sh/Yaksh\n")
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "a.html").write_text(text)
            self.assertEqual(sorted({h[1] for h in scan(d, ["Yaksh"], allow=allowed_forms(None))}), [3, 4])

    def test_the_package_names_pass(self):
        # the install commands name the owner too: the Homebrew tap, the Scoop bucket, the winget id, `gh -R`
        text = ("brew install alicesmith/ttyak/ttyak\n"
                "scoop bucket add ttyak https://github.com/alicesmith/scoop-ttyak\n"
                "https://github.com/alicesmith/homebrew-ttyak\n"
                "winget install alicesmith.ttyak\n"
                "gh attestation verify ttyak-1.0.0-x86_64-linux.zip -R alicesmith/ttyak\n"
                "<code>winget upgrade alicesmith.ttyak</code>\n")
        self.assertEqual(self.hits(text), [])

    def test_names_like_the_package_names_are_hits(self):
        text = ("brew install alicesmith/other\nalicesmith.ttyakx\nalicesmith/ttyakfoo\n"
                "github.com/alicesmith/scoop-other\nxalicesmith/ttyak\nalicesmith.ttyak.evil.example\n"
                "brew install alicesmith/ttyak/Alice\nwinget install xalicesmith.ttyak\n")
        self.assertEqual(self.hits(text), [1, 2, 3, 4, 5, 6, 7, 8])

    def test_the_package_names_need_an_owner(self):
        self.assertEqual(self.hits("brew install alicesmith/ttyak/ttyak\nalicesmith.ttyak\n", owner=None), [1, 2])

    def test_the_old_repo_address_is_not_allowed(self):
        self.assertEqual(self.hits("https://github.com/alicesmith/" + "k" + "lack\n"), [1])

    def test_nothing_is_allowed_without_an_owner(self):
        self.assertEqual(self.hits("https://github.com/alicesmith/ttyak\n", owner=None), [1])

    def test_removing_the_address_joins_nothing(self):
        # "Ali" + address + "ce" must not become "Alice" once the address is taken out
        self.assertEqual(self.hits("Ali https://github.com/alicesmith/ttyak ce\n"), [])
        self.assertEqual(self.hits("Ali github.com/alicesmith/ttyak ce\n"), [])

    def test_owner_from_remote(self):
        for url in ("git@github.com:alicesmith/ttyak.git", "https://github.com/alicesmith/ttyak.git",
                    "https://github.com/alicesmith/ttyak", "ssh://git@github.com/alicesmith/ttyak.git\n"):
            self.assertEqual(owner_from_remote(url), "alicesmith", url)
        for url in (None, "", "https://gitlab.com/alicesmith/ttyak.git", "/srv/git/ttyak.git"):
            self.assertIsNone(owner_from_remote(url), url)

    def run_main(self, text, argv=(), remote=None):
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "s").mkdir()
            (Path(d) / "s" / "i.html").write_text(text)
            (Path(d) / "l").write_text("\n".join(self.LIST) + "\n")
            out = io.StringIO()
            old = os.environ.get("TTYAK_SITE_FORBIDDEN")
            os.environ["TTYAK_SITE_FORBIDDEN"] = str(Path(d) / "l")
            try:
                with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
                    code = main([str(Path(d) / "s"), *argv], remote=lambda: remote)
            finally:
                if old is None:
                    os.environ.pop("TTYAK_SITE_FORBIDDEN")
                else:
                    os.environ["TTYAK_SITE_FORBIDDEN"] = old
            return code, out.getvalue()

    def test_cli_reads_the_owner_from_origin(self):
        code, out = self.run_main("https://github.com/alicesmith/ttyak\n", remote="git@github.com:alicesmith/ttyak.git")
        self.assertEqual(code, 0)
        code, out = self.run_main("alicesmith\n", remote="git@github.com:alicesmith/ttyak.git")
        self.assertEqual(code, 1)
        self.assertNotIn("alicesmith", out.lower())

    def test_cli_falls_back_to_allow_owner(self):
        self.assertEqual(self.run_main("https://github.com/alicesmith/ttyak\n", ["--allow-owner", "alicesmith"])[0], 0)
        self.assertEqual(self.run_main("https://github.com/alicesmith/ttyak\n")[0], 1)
        code, out = self.run_main("x\n", ["--allow-owner"])
        self.assertEqual(code, 2)


class Cli(unittest.TestCase):
    def run_cli(self, listfile, text):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "site"
            root.mkdir()
            (root / "i.html").write_text(text)
            env = dict(os.environ, TTYAK_SITE_FORBIDDEN=str(Path(d) / listfile))
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
            env = dict(os.environ, TTYAK_SITE_FORBIDDEN=str(lst))
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
            env = dict(os.environ, TTYAK_SITE_FORBIDDEN=str(Path(d) / "l"))
            r = subprocess.run([sys.executable, str(HERE / "check_private.py"), str(Path(d) / "s")],
                               capture_output=True, text=True, env=env)
            self.assertEqual(r.returncode, 2)

    def test_the_list_lives_in_the_ttyak_config(self):
        with tempfile.TemporaryDirectory() as d:
            home = Path(d)
            (home / "site").mkdir()
            (home / "site" / "i.html").write_text("Alice Smith\n")
            env = {k: v for k, v in os.environ.items() if k != "TTYAK_SITE_FORBIDDEN"}
            env["HOME"] = str(home)
            run = lambda: subprocess.run([sys.executable, str(HERE / "check_private.py"), str(home / "site")],
                                         capture_output=True, text=True, env=env)
            r = run()
            self.assertEqual(r.returncode, 2)
            self.assertIn(str(home / ".config/ttyak-site/forbidden.txt"), r.stderr)
            (home / ".config/ttyak-site").mkdir(parents=True)
            (home / ".config/ttyak-site/forbidden.txt").write_text("Alice Smith\n")
            self.assertEqual(run().returncode, 1)

    def test_missing_list(self):
        r = self.run_cli("nope.txt", "hello\n")
        self.assertEqual(r.returncode, 2)
        self.assertIn("nope.txt", r.stderr + r.stdout)


if __name__ == "__main__":
    unittest.main()

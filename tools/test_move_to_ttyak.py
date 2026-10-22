"""Tests for tools/move-to-ttyak.sh, each in its own temporary HOME."""
import os
import shutil
import stat
import subprocess
import tempfile
import unittest

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "move-to-ttyak.sh")
SHELLS = ["/usr/bin/dash", "/usr/bin/bash"]


class MoveToTtyak(unittest.TestCase):
    def setUp(self):
        self.new_home()

    def new_home(self):
        self.home = tempfile.mkdtemp(prefix="move-to-ttyak-test-")
        self.bin = os.path.join(self.home, "fake-bin")
        os.mkdir(self.bin)
        self.fake_pgrep("none")

    def tearDown(self):
        shutil.rmtree(self.home)

    def fake_pgrep(self, mode):
        """mode: none (nothing runs), all, ttyak (only ttyak runs), error (exit 2)."""
        body = {
            "none": "exit 1\n",
            "all": "echo 4242\nexit 0\n",
            "ttyak": 'for last; do :; done\n[ "$last" = ttyak ] && { echo 4242; exit 0; }\nexit 1\n',
            "error": "echo oops >&2\nexit 2\n",
        }[mode]
        path = os.path.join(self.bin, "pgrep")
        with open(path, "w") as f:
            f.write("#!/bin/sh\n" + body)
        os.chmod(path, 0o755)

    def p(self, *parts):
        return os.path.join(self.home, *parts)

    def write(self, rel, text="x", mode=None):
        path = self.p(rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as f:
            f.write(text)
        if mode is not None:
            os.chmod(path, mode)

    def read(self, rel):
        with open(self.p(rel)) as f:
            return f.read()

    def make_old(self):
        self.write(".config/klack/default/session.json", "s", 0o600)
        self.write(".config/klack/settings.toml", "a = 1\n")
        self.write(".cache/klack/default/x", "c")
        self.write(".local/share/klack/data", "d")
        self.write(".config/klack-site/site.toml", "t")

    def run_script(self, shell, *args, env=None):
        if env is None:
            env = {"HOME": self.home}
        env = {**env, "PATH": self.bin + ":/usr/bin:/bin"}
        r = subprocess.run([shell, SCRIPT, *args], env=env, capture_output=True, text=True)
        return r.returncode, r.stdout + r.stderr

    def each_shell(self, body):
        for shell in SHELLS:
            with self.subTest(shell=shell):
                shutil.rmtree(self.home)
                self.new_home()
                body(shell)

    def test_moves_every_folder(self):
        def body(shell):
            self.make_old()
            code, out = self.run_script(shell)
            self.assertEqual(code, 0, out)
            for old in [".config/klack", ".cache/klack", ".local/share/klack", ".config/klack-site"]:
                self.assertFalse(os.path.exists(self.p(old)), old)
            for new, rel in [(".config/ttyak", "default/session.json"), (".config/ttyak", "settings.toml"),
                             (".cache/ttyak", "default/x"), (".local/share/ttyak", "data"),
                             (".config/ttyak-site", "site.toml")]:
                self.assertTrue(os.path.isfile(self.p(new, rel)), (new, rel))
            self.assertEqual(self.read(".config/ttyak/settings.toml"), "a = 1\n")
            mode = stat.S_IMODE(os.stat(self.p(".config/ttyak/default/session.json")).st_mode)
            self.assertEqual(mode, 0o600)
        self.each_shell(body)

    def test_renames_logs(self):
        def body(shell):
            self.write(".cache/klack/a/klack.log", "log a")
            self.write(".cache/klack/b/klack.log", "old b")
            self.write(".cache/klack/b/ttyak.log", "new b")
            code, out = self.run_script(shell)
            self.assertEqual(code, 1, out)
            self.assertEqual(self.read(".cache/ttyak/a/ttyak.log"), "log a")
            self.assertFalse(os.path.exists(self.p(".cache/ttyak/a/klack.log")))
            self.assertEqual(self.read(".cache/ttyak/b/ttyak.log"), "new b")
            self.assertEqual(self.read(".cache/ttyak/b/klack.log"), "old b")
        self.each_shell(body)

    def test_never_overwrites(self):
        def body(shell):
            self.make_old()
            self.write(".config/ttyak/keep", "mine", 0o640)
            code, out = self.run_script(shell)
            self.assertEqual(code, 1, out)
            self.assertTrue(os.path.isfile(self.p(".config/klack/settings.toml")))
            self.assertEqual(os.listdir(self.p(".config/ttyak")), ["keep"])
            self.assertEqual(self.read(".config/ttyak/keep"), "mine")
            self.assertEqual(stat.S_IMODE(os.stat(self.p(".config/ttyak/keep")).st_mode), 0o640)
            self.assertFalse(os.path.exists(self.p(".cache/klack")))
            self.assertTrue(os.path.isdir(self.p(".cache/ttyak")))
            self.assertTrue(os.path.isdir(self.p(".local/share/ttyak")))
            self.assertTrue(os.path.isdir(self.p(".config/ttyak-site")))
        self.each_shell(body)

    def snapshot(self):
        seen = []
        for root, dirs, files in os.walk(self.home):
            for n in dirs + files:
                seen.append(os.path.relpath(os.path.join(root, n), self.home))
        return sorted(seen)

    def test_dry_run_moves_nothing(self):
        def body(shell):
            self.make_old()
            self.write(".cache/klack/a/klack.log", "l")
            before = self.snapshot()
            code, out = self.run_script(shell, "--dry-run")
            self.assertEqual(code, 0, out)
            self.assertEqual(self.snapshot(), before)
            self.assertIn("would move", out)
            self.assertIn(".config/ttyak-site", out)
            self.assertIn("ttyak.log", out)
        self.each_shell(body)

    def test_nothing_to_move_is_fine(self):
        def body(shell):
            code, out = self.run_script(shell)
            self.assertEqual(code, 0, out)
        self.each_shell(body)

    def test_refuses_while_klack_runs(self):
        def body(shell):
            self.make_old()
            self.fake_pgrep("all")
            before = self.snapshot()
            code, out = self.run_script(shell)
            self.assertEqual(code, 1, out)
            self.assertEqual(self.snapshot(), before)
            self.assertIn("running", out)
        self.each_shell(body)

    def test_refuses_while_ttyak_runs(self):
        def body(shell):
            self.make_old()
            self.fake_pgrep("ttyak")
            before = self.snapshot()
            code, out = self.run_script(shell)
            self.assertEqual(code, 1, out)
            self.assertEqual(self.snapshot(), before)
            self.assertIn("ttyak is running", out)
        self.each_shell(body)

    def test_stops_when_pgrep_cannot_tell(self):
        def body(shell):
            self.make_old()
            self.fake_pgrep("error")
            before = self.snapshot()
            code, out = self.run_script(shell)
            self.assertEqual(code, 1, out)
            self.assertEqual(self.snapshot(), before)
            self.assertIn("cannot tell", out)
        self.each_shell(body)

    def test_dangling_symlink_target_is_not_free(self):
        def body(shell):
            self.make_old()
            self.write(".cache/klack/a/klack.log", "l")
            os.symlink(self.p("nowhere"), self.p(".config/ttyak"))
            os.makedirs(self.p(".cache/klack/b"))
            os.symlink(self.p("nowhere"), self.p(".cache/klack/b/ttyak.log"))
            self.write(".cache/klack/b/klack.log", "old")
            code, out = self.run_script(shell)
            self.assertEqual(code, 1, out)
            self.assertTrue(os.path.isdir(self.p(".config/klack")))
            self.assertTrue(os.path.islink(self.p(".config/ttyak")))
            self.assertTrue(os.path.islink(self.p(".cache/ttyak/b/ttyak.log")))
            self.assertEqual(self.read(".cache/ttyak/b/klack.log"), "old")
        self.each_shell(body)

    def test_target_that_is_a_file_is_refused(self):
        def body(shell):
            self.make_old()
            self.write(".local/share/ttyak", "a file")
            code, out = self.run_script(shell)
            self.assertEqual(code, 1, out)
            self.assertEqual(self.read(".local/share/ttyak"), "a file")
            self.assertTrue(os.path.isfile(self.p(".local/share/klack/data")))
            self.assertTrue(os.path.isdir(self.p(".config/ttyak")))
        self.each_shell(body)

    def test_existing_ttyak_cache_is_left_alone(self):
        def body(shell):
            self.write(".cache/ttyak/a/klack.log", "l")
            self.write(".cache/klack/a/klack.log", "l2")
            before = self.snapshot()
            code, out = self.run_script(shell)
            self.assertEqual(code, 1, out)
            self.assertEqual(self.snapshot(), before)
        self.each_shell(body)

    def test_bad_home(self):
        def body(shell):
            for env in [{}, {"HOME": ""}, {"HOME": self.p("no-such-dir")}]:
                code, out = self.run_script(shell, env=env)
                self.assertEqual(code, 1, (env, out))
            self.write("afile", "x")
            code, out = self.run_script(shell, env={"HOME": self.p("afile")})
            self.assertEqual(code, 1, out)
        self.each_shell(body)

    def test_bad_arguments(self):
        def body(shell):
            self.make_old()
            before = self.snapshot()
            for args in [("--force",), ("--dry-run", "--dry-run"), ("",)]:
                code, out = self.run_script(shell, *args)
                self.assertEqual(code, 1, (args, out))
                self.assertEqual(self.snapshot(), before)
        self.each_shell(body)

    def test_reports_old_names(self):
        def body(shell):
            self.write(".config/klack/settings.toml", "log = 'klack'\nsecret = 'hunter2'\n")
            self.write(".config/klack/keys.toml", "quit = 'q'\n")
            self.write(".config/fish/conf.d/k.fish", "set -x KLACK_LOG 1\n")
            self.write(".config/fish/conf.d/other.fish", "set -x FOO 1\n")
            self.write(".bashrc", "export PATH=$PATH\n")
            code, out = self.run_script(shell)
            self.assertEqual(code, 0, out)
            self.assertIn("check by hand", out)
            self.assertIn(self.p(".config/ttyak/settings.toml"), out)
            self.assertNotIn("keys.toml", out)
            self.assertIn(self.p(".config/fish/conf.d/k.fish"), out)
            self.assertNotIn("other.fish", out)
            self.assertNotIn(".bashrc", out)
            self.assertNotIn("hunter2", out)
            self.assertEqual(self.read(".config/fish/conf.d/k.fish"), "set -x KLACK_LOG 1\n")
        self.each_shell(body)


if __name__ == "__main__":
    unittest.main()

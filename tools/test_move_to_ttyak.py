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
        self.home = tempfile.mkdtemp(prefix="move-to-ttyak-test-")
        self.bin = os.path.join(self.home, "fake-bin")
        os.mkdir(self.bin)
        self.fake_pgrep(running=False)

    def tearDown(self):
        shutil.rmtree(self.home)

    def fake_pgrep(self, running):
        path = os.path.join(self.bin, "pgrep")
        with open(path, "w") as f:
            f.write("#!/bin/sh\n" + ("echo 4242\nexit 0\n" if running else "exit 1\n"))
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

    def run_script(self, shell, *args):
        env = {"HOME": self.home, "PATH": self.bin + ":/usr/bin:/bin"}
        r = subprocess.run([shell, SCRIPT, *args], env=env, capture_output=True, text=True)
        return r.returncode, r.stdout + r.stderr

    def each_shell(self, body):
        for shell in SHELLS:
            with self.subTest(shell=shell):
                shutil.rmtree(self.home)
                self.setUp_again()
                body(shell)

    def setUp_again(self):
        self.home = tempfile.mkdtemp(prefix="move-to-ttyak-test-")
        self.bin = os.path.join(self.home, "fake-bin")
        os.mkdir(self.bin)
        self.fake_pgrep(running=False)

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
            self.assertEqual(code, 0, out)
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
            self.fake_pgrep(running=True)
            before = self.snapshot()
            code, out = self.run_script(shell)
            self.assertEqual(code, 1, out)
            self.assertEqual(self.snapshot(), before)
            self.assertIn("running", out)
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

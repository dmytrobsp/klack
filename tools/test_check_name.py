"""Tests for tools/check-name.sh, each in its own temporary git repository."""
import os
import shutil
import subprocess
import tempfile
import unittest

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "check-name.sh")
SHELLS = [s for s in (shutil.which("dash"), shutil.which("bash")) if s]
# The old name, built so that this file does not carry it.
OLD = "k" + "lack"


class CheckName(unittest.TestCase):
    def setUp(self):
        self.repo = tempfile.mkdtemp(prefix="check-name-test-")
        self.git("init", "-q")

    def tearDown(self):
        shutil.rmtree(self.repo)

    def git(self, *args):
        subprocess.run(["git", *args], cwd=self.repo, check=True, capture_output=True)

    def write(self, rel, text, track=True):
        path = os.path.join(self.repo, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as f:
            f.write(text)
        if track:
            self.git("add", "--", rel)

    def run_guard(self, *paths):
        """Runs the guard under each shell; returns (exit code, stdout), the same for both."""
        results = []
        for shell in SHELLS:
            r = subprocess.run([shell, SCRIPT, *paths], cwd=self.repo, capture_output=True, text=True)
            results.append((r.returncode, r.stdout))
        self.assertEqual(results[0], results[1])
        return results[0]

    def test_old_name_in_any_case_fails_with_file_and_line(self):
        self.write("src/a.rs", "fine\nlet x = \"%s\";\n" % OLD.capitalize())
        self.write("src/b.rs", "ok\nok\n%s\n" % OLD.upper())
        code, out = self.run_guard("src")
        self.assertEqual(code, 1)
        self.assertIn("src/a.rs:2:", out)
        self.assertIn("src/b.rs:3:", out)

    def test_clean_files_pass(self):
        self.write("src/a.rs", "ttyak\n")
        self.assertEqual(self.run_guard("src"), (0, ""))

    def test_allowlisted_files_pass(self):
        self.write("PROGRESS.md", "%s\n" % OLD)
        self.write("docs/superpowers/x.md", "%s\n" % OLD)
        self.write("tools/move-to-ttyak.sh", "%s\n" % OLD)
        self.write("tools/check-name.sh", "%s\n" % OLD)
        self.assertEqual(self.run_guard("."), (0, ""))

    def test_allowlist_is_exact(self):
        self.write("docs/PROGRESS.md", "%s\n" % OLD)
        self.write("docs/superpowers-x.md", "%s\n" % OLD)
        code, out = self.run_guard(".")
        self.assertEqual(code, 1)
        self.assertIn("docs/PROGRESS.md:1:", out)
        self.assertIn("docs/superpowers-x.md:1:", out)

    def test_marked_line_passes_unmarked_line_fails(self):
        self.write("site/fill.py", "a = '%s-'  # old-name-ok\nb = '%s-'\n" % (OLD, OLD))
        code, out = self.run_guard("site")
        self.assertEqual(code, 1)
        self.assertNotIn("site/fill.py:1:", out)
        self.assertIn("site/fill.py:2:", out)

    def test_marker_in_the_path_does_not_excuse_a_line(self):
        self.write("src/old-name-ok.rs", "x = '%s'\n" % OLD)
        code, out = self.run_guard("src")
        self.assertEqual(code, 1)
        self.assertIn("src/old-name-ok.rs:1:", out)

    def test_untracked_files_are_ignored(self):
        self.write("src/a.rs", "ttyak\n")
        self.write("src/new.rs", "%s\n" % OLD, track=False)
        self.assertEqual(self.run_guard("src"), (0, ""))

    def test_only_the_given_paths_are_checked(self):
        self.write("src/a.rs", "ttyak\n")
        self.write("other/b.rs", "%s\n" % OLD)
        self.assertEqual(self.run_guard("src"), (0, ""))

    def test_old_name_in_a_file_name_fails(self):
        self.write("vendor/%s-PATCH.md" % OLD.upper(), "notes\n")
        code, out = self.run_guard("vendor")
        self.assertEqual(code, 1)
        self.assertIn("vendor/%s-PATCH.md" % OLD.upper(), out)

    def test_binary_files_are_skipped(self):
        path = os.path.join(self.repo, "bin.dat")
        with open(path, "wb") as f:
            f.write(b"\x00\x01" + OLD.encode() + b"\x00")
        self.git("add", "bin.dat")
        self.assertEqual(self.run_guard("bin.dat"), (0, ""))

    def test_path_that_is_not_tracked_is_an_error(self):
        self.write("src/a.rs", "ttyak\n")
        code, _ = self.run_guard("src", "srcc")
        self.assertEqual(code, 2)

    def test_no_paths_is_an_error(self):
        code, _ = self.run_guard()
        self.assertEqual(code, 2)


if __name__ == "__main__":
    unittest.main()

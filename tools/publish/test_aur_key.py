"""Tests for tools/publish/aur-key.sh: a temporary TMPDIR and a stub gh that keeps what `secret set` got on stdin."""
import os
import shutil
import subprocess
import tempfile
import unittest

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "aur-key.sh")

# Logs each call's arguments; `secret set` writes its stdin to FAKE_GH_STDIN (fails with FAKE_GH_SET_FAIL);
# `secret list --json name --jq .[].name` prints FAKE_GH_SECRETS (comma-separated; fails with FAKE_GH_LIST_FAIL);
# `api repos/…` prints FAKE_GH_ADMIN (default true); `auth status` fails with FAKE_GH_AUTH_FAIL.
FAKE_GH = r'''#!/usr/bin/env python3
import os, sys
args = sys.argv[1:]
with open(os.environ["FAKE_GH_LOG"], "a") as log:
    log.write(" ".join(args) + "\n")
env = os.environ.get
if args[:2] == ["auth", "status"]:
    sys.exit(1 if env("FAKE_GH_AUTH_FAIL") else 0)
if args == ["api", "repos/dmytrobsp/ttyak", "--jq", ".permissions.admin"]:
    print(env("FAKE_GH_ADMIN", "true"))
    sys.exit(0)
if args == ["secret", "list", "-R", "dmytrobsp/ttyak", "--json", "name", "--jq", ".[].name"]:
    if env("FAKE_GH_LIST_FAIL"):
        sys.stderr.write("HTTP 403: Resource not accessible\n")
        sys.exit(1)
    for name in filter(None, env("FAKE_GH_SECRETS", "").split(",")):
        print(name)
    sys.exit(0)
if args == ["secret", "set", "AUR_SSH_KEY", "-R", "dmytrobsp/ttyak"]:
    data = sys.stdin.buffer.read()
    if env("FAKE_GH_SET_FAIL"):
        sys.stderr.write("HTTP 404: Not Found\n")
        sys.exit(1)
    with open(env("FAKE_GH_STDIN"), "wb") as f:
        f.write(data)
    print("✓ Set Actions secret AUR_SSH_KEY for dmytrobsp/ttyak")
    sys.exit(0)
sys.stderr.write("fake gh: unexpected call %r\n" % args)
sys.exit(3)
'''


class AurKey(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="aur-key-test-")
        self.tmpdir = os.path.join(self.tmp, "tmp")
        bin_dir = os.path.join(self.tmp, "bin")
        os.mkdir(self.tmpdir)
        os.mkdir(bin_dir)
        gh = os.path.join(bin_dir, "gh")
        with open(gh, "w") as f:
            f.write(FAKE_GH)
        os.chmod(gh, 0o755)
        self.log = os.path.join(self.tmp, "gh.log")
        self.stdin = os.path.join(self.tmp, "gh.stdin")
        self.env = {k: v for k, v in os.environ.items() if k not in ("GH_TOKEN", "GITHUB_TOKEN", "GH_HOST", "SSH_AUTH_SOCK")}
        self.env.update(PATH=bin_dir + os.pathsep + os.environ["PATH"], TMPDIR=self.tmpdir, FAKE_GH_LOG=self.log,
                        FAKE_GH_STDIN=self.stdin)

    def tearDown(self):
        shutil.rmtree(self.tmp)

    def run_script(self, *args, **env):
        e = dict(self.env, **env)
        return subprocess.run(["bash", SCRIPT, *args], env=e, capture_output=True, text=True, stdin=subprocess.DEVNULL)

    def calls(self):
        if not os.path.exists(self.log):
            return []
        with open(self.log) as f:
            return f.read().splitlines()

    def sets(self):
        return [c for c in self.calls() if c.startswith("secret set")]

    def received(self):
        with open(self.stdin, "rb") as f:
            return f.read()

    def public_of(self, private):
        """The public key line ssh-keygen derives from the private key gh got."""
        path = os.path.join(self.tmp, "got")
        with open(os.open(path, os.O_WRONLY | os.O_CREAT, 0o600), "wb") as f:
            f.write(private)
        r = subprocess.run(["ssh-keygen", "-y", "-P", "", "-f", path], capture_output=True, text=True, check=True,
                           stdin=subprocess.DEVNULL)
        os.remove(path)
        return r.stdout.strip()

    def test_the_private_key_reaches_gh_and_only_the_public_key_is_printed(self):
        r = self.run_script()
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(self.sets(), ["secret set AUR_SSH_KEY -R dmytrobsp/ttyak"])
        private = self.received()
        self.assertTrue(private.startswith(b"-----BEGIN OPENSSH PRIVATE KEY-----\n"), private[:40])
        # an ed25519 key without a passphrase (the workflow reads it with -P '')
        pub = self.public_of(private)
        self.assertTrue(pub.startswith("ssh-ed25519 "), pub)
        lines = r.stdout.splitlines()
        self.assertEqual(len(lines), 1, r.stdout)
        self.assertEqual(lines[0].split()[:2], pub.split()[:2])
        body = private.decode().splitlines()[1]
        for out in (r.stdout, r.stderr):
            self.assertNotIn("PRIVATE KEY", out)
            self.assertNotIn(body, out)
        self.assertNotIn("Set Actions secret", r.stdout)

    def test_the_private_file_is_gone_afterwards(self):
        for env in ({}, {"FAKE_GH_SET_FAIL": "1"}):
            with self.subTest(**env):
                r = self.run_script(**env)
                self.assertEqual(r.returncode, 1 if env else 0, r.stderr)
                self.assertEqual(os.listdir(self.tmpdir), [])
        self.assertEqual(r.stdout, "", "a failed set prints no public key")
        self.assertIn("gh secret set AUR_SSH_KEY failed", r.stderr)

    def test_an_existing_secret_is_not_overwritten_without_force(self):
        r = self.run_script(FAKE_GH_SECRETS="APPLE_P12,AUR_SSH_KEY")
        self.assertEqual(r.returncode, 1)
        self.assertEqual(self.sets(), [])
        self.assertEqual(r.stdout, "")
        self.assertIn("--force", r.stderr)
        self.assertEqual(os.listdir(self.tmpdir), [])
        r = self.run_script("--force", FAKE_GH_SECRETS="APPLE_P12,AUR_SSH_KEY")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(self.sets(), ["secret set AUR_SSH_KEY -R dmytrobsp/ttyak"])
        self.assertTrue(r.stdout.startswith("ssh-ed25519 "))

    def test_other_secrets_do_not_count(self):
        r = self.run_script(FAKE_GH_SECRETS="AUR_SSH_KEY_OLD,PUBLISH_TOKEN")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(len(self.sets()), 1)

    def test_refuses_before_making_a_key(self):
        cases = {
            "gh is not logged in": {"FAKE_GH_AUTH_FAIL": "1"},
            "not an admin": {"FAKE_GH_ADMIN": "false"},
            "the secrets cannot be listed": {"FAKE_GH_LIST_FAIL": "1"},
        }
        for why, env in cases.items():
            with self.subTest(why):
                r = self.run_script(**env)
                self.assertEqual(r.returncode, 1, why)
                self.assertEqual(r.stdout, "")
                self.assertEqual(self.sets(), [])
                self.assertEqual(os.listdir(self.tmpdir), [])

    def test_usage(self):
        r = self.run_script("--forse")
        self.assertEqual(r.returncode, 2)
        self.assertIn("usage:", r.stderr)
        self.assertEqual(self.calls(), [])


if __name__ == "__main__":
    unittest.main()

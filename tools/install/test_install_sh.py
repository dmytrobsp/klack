"""Tests for site/install.sh: a fake release served on 127.0.0.1, a temporary HOME per run, under dash and bash.

The fixtures (zips holding a `ttyak` shell stub, SHA256SUMS, latest.json) are built in a temporary folder at test
time. Each run gets a PATH of its own: links to the tools the script may use, minus the ones a test hides, plus fake
`uname`/`sysctl`/`id` where a test needs them."""
import functools
import hashlib
import http.server
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
SCRIPT = os.path.join(ROOT, "site", "install.sh")
SHELLS = [s for s in (shutil.which("dash"), shutil.which("bash")) if s]
MARKER = "# added by the ttyak installer"
LATEST = "0.10.0"
PLATFORMS = ["x86_64-linux", "aarch64-macos", "x86_64-macos"]
# version -> platforms with an archive; 6.6.6's SHA256SUMS lies about its Linux archive
RELEASES = {"0.9.0": PLATFORMS, "0.10.0": PLATFORMS, "0.11.0-rc.1": PLATFORMS + ["aarch64-linux", "x86_64-windows"],
            "6.6.6": PLATFORMS}
# The tools the script (and the stub) may run; a test hides some of them.
TOOLS = ["sh", "id", "mktemp", "rm", "mkdir", "chmod", "mv", "cp", "cat", "sed", "awk", "tr", "tail", "grep",
         "dirname", "curl", "wget", "unzip", "bsdtar", "python3", "sha256sum", "shasum"]
STUB = """#!/bin/sh
# ttyak stub for {platform}
case "${{1:-}}" in
--version) echo "ttyak {version}" ;;
uninstall)
\tshift
\there=$(dirname "$0")
\t{{ echo "args:$*"; if [ -t 0 ]; then echo stdin:tty; else echo stdin:other; fi; }} >"$here/uninstall-args"
\t;;
esac
"""


def sha256(path):
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


def build_fixtures(root):
    """The fake release tree under root: gh/releases/download/v<V>/…, latest.json and broken feeds."""
    for version, platforms in RELEASES.items():
        d = os.path.join(root, "gh", "releases", "download", "v" + version)
        os.makedirs(d)
        sums = []
        for platform in platforms:
            name = "ttyak-%s-%s.zip" % (version, platform)
            exe = "ttyak.exe" if platform.endswith("windows") else "ttyak"
            with zipfile.ZipFile(os.path.join(d, name), "w") as z:
                info = zipfile.ZipInfo(exe)
                info.external_attr = 0o755 << 16
                z.writestr(info, STUB.format(platform=platform, version=version))
                for extra in ("README.md", "LICENSE-MIT", "LICENSE-APACHE"):
                    z.writestr(extra, extra + "\n")
            digest = sha256(os.path.join(d, name))
            if version == "6.6.6" and platform == "x86_64-linux":
                digest = "0" * 64
            sums.append("%s  %s\n" % (digest, name))
        with open(os.path.join(d, "SHA256SUMS"), "w") as f:
            f.writelines(sums)
    feed = {"name": "ttyak", "version": LATEST, "date": "2026-10-23",
            "notes": "https://github.com/dmytrobsp/ttyak/releases/tag/v" + LATEST,
            "assets": {p: {"url": "https://example.invalid/ttyak-%s-%s.zip" % (LATEST, p), "sha256": "0" * 64}
                       for p in PLATFORMS}}
    with open(os.path.join(root, "latest.json"), "w") as f:
        json.dump(feed, f, indent=2)
    os.makedirs(os.path.join(root, "feeds"))
    broken = {
        "empty.json": "",
        "no-version.json": '{"name":"ttyak","date":"2026-10-23"}',
        "number.json": '{"name":"ttyak","version":10,"date":"2026-10-23"}',
        "html.json": "<html><body>404 Not Found</body></html>",
        "shell.json": '{"name":"ttyak","version":"0.10.0$(touch pwned)","date":"2026-10-23"}',
        "two-lines.json": '{"name":"ttyak","version":"0.10.0\\n1.0.0"}',
        "truncated.json": '{"name":"ttyak","vers',
    }
    for name, text in broken.items():
        with open(os.path.join(root, "feeds", name), "w") as f:
            f.write(text)


class Handler(http.server.SimpleHTTPRequestHandler):
    """Serves the fixture tree; /gh/releases/latest redirects as GitHub's does (to `redirect_tag`, 404 when None)."""
    redirect_tag = "v" + LATEST
    requests = []

    def log_message(self, *args):
        pass

    def _latest(self):
        Handler.requests.append(self.command + " " + self.path)
        if self.path != "/gh/releases/latest":
            return False
        if Handler.redirect_tag is None:
            self.send_error(404)
        else:
            self.send_response(302)
            self.send_header("Location", "https://github.com/dmytrobsp/ttyak/releases/tag/" + Handler.redirect_tag)
            self.send_header("Content-Length", "0")
            self.end_headers()
        return True

    def do_GET(self):
        if not self._latest():
            super().do_GET()

    def do_HEAD(self):
        if not self._latest():
            super().do_HEAD()


def setUpModule():
    global FIXTURES, SERVER, BASE
    FIXTURES = tempfile.mkdtemp(prefix="ttyak-install-fixtures-")
    build_fixtures(FIXTURES)
    SERVER = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Handler, directory=FIXTURES))
    threading.Thread(target=SERVER.serve_forever, daemon=True).start()
    BASE = "http://127.0.0.1:%d" % SERVER.server_address[1]


def tearDownModule():
    SERVER.shutdown()
    SERVER.server_close()
    shutil.rmtree(FIXTURES)


def slurp(path):
    with open(path) as f:
        return f.read()


def write_exe(path, text):
    """A fresh executable file at path: a link there is removed first, never written through."""
    if os.path.lexists(path):
        os.remove(path)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o755)
    with os.fdopen(fd, "w") as f:
        f.write(text)


class Run:
    """One run's folders: home, tmp, bin (the run's PATH)."""

    def __init__(self, test, shell, os_name="Linux", arch="x86_64", translated=None, root=False, hide=()):
        self.test, self.shell = test, shell
        self.top = tempfile.mkdtemp(prefix="ttyak-install-test-")
        test.addCleanup(shutil.rmtree, self.top)
        self.home = os.path.join(self.top, "home")
        self.tmp = os.path.join(self.top, "tmp")
        self.bin = os.path.join(self.top, "bin")
        for d in (self.home, self.tmp, self.bin):
            os.mkdir(d)
        for tool in TOOLS:
            real = shutil.which(tool)
            if real and tool not in hide:
                os.symlink(real, os.path.join(self.bin, tool))
        write_exe(os.path.join(self.bin, "uname"),
                  '#!/bin/sh\ncase "$1" in -s) echo %s ;; -m) echo %s ;; *) exit 1 ;; esac\n' % (os_name, arch))
        if translated is not None:
            write_exe(os.path.join(self.bin, "sysctl"),
                      '#!/bin/sh\n[ "$2" = sysctl.proc_translated ] && echo %d\n' % translated)
        if root:
            write_exe(os.path.join(self.bin, "id"),
                      '#!/bin/sh\nif [ "$1" = -u ]; then echo 0; else exec %s "$@"; fi\n' % shutil.which("id"))
        self.local_bin = os.path.join(self.home, ".local", "bin")
        self.env = {"HOME": self.home, "PATH": self.bin, "SHELL": "/bin/sh", "TMPDIR": self.tmp,
                    "TTYAK_LATEST_URL": BASE + "/latest.json", "TTYAK_DOWNLOAD_URL": BASE + "/gh/releases/download"}

    def argv(self, args):
        return [self.shell, SCRIPT, *args]

    def run(self, *args, **env):
        """Runs the script in a session of its own (no terminal), stdin closed."""
        Handler.requests.clear()
        e = dict(self.env, **env)
        p = subprocess.run(self.argv(args), env=e, stdin=subprocess.DEVNULL, capture_output=True, text=True,
                           timeout=60, start_new_session=True)
        self.test.assertEqual(os.listdir(self.tmp), [], "temporary files left behind")
        return p

    def ok(self, *args, **env):
        p = self.run(*args, **env)
        self.test.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        return p

    def fails(self, *args, **env):
        p = self.run(*args, **env)
        self.test.assertNotEqual(p.returncode, 0, p.stdout + p.stderr)
        return p

    def record(self, d=None):
        with open(os.path.join(d or self.local_bin, "install.json")) as f:
            return f.read()

    def installed(self, d=None):
        return subprocess.run([os.path.join(d or self.local_bin, "ttyak"), "--version"], capture_output=True,
                              text=True, env={"PATH": self.bin}).stdout.strip()

    def read(self, rel):
        with open(os.path.join(self.home, rel)) as f:
            return f.read()

    def with_tty(self, args, answer=b"", **env):
        """Runs the script in a new session whose controlling terminal is a pty (stdin stays a pipe); `answer` is
        typed on the terminal. Returns (exit code, stdout, stderr, what the terminal showed)."""
        import pty
        master, slave = pty.openpty()
        name = os.ttyname(slave)
        # Opening the pty in a new session makes it that session's controlling terminal.
        grab = "import os, sys; os.close(os.open(sys.argv[1], os.O_RDWR)); os.execv(sys.argv[2], sys.argv[2:])"
        if answer:
            os.write(master, answer)
        p = subprocess.Popen([sys.executable, "-c", grab, name, *self.argv(args)], env=dict(self.env, **env),
                             stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                             start_new_session=True)
        os.close(slave)
        out, err = p.communicate(timeout=30)
        shown = b""
        os.set_blocking(master, False)
        try:
            shown = os.read(master, 65536)
        except OSError:
            pass
        os.close(master)
        return p.returncode, out.decode(), err.decode(), shown.decode(errors="replace")


class InstallSh:
    """The tests, run once per shell (the classes at the end)."""
    shell = None

    def runs(self, **kw):
        """A fresh run (one, as a loop, so that each case reads as a block)."""
        yield Run(self, self.shell, **kw)

    def assertRecord(self, r, version, d=None):
        d = d or r.local_bin
        self.assertEqual(r.record(d), '{"method":"script","version":"%s","dir":%s}\n' % (version, json.dumps(d, ensure_ascii=False)))
        self.assertEqual(json.loads(r.record(d)), {"method": "script", "version": version, "dir": d})

    def test_installs_latest(self):
        for r in self.runs():
            p = r.ok()
            exe = os.path.join(r.local_bin, "ttyak")
            self.assertEqual(stat.S_IMODE(os.stat(exe).st_mode), 0o755)
            self.assertEqual(r.installed(), "ttyak " + LATEST)
            self.assertRecord(r, LATEST)
            self.assertEqual(sorted(os.listdir(r.local_bin)), ["install.json", "ttyak"])
            self.assertIn("Installed ttyak %s as %s" % (LATEST, exe), p.stdout)
            self.assertIn("GET /latest.json", Handler.requests)
            self.assertNotIn("HEAD /gh/releases/latest", Handler.requests)
            # the folders it made are this user's only
            self.assertEqual(stat.S_IMODE(os.stat(r.local_bin).st_mode), 0o700)
            self.assertEqual(p.stderr, "")

    def test_updates_in_place(self):
        for r in self.runs():
            r.ok("--version", "0.9.0")
            self.assertEqual(r.installed(), "ttyak 0.9.0")
            p = r.ok()
            self.assertEqual(r.installed(), "ttyak " + LATEST)
            self.assertRecord(r, LATEST)
            self.assertIn("Updated ttyak 0.9.0 to %s in %s" % (LATEST, r.local_bin), p.stdout)
            self.assertEqual(sorted(os.listdir(r.local_bin)), ["install.json", "ttyak"])
            p = r.ok()
            self.assertIn("Installed ttyak %s over the copy in %s" % (LATEST, r.local_bin), p.stdout)

    def test_version_flag_and_env(self):
        for r in self.runs():
            for args, env, want in ((["--version", "0.9.0"], {}, "0.9.0"), (["--version=0.11.0-rc.1"], {}, "0.11.0-rc.1"),
                                    ([], {"TTYAK_VERSION": "0.9.0"}, "0.9.0"), (["--version", "v0.11.0-rc.1"], {}, "0.11.0-rc.1"),
                                    (["--version", "0.9.0"], {"TTYAK_VERSION": "6.6.6"}, "0.9.0")):
                r.ok(*args, **env)
                self.assertEqual(r.installed(), "ttyak " + want, (args, env))
                self.assertRecord(r, want)
                self.assertNotIn("GET /latest.json", Handler.requests)
            for bad in ("1.2", "1.2.3;true", "../../etc", "1.2.3 4", "1.2.3\n4.5.6"):
                p = r.fails("--version", bad)
                self.assertIn("not a ttyak version", p.stderr)
            p = r.fails("--version", "9.9.9")
            self.assertIn("could not download", p.stderr)
            p = r.fails("--version")
            self.assertIn("--version needs a version", p.stderr)

    def test_dir_flag(self):
        for r in self.runs():
            target = os.path.join(r.top, "my tools", "it's here")
            p = r.ok("--dir", target)
            self.assertEqual(r.installed(target), "ttyak " + LATEST)
            self.assertRecord(r, LATEST, target)
            self.assertFalse(os.path.exists(r.local_bin))
            self.assertEqual(stat.S_IMODE(os.stat(target).st_mode), 0o700)
            profile = r.read(".profile")
            self.assertIn("export PATH='%s':\"$PATH\" %s\n" % (target.replace("'", "'\\''"), MARKER), profile)
            self.assertIn("added it in %s/.profile" % r.home, p.stdout)
            # the line works in a shell
            out = subprocess.run([r.shell, "-c", '. "$HOME/.profile"; printf %s "$PATH"'], env={"HOME": r.home, "PATH": "/usr/bin"},
                                 capture_output=True, text=True).stdout
            self.assertEqual(out, target + ":/usr/bin")
            # the env var, a relative folder and ~/
            other = os.path.join(r.top, "env-dir")
            r.ok(TTYAK_DIR=other)
            self.assertRecord(r, LATEST, other)
            subprocess.run(["mkdir", os.path.join(r.top, "cwd")], check=True)
            p = subprocess.run(r.argv(["--dir", "rel"]), env=r.env, cwd=os.path.join(r.top, "cwd"), capture_output=True,
                               text=True, stdin=subprocess.DEVNULL, timeout=60)
            self.assertEqual(p.returncode, 0, p.stderr)
            self.assertRecord(r, LATEST, os.path.join(r.top, "cwd", "rel"))
            r.ok("--dir=~/apps")
            self.assertRecord(r, LATEST, os.path.join(r.home, "apps"))
            odd = os.path.join(r.top, 'say "hi" \\ ünï')
            r.ok("--dir", odd)
            self.assertRecord(r, LATEST, odd)
            # a name JSON and the profiles cannot carry safely
            for bad in ("line\nbreak", "tab\there"):
                p = r.fails("--dir", os.path.join(r.top, bad))
                self.assertIn("control characters", p.stderr)
                self.assertFalse(os.path.exists(os.path.join(r.top, bad)))

    def test_dir_not_writable(self):
        for r in self.runs():
            ro = os.path.join(r.top, "ro")
            os.mkdir(ro, 0o500)
            r.test.addCleanup(os.chmod, ro, 0o700)
            p = r.fails("--dir", ro)
            self.assertIn("cannot write to " + ro, p.stderr)

    def test_refuses_bad_checksum(self):
        for r in self.runs():
            p = r.fails("--version", "6.6.6")
            self.assertIn("checksum mismatch for ttyak-6.6.6-x86_64-linux.zip", p.stderr)
            self.assertIn("nothing was installed", p.stderr)
            self.assertFalse(os.path.exists(r.local_bin))
            # an existing install stays as it was
            r.ok("--version", "0.9.0")
            r.fails("--version", "6.6.6")
            self.assertEqual(r.installed(), "ttyak 0.9.0")
            self.assertRecord(r, "0.9.0")

    def test_path_added_once(self):
        default_line = 'export PATH="$HOME/.local/bin:$PATH" ' + MARKER + "\n"
        fish_line = "fish_add_path --path ~/.local/bin " + MARKER + "\n"
        cases = (("/bin/bash", ".bashrc", default_line), ("/usr/bin/zsh", ".zshrc", default_line),
                 ("/usr/bin/fish", ".config/fish/conf.d/ttyak.fish", fish_line), ("/bin/sh", ".profile", default_line),
                 ("", ".profile", default_line), ("/usr/bin/ksh", ".profile", default_line))
        for shell_path, rel, line in cases:
            for r in self.runs():
                with open(os.path.join(r.home, ".profile"), "w") as f:
                    f.write("# mine, no newline at the end")
                with open(os.path.join(r.home, ".bashrc"), "w") as f:
                    f.write("alias ll='ls -l'\n")
                p = r.ok(SHELL=shell_path)
                self.assertIn("added it in %s/%s" % (r.home, rel), p.stdout)
                self.assertIn("Open a new terminal", p.stdout)
                text = r.read(rel)
                self.assertEqual(text.count(MARKER), 1, rel)
                self.assertTrue(text.endswith("\n" + line) or text == line, text)
                if rel == ".profile":
                    self.assertTrue(text.startswith("# mine, no newline at the end\n"))
                if rel == ".bashrc":
                    self.assertTrue(text.startswith("alias ll='ls -l'\n"))
                p = r.ok(SHELL=shell_path)
                self.assertEqual(r.read(rel), text)
                self.assertIn("already puts %s on PATH" % r.local_bin, p.stdout)
        # macOS bash reads ~/.bash_profile in its login shells
        for r in self.runs(os_name="Darwin", arch="arm64", translated=0):
            r.ok(SHELL="/bin/bash")
            self.assertEqual(r.read(".bash_profile"), default_line)
            self.assertFalse(os.path.exists(os.path.join(r.home, ".bashrc")))
        # ZDOTDIR and XDG_CONFIG_HOME are followed
        for r in self.runs():
            zdot, xdg = os.path.join(r.top, "zdot"), os.path.join(r.top, "xdg")
            r.ok(SHELL="/bin/zsh", ZDOTDIR=zdot)
            self.assertEqual(slurp(os.path.join(zdot, ".zshrc")), default_line)
            r.ok(SHELL="/usr/bin/fish", XDG_CONFIG_HOME=xdg)
            self.assertEqual(slurp(os.path.join(xdg, "fish", "conf.d", "ttyak.fish")), fish_line)
        # already on PATH: nothing is added anywhere
        for r in self.runs():
            for shell_path in ("/bin/bash", "/usr/bin/fish", "/bin/sh"):
                p = r.ok(SHELL=shell_path, PATH=r.local_bin + ":" + r.bin)
                self.assertIn("Run: ttyak", p.stdout)
            self.assertEqual(sorted(os.listdir(r.home)), [".local"])
        # fish quoting for another folder
        for r in self.runs():
            target = os.path.join(r.top, "a'b\\c")
            r.ok("--dir", target, SHELL="/usr/bin/fish")
            line = r.read(".config/fish/conf.d/ttyak.fish")
            self.assertEqual(line, "fish_add_path --path '%s' %s\n" % (target.replace("\\", "\\\\").replace("'", "\\'"), MARKER))

    @unittest.skipUnless(shutil.which("fish"), "needs fish")
    def test_fish_line_works(self):
        for r in self.runs():
            target = os.path.join(r.top, "a'b\\c d")
            r.ok("--dir", target, SHELL="/usr/bin/fish")
            conf = os.path.join(r.home, ".config", "fish", "conf.d", "ttyak.fish")
            out = subprocess.run(["fish", "--no-config", "-c", "source %s; string join \\n -- $PATH" % conf.replace(" ", "\\ ")],
                                 env={"HOME": r.home, "PATH": "/usr/bin"}, capture_output=True, text=True).stdout
            self.assertEqual(out.splitlines()[0], target)

    def test_no_tty_never_waits(self):
        setsid = shutil.which("setsid")
        for r in self.runs():
            def run(*args, **env):
                argv = ([setsid, "-w"] if setsid else []) + r.argv(args)
                p = subprocess.Popen(argv, env=dict(r.env, **env), stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=subprocess.PIPE, text=True, start_new_session=not setsid)
                start = time.monotonic()
                try:
                    out, err = p.communicate(timeout=10)
                except subprocess.TimeoutExpired:
                    p.kill()
                    p.communicate()
                    self.fail("waited for input: %s" % (args,))
                self.assertLess(time.monotonic() - start, 10)
                return p.returncode, out, err
            code, out, err = run()
            self.assertEqual(code, 0, err)
            self.assertEqual(r.installed(), "ttyak " + LATEST)
            code, out, err = run("--uninstall")
            self.assertNotEqual(code, 0)
            self.assertIn("no terminal", err)
            self.assertIn("--yes", err)
            self.assertIn("--keep-data", err)
            self.assertFalse(os.path.exists(os.path.join(r.local_bin, "uninstall-args")))
        for r in self.runs(root=True):
            err = r.fails().stderr
            self.assertIn("running as root with no terminal", err)
            self.assertIn("--yes", err)
            self.assertIn("--dir", err)
            self.assertFalse(os.path.exists(r.local_bin))

    def test_root_installs_only_when_asked(self):
        for r in self.runs(root=True):
            p = r.ok("--yes")
            self.assertIn("running as root", p.stderr)
            self.assertRecord(r, LATEST)
            other = os.path.join(r.top, "opt")
            p = r.ok("--dir", other)
            self.assertIn("running as root", p.stderr)
            self.assertRecord(r, LATEST, other)
        for r in self.runs(root=True):
            code, out, err, shown = r.with_tty([], answer=b"y\n")
            self.assertEqual(code, 0, err)
            self.assertIn("install ttyak for root", shown)
            self.assertRecord(r, LATEST)
        for r in self.runs(root=True):
            code, out, err, shown = r.with_tty([], answer=b"\n")
            self.assertNotEqual(code, 0)
            self.assertIn("nothing installed", err)
            self.assertFalse(os.path.exists(os.path.join(r.local_bin, "ttyak")))

    def test_picks_platform(self):
        cases = ((dict(os_name="Linux", arch="x86_64"), "x86_64-linux"), (dict(os_name="Linux", arch="amd64"), "x86_64-linux"),
                 (dict(os_name="Linux", arch="aarch64"), "aarch64-linux"), (dict(os_name="Linux", arch="arm64"), "aarch64-linux"),
                 (dict(os_name="Darwin", arch="arm64", translated=0), "aarch64-macos"),
                 (dict(os_name="Darwin", arch="x86_64", translated=1), "aarch64-macos"),
                 (dict(os_name="Darwin", arch="x86_64", translated=0), "x86_64-macos"),
                 (dict(os_name="Darwin", arch="x86_64"), "x86_64-macos"))
        for kw, platform in cases:
            for r in self.runs(**kw):
                r.ok("--version", "0.11.0-rc.1")
                self.assertIn("GET /gh/releases/download/v0.11.0-rc.1/ttyak-0.11.0-rc.1-%s.zip" % platform, Handler.requests)
                with open(os.path.join(r.local_bin, "ttyak")) as f:
                    self.assertIn("ttyak stub for " + platform, f.read())
        # 0.10.0 has no aarch64 Linux build yet
        for r in self.runs(os_name="Linux", arch="aarch64"):
            p = r.fails()
            self.assertIn("ttyak %s has no build for aarch64-linux" % LATEST, p.stderr)
            self.assertFalse(os.path.exists(r.local_bin))
        for kw, words in ((dict(os_name="FreeBSD", arch="amd64"), "no build for FreeBSD"),
                          (dict(os_name="Linux", arch="i686"), "no build for i686 on linux"),
                          (dict(os_name="Linux", arch="riscv64"), "no build for riscv64 on linux"),
                          (dict(os_name="MINGW64_NT-10.0", arch="x86_64"), "install.ps1")):
            for r in self.runs(**kw):
                p = r.fails()
                self.assertIn(words, p.stderr)
                self.assertEqual(Handler.requests, [])
                self.assertFalse(os.path.exists(r.local_bin))

    def test_unzip_fallbacks(self):
        for r in self.runs(hide=("unzip", "bsdtar")):
            r.ok()
            self.assertEqual(r.installed(), "ttyak " + LATEST)
            self.assertEqual(stat.S_IMODE(os.stat(os.path.join(r.local_bin, "ttyak")).st_mode), 0o755)
        # bsdtar before python3 (a fake one, which logs and unpacks with python3)
        for r in self.runs(hide=("unzip", "bsdtar")):
            log = os.path.join(r.top, "bsdtar.log")
            write_exe(os.path.join(r.bin, "bsdtar"),
                      '#!/bin/sh\necho "$*" >%s\n[ "$1" = -xf ] && [ "$3" = -C ] && exec python3 -m zipfile -e "$2" "$4"\n' % log)
            r.ok()
            self.assertEqual(r.installed(), "ttyak " + LATEST)
            self.assertRegex(slurp(log), r"^-xf \S+/ttyak-%s-x86_64-linux\.zip -C \S+$" % LATEST)
        for r in self.runs(hide=("unzip", "bsdtar", "python3")):
            p = r.fails()
            self.assertIn("install unzip", p.stderr)
            self.assertEqual(Handler.requests, [])

    def test_other_tools(self):
        if shutil.which("wget"):
            for r in self.runs(hide=("curl",)):
                r.ok()
                self.assertEqual(r.installed(), "ttyak " + LATEST)
        if shutil.which("shasum"):
            for r in self.runs(hide=("sha256sum",)):
                r.ok()
                self.assertEqual(r.installed(), "ttyak " + LATEST)
                r.fails("--version", "6.6.6")
        for r in self.runs(hide=("curl", "wget")):
            p = r.fails()
            self.assertIn("needs curl or wget", p.stderr)
        for r in self.runs(hide=("sha256sum", "shasum")):
            p = r.fails()
            self.assertIn("needs sha256sum or shasum", p.stderr)

    def test_latest_falls_back_to_github(self):
        fetchers = [("curl", ())] + ([("wget", ("curl",))] if shutil.which("wget") else [])
        for name, hide in fetchers:
            for r in self.runs(hide=hide):
                with self.subTest(fetcher=name):
                    # latest.json unreachable: the redirect's tag
                    Handler.redirect_tag = "v0.9.0"
                    try:
                        p = r.ok(TTYAK_LATEST_URL=BASE + "/missing.json")
                        self.assertIn("asking GitHub", p.stdout)
                        self.assertEqual(r.installed(), "ttyak 0.9.0")
                        self.assertTrue({"HEAD /gh/releases/latest", "GET /gh/releases/latest"} & set(Handler.requests))
                        r.ok(TTYAK_LATEST_URL="http://127.0.0.1:1/latest.json")
                        self.assertEqual(r.installed(), "ttyak 0.9.0")
                        # broken feeds: the same, never anything from them
                        for feed in sorted(os.listdir(os.path.join(FIXTURES, "feeds"))):
                            Handler.redirect_tag = "v0.11.0-rc.1"
                            r.ok(TTYAK_LATEST_URL=BASE + "/feeds/" + feed)
                            self.assertEqual(r.installed(), "ttyak 0.11.0-rc.1", feed)
                            Handler.redirect_tag = "v0.9.0"
                            r.ok(TTYAK_LATEST_URL=BASE + "/feeds/" + feed)
                            self.assertEqual(r.installed(), "ttyak 0.9.0", feed)
                        # neither tells: a clear error
                        Handler.redirect_tag = None
                        p = r.fails(TTYAK_LATEST_URL=BASE + "/missing.json")
                        self.assertIn("could not find out the latest ttyak version", p.stderr)
                        self.assertIn("--version", p.stderr)
                        Handler.redirect_tag = "v1.0;rm"
                        p = r.fails(TTYAK_LATEST_URL=BASE + "/missing.json")
                        self.assertIn("could not find out the latest ttyak version", p.stderr)
                        # a download URL that is not GitHub's shape: no redirect to ask
                        p = r.fails(TTYAK_LATEST_URL=BASE + "/missing.json", TTYAK_DOWNLOAD_URL=BASE + "/mirror")
                        self.assertIn("could not find out the latest ttyak version", p.stderr)
                    finally:
                        Handler.redirect_tag = "v" + LATEST

    def test_uninstall_passes_flags(self):
        for r in self.runs():
            p = r.fails("--uninstall", "--yes")
            self.assertIn("no ttyak in " + r.local_bin, p.stderr)
            r.ok()
            args = os.path.join(r.local_bin, "uninstall-args")
            for flags, env, want in ((["--uninstall", "--yes"], {}, "args:--yes"),
                                     (["--uninstall", "--yes", "--keep-data"], {}, "args:--yes --keep-data"),
                                     (["--keep-data", "-y", "--uninstall"], {}, "args:--yes --keep-data"),
                                     ([], {"TTYAK_UNINSTALL": "1", "TTYAK_YES": "1"}, "args:--yes"),
                                     ([], {"TTYAK_UNINSTALL": "1", "TTYAK_YES": "1", "TTYAK_KEEP_DATA": "1"},
                                      "args:--yes --keep-data")):
                r.ok(*flags, **env)
                self.assertEqual(slurp(args), want + "\nstdin:other\n", (flags, env))
                os.remove(args)
            target = os.path.join(r.top, "elsewhere")
            r.ok("--dir", target)
            r.ok("--uninstall", "--yes", "--dir", target)
            self.assertEqual(slurp(os.path.join(target, "uninstall-args")), "args:--yes\nstdin:other\n")
            # with a terminal and no --yes, ttyak asks there itself
            code, out, err, shown = r.with_tty(["--uninstall"])
            self.assertEqual(code, 0, err)
            self.assertEqual(slurp(args), "args:\nstdin:tty\n")
            code, out, err, shown = r.with_tty(["--uninstall", "--keep-data"])
            self.assertEqual(slurp(args), "args:--keep-data\nstdin:tty\n")

    def test_flags(self):
        for r in self.runs():
            p = r.ok("--help")
            self.assertIn("--uninstall", p.stdout)
            self.assertIn("TTYAK_KEEP_DATA", p.stdout)
            self.assertEqual(Handler.requests, [])
            p = r.fails("--bogus")
            self.assertIn("unknown option: --bogus", p.stderr)
            p = r.fails(HOME="")
            self.assertIn("HOME is not set", p.stderr)

class Shellcheck(unittest.TestCase):
    @unittest.skipUnless(shutil.which("podman"), "needs podman")
    def test_shellcheck(self):
        # a copy, so that the container never relabels the repository
        d = tempfile.mkdtemp(prefix="ttyak-install-shellcheck-")
        self.addCleanup(shutil.rmtree, d)
        shutil.copy(SCRIPT, d)
        p = subprocess.run(["podman", "run", "--rm", "-v", d + ":/mnt:ro,Z", "docker.io/koalaman/shellcheck:stable",
                            "/mnt/install.sh"], capture_output=True, text=True, timeout=300)
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)


for _shell in SHELLS:
    _name = "InstallSh_" + os.path.basename(_shell)
    globals()[_name] = type(_name, (InstallSh, unittest.TestCase), {"shell": _shell})


if __name__ == "__main__":
    unittest.main()

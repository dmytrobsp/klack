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
MARKER = '# added by the ttyak installer; "ttyak uninstall" removes this line'
LATEST = "0.10.0"
PLATFORMS = ["x86_64-linux", "aarch64-macos", "x86_64-macos"]
# version -> platforms with an archive; 6.6.6's SHA256SUMS lies about its Linux archive
# 0.9.2's SHA256SUMS has CRLF line ends.
RELEASES = {"0.9.0": PLATFORMS, "0.9.2": PLATFORMS, "0.10.0": PLATFORMS,
            "0.11.0-rc.1": PLATFORMS + ["aarch64-linux", "x86_64-windows"], "6.6.6": PLATFORMS}
# The tools the script (and the stub) may run; a test hides some of them.
TOOLS = ["sh", "id", "ls", "mktemp", "rm", "mkdir", "chmod", "mv", "cp", "cat", "sed", "awk", "tr", "tail", "grep",
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


def posix_line(d=None):
    """The line the script puts in sh-like startup files, for ~/.local/bin or for the folder d."""
    if d is None:
        return 'case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) export PATH="$HOME/.local/bin:$PATH" ;; esac'
    q = "'" + d.replace("'", "'\\''") + "'"
    return 'case ":$PATH:" in *:%s:*) ;; *) export PATH=%s:"$PATH" ;; esac' % (q, q)


def fish_line(d=None):
    """The line the script puts in conf.d/ttyak.fish."""
    q = "~/.local/bin" if d is None else "'" + d.replace("\\", "\\\\").replace("'", "\\'") + "'"
    return "contains -- %s $PATH; or set -gx PATH %s $PATH" % (q, q)


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
        with open(os.path.join(d, "SHA256SUMS"), "w", newline="\r\n" if version == "0.9.2" else "\n") as f:
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
    os.makedirs(os.path.join(root, "feeds-ok"))
    with open(os.path.join(root, "feeds-ok", "crlf.json"), "w", newline="\r\n") as f:
        json.dump(dict(feed, version="0.9.2"), f, indent=2)


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

    def __init__(self, test, shell, os_name="Linux", arch="x86_64", translated=None, root=False, home_owner=0, hide=()):
        """root: `id -u` says 0; then `ls -ldn` says HOME belongs to home_owner (None: the real ls, this user)."""
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
            if home_owner is not None:
                write_exe(os.path.join(self.bin, "ls"),
                          '#!/bin/sh\nif [ "$1" = -ldn ]; then echo "drwx------ 2 %d 0 4096 Jan 1 00:00 $3"; '
                          'else exec %s "$@"; fi\n' % (home_owner, shutil.which("ls")))
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
            self.assertEqual(profile, posix_line(target) + " " + MARKER + "\n")
            self.assertIn("Added %s to PATH in %s/.profile" % (target, r.home), p.stdout)
            # the line works in a shell, also sourced twice
            out = subprocess.run([r.shell, "-c", '. "$HOME/.profile"; . "$HOME/.profile"; printf %s "$PATH"'],
                                 env={"HOME": r.home, "PATH": "/usr/bin"}, capture_output=True, text=True).stdout
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
        default_line = posix_line() + " " + MARKER + "\n"
        fish_default = fish_line() + " " + MARKER + "\n"
        cases = (("/bin/bash", ".bashrc", default_line), ("/usr/bin/zsh", ".zshrc", default_line),
                 ("/usr/bin/fish", ".config/fish/conf.d/ttyak.fish", fish_default), ("/bin/sh", ".profile", default_line),
                 ("/usr/bin/dash", ".profile", default_line), ("/usr/bin/ksh", ".profile", default_line),
                 ("/bin/mksh", ".profile", default_line), ("/usr/bin/yash", ".profile", default_line))
        for shell_path, rel, line in cases:
            for r in self.runs():
                with open(os.path.join(r.home, ".profile"), "w") as f:
                    f.write("# mine, no newline at the end")
                with open(os.path.join(r.home, ".bashrc"), "w") as f:
                    f.write("alias ll='ls -l'\n")
                p = r.ok(SHELL=shell_path)
                self.assertIn("Added %s to PATH in %s/%s; other shells you use need the same in their own startup files"
                              % (r.local_bin, r.home, rel), p.stdout)
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
        # the default line sourced twice puts the folder on PATH once
        for r in self.runs():
            r.ok()
            out = subprocess.run([r.shell, "-c", '. "$HOME/.profile"; . "$HOME/.profile"; printf %s "$PATH"'],
                                 env={"HOME": r.home, "PATH": "/usr/bin"}, capture_output=True, text=True).stdout
            self.assertEqual(out, r.local_bin + ":/usr/bin")
        # ZDOTDIR and XDG_CONFIG_HOME are followed
        for r in self.runs():
            zdot, xdg = os.path.join(r.top, "zdot"), os.path.join(r.top, "xdg")
            r.ok(SHELL="/bin/zsh", ZDOTDIR=zdot)
            self.assertEqual(slurp(os.path.join(zdot, ".zshrc")), default_line)
            r.ok(SHELL="/usr/bin/fish", XDG_CONFIG_HOME=xdg)
            self.assertEqual(slurp(os.path.join(xdg, "fish", "conf.d", "ttyak.fish")), fish_default)
        # already on PATH (also as a dir/ entry): nothing is added anywhere, and no note about another ttyak
        for r in self.runs():
            for path in (r.local_bin + ":" + r.bin, r.local_bin + "/:" + r.bin):
                for shell_path in ("/bin/bash", "/usr/bin/fish", "/bin/sh"):
                    p = r.ok(SHELL=shell_path, PATH=path)
                    self.assertIn("Run: ttyak", p.stdout)
                    self.assertEqual(p.stderr, "")
            self.assertEqual(sorted(os.listdir(r.home)), [".local"])
        # fish quoting for another folder
        for r in self.runs():
            target = os.path.join(r.top, "a'b\\c")
            r.ok("--dir", target, SHELL="/usr/bin/fish")
            self.assertEqual(r.read(".config/fish/conf.d/ttyak.fish"), fish_line(target) + " " + MARKER + "\n")

    def test_macos_bash_startup_file(self):
        # login shells read the first of these that exists: a new ~/.bash_profile would hide the others
        for existing, want in (((), ".bash_profile"), ((".profile",), ".profile"),
                               ((".bash_login", ".profile"), ".bash_login"), ((".bash_profile", ".profile"), ".bash_profile")):
            for r in self.runs(os_name="Darwin", arch="arm64", translated=0):
                for rel in existing:
                    with open(os.path.join(r.home, rel), "w") as f:
                        f.write("# mine\n")
                r.ok(SHELL="/bin/bash")
                self.assertEqual(r.read(want), ("# mine\n" if want in existing else "") + posix_line() + " " + MARKER + "\n")
                for rel in existing:
                    if rel != want:
                        self.assertEqual(r.read(rel), "# mine\n")
                self.assertEqual(sorted(os.listdir(r.home)), sorted({".local", want, *existing}))

    def test_other_shells_only_get_hints(self):
        cases = (("/bin/csh", "set path = ( ~/.local/bin $path )", "~/.cshrc"),
                 ("/usr/bin/tcsh", "set path = ( ~/.local/bin $path )", "~/.tcshrc"),
                 ("/usr/bin/nu", "$env.PATH = ($env.PATH | prepend ($nu.home-path | path join .local bin))", "env.nu"),
                 ("/usr/bin/elvish", "set paths = [~/.local/bin $@paths]", "rc.elv"),
                 ("/usr/bin/xonsh", "$PATH.insert(0, '~/.local/bin')", "~/.xonshrc"),
                 ("/opt/odd/bin/zork", posix_line(), "startup file"), ("", posix_line(), "startup file"))
        for shell_path, line, where in cases:
            for r in self.runs():
                p = r.ok(SHELL=shell_path)
                self.assertIn("  " + line + " " + MARKER + "\n", p.stdout)
                self.assertIn(where, p.stdout)
                self.assertNotIn("Added", p.stdout)
                self.assertEqual(sorted(os.listdir(r.home)), [".local"])

    def test_other_shells_hints_for_another_folder(self):
        for r in self.runs():
            plain = os.path.join(r.top, "plain dir")
            for shell_path, line in (("/bin/tcsh", "set path = ( '%s' $path )"), ("/usr/bin/nu", "$env.PATH = ($env.PATH | prepend '%s')"),
                                     ("/usr/bin/elvish", "set paths = ['%s' $@paths]"), ("/usr/bin/xonsh", "$PATH.insert(0, '%s')")):
                p = r.ok("--dir", plain, SHELL=shell_path)
                self.assertIn("  " + line % plain + " " + MARKER + "\n", p.stdout)
                # a folder with a quote or a backslash: words, no line to paste
                for odd in ("it's", 'say "x"', "back\\slash"):
                    target = os.path.join(r.top, odd)
                    p = r.ok("--dir", target, SHELL=shell_path)
                    self.assertIn("add %s to PATH in " % target, p.stdout)
                    self.assertNotIn(MARKER, p.stdout)
            self.assertEqual(os.listdir(r.home), [])

    def test_startup_folder_not_writable_or_linked(self):
        # an absent startup file whose folder cannot be written: the line is printed, the install stands
        for r in self.runs():
            zdot = os.path.join(r.top, "zdot")
            os.mkdir(zdot, 0o500)
            r.test.addCleanup(os.chmod, zdot, 0o700)
            p = r.ok(SHELL="/bin/zsh", ZDOTDIR=zdot)
            self.assertIn("  " + posix_line() + " " + MARKER + "\n", p.stdout)
            self.assertIn("cannot write in %s: add this line to %s/.zshrc" % (zdot, zdot), p.stdout)
            self.assertEqual(os.listdir(zdot), [])
            self.assertEqual(r.installed(), "ttyak " + LATEST)
        # a linked folder on the way (dotfiles kept elsewhere): nothing written through it
        for r in self.runs():
            dotfiles = os.path.join(r.top, "dotfiles")
            os.makedirs(os.path.join(dotfiles, "fish"))
            os.makedirs(os.path.join(dotfiles, "zsh"))
            os.symlink(dotfiles, os.path.join(r.home, ".config"))
            p = r.ok(SHELL="/usr/bin/fish")
            self.assertIn("  " + fish_line() + " " + MARKER + "\n", p.stdout)
            p = r.ok(SHELL="/bin/zsh", ZDOTDIR=os.path.join(r.home, ".config", "zsh"))
            self.assertIn("  " + posix_line() + " " + MARKER + "\n", p.stdout)
            self.assertEqual(sorted(os.listdir(os.path.join(dotfiles, "fish"))), [])
            self.assertEqual(sorted(os.listdir(os.path.join(dotfiles, "zsh"))), [])

    def test_profile_link_or_read_only_left_alone(self):
        for r in self.runs():
            real = os.path.join(r.top, "dotfiles-bashrc")
            with open(real, "w") as f:
                f.write("# managed\n")
            os.symlink(real, os.path.join(r.home, ".bashrc"))
            p = r.ok(SHELL="/bin/bash")
            self.assertEqual(slurp(real), "# managed\n")
            self.assertTrue(os.path.islink(os.path.join(r.home, ".bashrc")))
            self.assertIn("  " + posix_line() + " " + MARKER + "\n", p.stdout)
            self.assertIn(r.home + "/.bashrc", p.stdout)
        for r in self.runs():
            ro = os.path.join(r.home, ".zshrc")
            with open(ro, "w") as f:
                f.write("# mine\n")
            os.chmod(ro, 0o400)
            p = r.ok(SHELL="/bin/zsh")
            self.assertEqual(slurp(ro), "# mine\n")
            self.assertIn("  " + posix_line() + " " + MARKER + "\n", p.stdout)

    def test_no_modify_path(self):
        for r in self.runs():
            for args, env in ((["--no-modify-path"], {}), ([], {"TTYAK_NO_MODIFY_PATH": "1"})):
                p = r.ok(*args, SHELL="/bin/bash", **env)
                self.assertEqual(sorted(os.listdir(r.home)), [".local"])
                self.assertIn("  " + posix_line() + " " + MARKER + "\n", p.stdout)
                self.assertIn(r.home + "/.bashrc", p.stdout)

    def test_hash_read_from_stdin(self):
        # sha256sum escapes a file name with a backslash and then marks the hash with one too
        for hide in ((), ("sha256sum",)):
            if hide and not shutil.which("shasum"):
                continue
            for r in self.runs(hide=hide):
                tmp = os.path.join(r.top, "back\\slash")
                os.mkdir(tmp)
                r.ok(TMPDIR=tmp)
                self.assertEqual(r.installed(), "ttyak " + LATEST)
                self.assertEqual(os.listdir(tmp), [])

    def test_staged_files_never_followed(self):
        for r in self.runs():
            os.makedirs(r.local_bin)
            victim = os.path.join(r.top, "victim")
            with open(victim, "w") as f:
                f.write("keep\n")
            for name in (".ttyak.new", ".install.json.new"):
                os.symlink(victim, os.path.join(r.local_bin, name))
            r.ok()
            self.assertEqual(slurp(victim), "keep\n")
            self.assertEqual(r.installed(), "ttyak " + LATEST)
            self.assertRecord(r, LATEST)
            self.assertEqual(sorted(os.listdir(r.local_bin)), ["install.json", "ttyak"])

    def test_home_with_trailing_slash(self):
        for r in self.runs():
            r.ok(HOME=r.home + "/")
            self.assertRecord(r, LATEST)
            self.assertEqual(r.read(".profile"), posix_line() + " " + MARKER + "\n")

    def test_crlf_files(self):
        for r in self.runs():
            r.ok(TTYAK_LATEST_URL=BASE + "/feeds-ok/crlf.json")
            self.assertEqual(r.installed(), "ttyak 0.9.2")
            self.assertNotIn("HEAD /gh/releases/latest", Handler.requests)

    def test_truncated_script_runs_nothing(self):
        with open(SCRIPT, "rb") as f:
            data = f.read()
        for r in self.runs():
            part = os.path.join(r.top, "part.sh")
            for cut in [4000] + list(range(300, len(data) - 20, 700)):
                with open(part, "wb") as f:
                    f.write(data[:cut])
                Handler.requests.clear()
                with open(part, "rb") as stdin:
                    p = subprocess.run([r.shell], env=r.env, stdin=stdin, capture_output=True, timeout=30,
                                       start_new_session=True)
                if cut == 4000:
                    self.assertNotEqual(p.returncode, 0)
                self.assertEqual(os.listdir(r.home), [], cut)
                self.assertEqual(Handler.requests, [], cut)

    @unittest.skipUnless(shutil.which("fish"), "needs fish")
    def test_fish_line_works(self):
        # sourced twice: the folder once, first, and fish says nothing
        for r in self.runs():
            for args in ([], ["--dir", os.path.join(r.top, "a'b\\c d")]):
                r.ok(*args, SHELL="/usr/bin/fish")
                conf = os.path.join(r.home, ".config", "fish", "conf.d", "ttyak.fish")
                p = subprocess.run(["fish", "--no-config", "-c", "source $argv[1]; source $argv[1]; string join \\n -- $PATH", conf],
                                   env={"HOME": r.home, "PATH": "/usr/bin"}, capture_output=True, text=True)
                want = args[1] if args else r.local_bin
                self.assertEqual(p.stdout.splitlines(), [want, "/usr/bin"])
                self.assertEqual(p.stderr, "")
                os.remove(conf)

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

    def test_sudo_refused(self):
        # sudo that keeps the user's HOME would leave root-owned files in it
        for r in self.runs(root=True):
            p = r.fails("--yes", SUDO_USER="me")
            self.assertIn("you ran the installer with sudo", p.stderr)
            self.assertIn("no sudo needed", p.stderr)
            self.assertEqual(os.listdir(r.home), [])
            self.assertEqual(Handler.requests, [])
            # with --dir it installs there, and still touches nothing in the user's HOME
            other = os.path.join(r.top, "opt")
            for shell_path in ("/usr/bin/fish", "/bin/bash", "/bin/sh"):
                p = r.ok("--dir", other, SUDO_USER="me", SHELL=shell_path)
                self.assertRecord(r, LATEST, other)
                self.assertEqual(os.listdir(r.home), [])
                self.assertIn(MARKER + "\n", p.stdout)
                self.assertIn("sudo", p.stdout)
        for r in self.runs(root=True, home_owner=None):
            p = r.fails("--yes")
            self.assertIn("you ran the installer with sudo", p.stderr)
            self.assertEqual(os.listdir(r.home), [])
            other = os.path.join(r.top, "opt")
            r.ok("--dir", other, SHELL="/usr/bin/fish")
            self.assertRecord(r, LATEST, other)
            self.assertEqual(os.listdir(r.home), [])

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

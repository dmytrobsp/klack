"""Tests for site/install.ps1 on the Windows VM (`ssh windows`, Windows PowerShell 5.1), against a fake release served
from this machine on 192.168.122.1.

The fixtures (zips holding a stub `ttyak.exe`, SHA256SUMS, latest.json) are built here at test time. The stub is a
tiny C# program compiled on the VM with the .NET Framework's csc.exe; each release's copy has its version appended
to the file, which the stub reads back. Every run gets folders of its own under one temporary folder on the VM
(removed at the end) and a LOCALAPPDATA there, so the default folder is a temporary one too; one test uses the real
default folder and removes it afterwards. The user PATH is saved first and restored after every test that lets the
script change it. Skipped when the VM cannot be reached (CI has none); PSScriptAnalyzer runs in podman."""
import base64
import functools
import hashlib
import html
import http.server
import itertools
import json
import os
import pty
import re
import shutil
import subprocess
import tempfile
import threading
import unittest
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
SCRIPT = os.path.join(ROOT, "site", "install.ps1")
VM = "windows"
SSH = ["ssh", "-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", VM]
# The host's address on the VM's network: the fixture server listens there.
HOST = "192.168.122.1"
LATEST = "0.10.0"
PLATFORM = "x86_64-windows"
# version -> platforms with an archive; 6.6.6's SHA256SUMS lies about the Windows archive, 0.9.3's has no proper
# checksum for it, 0.9.1 has no Windows build; 0.9.2's SHA256SUMS has CRLF line ends and `*` (binary) marks.
RELEASES = {"0.9.0": [PLATFORM], "0.9.1": ["x86_64-linux"], "0.9.2": [PLATFORM], "0.9.3": [PLATFORM],
            "0.10.0": [PLATFORM, "x86_64-linux"], "0.11.0-rc.1": [PLATFORM], "6.6.6": [PLATFORM]}
STUB_CS = r"""
using System;
using System.IO;
using System.Text;
using System.Threading;

static class Stub
{
    static int Main(string[] args)
    {
        string exe = System.Reflection.Assembly.GetEntryAssembly().Location;
        string text = Encoding.ASCII.GetString(File.ReadAllBytes(exe));
        string tag = text.Substring(text.LastIndexOf("TTYAK-STUB ") + 11).Trim();
        string first = args.Length > 0 ? args[0] : "";
        if (first == "--version") { Console.WriteLine("ttyak " + tag.Split(' ')[0]); return 0; }
        if (first == "uninstall")
        {
            File.WriteAllText(Path.Combine(Path.GetDirectoryName(exe), "uninstall-args"),
                "args:" + string.Join(" ", args, 1, args.Length - 1) + "\nstdin:" +
                (Console.IsInputRedirected ? "other" : "tty") + "\nstdout:" + (Console.IsOutputRedirected ? "other" : "tty") + "\n");
            string code = Environment.GetEnvironmentVariable("TTYAK_STUB_EXIT");
            return string.IsNullOrEmpty(code) ? 0 : int.Parse(code);
        }
        if (first == "sleep") { Thread.Sleep(int.Parse(args[1]) * 1000); return 0; }
        return 0;
    }
}
"""
MANUAL_PATH_HINT = "Edit environment variables for your account"


def q(s):
    """s as a PowerShell single-quoted string."""
    return "'" + s.replace("'", "''") + "'"


def b64text(s):
    """PowerShell that evaluates to the string s, whatever it holds."""
    return "[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('%s'))" % base64.b64encode(s.encode()).decode()


def decode_clixml(err):
    """stderr as text: under -EncodedCommand Windows PowerShell writes its errors (and progress) as CLIXML."""
    def errors(m):
        text = "".join(t for kind, t in re.findall(r'<S S="(\w+)">(.*?)</S>', m.group(0), flags=re.S) if kind == "Error")
        return re.sub(r"_x([0-9A-Fa-f]{4})_", lambda x: chr(int(x.group(1), 16)), html.unescape(text))
    return re.sub(r"<Objs .*?</Objs>", errors, re.sub(r"#< CLIXML\r?\n", "", err), flags=re.S)


def ps(code, timeout=120, tty=False, shell="powershell"):
    """Runs PowerShell code on the VM with shell (powershell: 5.1, pwsh: 7), stdin closed (with tty, on a terminal,
    which passes no exit code back); returns the CompletedProcess."""
    prelude = ("$ProgressPreference = 'SilentlyContinue'\n"
               "[Console]::OutputEncoding = New-Object Text.UTF8Encoding $false\n")
    encoded = base64.b64encode((prelude + code).encode("utf-16-le")).decode()
    command = [shell + " -NoProfile -ExecutionPolicy Bypass -EncodedCommand " + encoded]
    if tty:
        # the remote side gets a terminal only when ssh's own stdin is one
        master, slave = pty.openpty()
        try:
            p = subprocess.run(["ssh", "-tt", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", VM] + command,
                               stdin=slave, capture_output=True, timeout=timeout)
        finally:
            os.close(slave)
            os.close(master)
    else:
        p = subprocess.run(SSH + command, stdin=subprocess.DEVNULL, capture_output=True, timeout=timeout)
    p.stdout = p.stdout.decode("utf-8", errors="replace").replace("\r\n", "\n")
    if tty:
        # the terminal's escape sequences
        p.stdout = re.sub(r"\x1b\[[0-9;?]*[A-Za-z]|\x1b\][^\x07]*\x07", "", p.stdout)
    p.stderr = decode_clixml(p.stderr.decode("utf-8", errors="replace")).replace("\r\n", "\n")
    return p


def ps_ok(code, timeout=120):
    p = ps(code, timeout)
    if p.returncode != 0:
        raise AssertionError("VM command failed (%d): %s%s" % (p.returncode, p.stdout, p.stderr))
    return p.stdout


def vm_reachable():
    if not shutil.which("ssh"):
        return False
    try:
        return subprocess.run(SSH + ["exit"], stdin=subprocess.DEVNULL, capture_output=True, timeout=30).returncode == 0
    except subprocess.TimeoutExpired:
        return False


REACHABLE = vm_reachable()


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def build_fixtures(root, stub):
    """The fake release tree under root: gh/releases/download/v<V>/…, latest.json, broken feeds, install.ps1."""
    for version, platforms in RELEASES.items():
        d = os.path.join(root, "gh", "releases", "download", "v" + version)
        os.makedirs(d)
        # names next to the real one, which must not be taken for it
        sums = ["%s  ttyak-%s-%s.zip.old\n" % ("1" * 64, version, PLATFORM),
                "%s  xttyak-%s-%s.zip\n" % ("2" * 64, version, PLATFORM)]
        for platform in platforms:
            name = "ttyak-%s-%s.zip" % (version, platform)
            with zipfile.ZipFile(os.path.join(d, name), "w") as z:
                z.writestr("ttyak.exe", stub + ("\nTTYAK-STUB %s %s\n" % (version, platform)).encode())
                for extra in ("README.md", "LICENSE-MIT", "LICENSE-APACHE"):
                    z.writestr(extra, extra + "\n")
            with open(os.path.join(d, name), "rb") as f:
                digest = sha256(f.read())
            if version == "6.6.6":
                digest = "0" * 64
            if version == "0.9.3":
                digest = "z" * 64
            sums.append("%s %s%s\n" % (digest, " *" if version == "0.9.2" else " ", name))
        with open(os.path.join(d, "SHA256SUMS"), "w", newline="\r\n" if version == "0.9.2" else "\n") as f:
            f.writelines(sums)
    feed = {"name": "ttyak", "version": LATEST, "date": "2026-10-23",
            "notes": "https://github.com/dmytrobsp/ttyak/releases/tag/v" + LATEST,
            "assets": {PLATFORM: {"url": "https://example.invalid/ttyak-%s-%s.zip" % (LATEST, PLATFORM),
                                  "sha256": "0" * 64}}}
    with open(os.path.join(root, "latest.json"), "w") as f:
        json.dump(feed, f, indent=2)
    os.makedirs(os.path.join(root, "feeds"))
    broken = {
        "empty.json": "",
        "no-version.json": '{"name":"ttyak","date":"2026-10-23"}',
        "number.json": '{"name":"ttyak","version":10,"date":"2026-10-23"}',
        "html.json": "<html><body>404 Not Found</body></html>",
        "shell.json": '{"name":"ttyak","version":"0.10.0$(New-Item pwned)","date":"2026-10-23"}',
        "two-lines.json": '{"name":"ttyak","version":"0.10.0\\n1.0.0"}',
        "truncated.json": '{"name":"ttyak","vers',
    }
    for name, text in broken.items():
        with open(os.path.join(root, "feeds", name), "w") as f:
            f.write(text)
    os.makedirs(os.path.join(root, "feeds-ok"))
    with open(os.path.join(root, "feeds-ok", "crlf.json"), "w", newline="\r\n") as f:
        json.dump(dict(feed, version="0.9.2"), f, indent=2)
    shutil.copy(SCRIPT, os.path.join(root, "install.ps1"))
    os.makedirs(os.path.join(root, "text"))
    shutil.copy(SCRIPT, os.path.join(root, "text", "install.ps1"))


class Handler(http.server.SimpleHTTPRequestHandler):
    """Serves the fixture tree; /gh/releases/latest redirects as GitHub's does (to `redirect_tag`, 404 when None).
    install.ps1 goes out as application/octet-stream, the worst a static host may say; /text/… as text/plain."""
    redirect_tag = "v" + LATEST
    requests = []

    def log_message(self, *args):
        pass

    def guess_type(self, path):
        if path.endswith(".ps1"):
            return "text/plain" if "/text/" in path.replace("\\", "/") else "application/octet-stream"
        return super().guess_type(path)

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


# The user PATH as the registry holds it ({"kind": …, "value": …}; kind None: no value), read before any test.
ORIGINAL_PATH = None
PIDS = []


def read_user_path():
    out = ps_ok("$k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment')\n"
                "$v = $k.GetValue('Path', $null, 'DoNotExpandEnvironmentNames')\n"
                "$kind = if ($null -eq $v) { $null } else { $k.GetValueKind('Path').ToString() }\n"
                "[Console]::Out.Write((ConvertTo-Json @{ kind = $kind; value = $v } -Compress))")
    return json.loads(out)


def write_user_path(state):
    """Sets the user PATH as state says (kind None: removes it)."""
    if state["kind"] is None:
        change = "$k.DeleteValue('Path', $false)"
    else:
        change = "$k.SetValue('Path', %s, [Microsoft.Win32.RegistryValueKind]::%s)" % (b64text(state["value"]), state["kind"])
    ps_ok("$k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)\n%s\n$k.Close()" % change)


FIXTURES = SERVER = VM_ROOT = None


def setUpModule():
    global FIXTURES, SERVER, BASE, VM_ROOT, ORIGINAL_PATH, ELEVATED
    if not REACHABLE:
        return
    try:
        ORIGINAL_PATH = read_user_path()
        VM_ROOT = ps_ok("$d = Join-Path ([IO.Path]::GetTempPath()) ('ttyak-ps1-test-' + [Guid]::NewGuid().ToString('N').Substring(0, 8))\n"
                        "[void][IO.Directory]::CreateDirectory($d)\n[Console]::Out.Write($d)")
        ELEVATED = ps_ok("([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent())"
                         ".IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)").strip() == "True"
        out = ps_ok("$d = %s\n[IO.File]::WriteAllText(\"$d\\stub.cs\", %s)\n"
                    "& \"$env:WINDIR\\Microsoft.NET\\Framework64\\v4.0.30319\\csc.exe\" /nologo /out:\"$d\\stub.exe\" \"$d\\stub.cs\" | Out-Null\n"
                    "[Console]::Out.Write([Convert]::ToBase64String([IO.File]::ReadAllBytes(\"$d\\stub.exe\")))"
                    % (q(VM_ROOT), b64text(STUB_CS)))
        stub = base64.b64decode(out)
        FIXTURES = tempfile.mkdtemp(prefix="ttyak-install-ps1-fixtures-")
        build_fixtures(FIXTURES, stub)
        SERVER = http.server.ThreadingHTTPServer((HOST, 0), functools.partial(Handler, directory=FIXTURES))
        threading.Thread(target=SERVER.serve_forever, daemon=True).start()
        BASE = "http://%s:%d" % (HOST, SERVER.server_address[1])
    except BaseException:
        tearDownModule()
        raise


def tearDownModule():
    """Stops the server, removes the fixtures and the VM's test folder, restores the user PATH; also after a setup
    that failed halfway."""
    if not REACHABLE:
        return
    if SERVER:
        SERVER.shutdown()
        SERVER.server_close()
    if FIXTURES:
        shutil.rmtree(FIXTURES)
    for pid in PIDS:
        ps("Stop-Process -Id %d -Force -ErrorAction SilentlyContinue" % pid)
    if ORIGINAL_PATH is not None and read_user_path() != ORIGINAL_PATH:
        write_user_path(ORIGINAL_PATH)
    if VM_ROOT:
        ps_ok("Remove-Item -LiteralPath %s -Recurse -Force" % q(VM_ROOT))
        left = ps_ok("Test-Path -LiteralPath %s" % q(VM_ROOT)).strip()
        assert left == "False", "the VM's test folder is still there: " + VM_ROOT
    assert ORIGINAL_PATH is None or read_user_path() == ORIGINAL_PATH, "the user PATH was not restored"


COUNTER = itertools.count(1)


class Run:
    """One run's folders on the VM: top, tmp (TEMP), local (LOCALAPPDATA); `dir` is the default install folder."""

    def __init__(self, test, modify_path=False, real_local=False):
        """modify_path: let the script change the user PATH (restored after the test), in a session whose PATH is
        the machine's only, like a terminal opened before the install; otherwise TTYAK_NO_MODIFY_PATH=1.
        real_local: the real LOCALAPPDATA, so the real default folder."""
        self.test = test
        self.modify_path = modify_path
        self.top = "%s\\r%d" % (VM_ROOT, next(COUNTER))
        self.tmp = self.top + "\\tmp"
        self.local = None if real_local else self.top + "\\local"
        ps_ok("[void][IO.Directory]::CreateDirectory(%s)" % q(self.tmp))
        if real_local:
            self.dir = ps_ok("[Console]::Out.Write((Join-Path $env:LOCALAPPDATA 'Programs\\ttyak'))")
        else:
            self.dir = self.local + "\\Programs\\ttyak"
        self.env = {"TTYAK_LATEST_URL": BASE + "/latest.json", "TTYAK_DOWNLOAD_URL": BASE + "/gh/releases/download"}
        if modify_path:
            test.addCleanup(write_user_path, ORIGINAL_PATH)
        else:
            self.env["TTYAK_NO_MODIFY_PATH"] = "1"

    def setup(self, env, cwd=None, home=None, prepend_path=None):
        """The PowerShell lines that set up a run: its TEMP, LOCALAPPDATA, environment and folder."""
        lines = ["$env:TEMP = %s" % q(self.tmp), "$env:TMP = %s" % q(self.tmp)]
        if self.local:
            lines.append("$env:LOCALAPPDATA = %s" % q(self.local))
        for k, v in env.items():
            lines.append("Remove-Item Env:%s -ErrorAction SilentlyContinue" % k if v is None else "$env:%s = %s" % (k, q(v)))
        if self.modify_path:
            lines.append("$env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine')")
        if prepend_path:
            lines.append("$env:Path = %s + ';' + $env:Path" % q(prepend_path))
        if home:
            lines.append("(Get-PSProvider FileSystem).Home = %s" % q(home))
        lines.append("Set-Location -LiteralPath %s" % q(cwd or self.top))
        return "\n".join(lines) + "\n"

    def run(self, *args, mode="file", cwd=None, home=None, prepend_path=None, before="", after="", tty=False, **env):
        """Runs the script with args: mode "file" as a .ps1 file, "iex" as `irm … | iex`, "sb" as
        `& ([scriptblock]::Create((irm …))) args`. Checks that TEMP is left empty."""
        Handler.requests.clear()
        argv = " ".join(a if re.fullmatch(r"-[A-Za-z]+", a) else q(a) for a in args)
        if mode == "file":
            call = "& %s %s" % (q(VM_ROOT + "\\install.ps1"), argv)
        elif mode == "iex":
            call = "irm %s | iex" % q(BASE + "/install.ps1")
        else:
            call = "& ([scriptblock]::Create((irm %s))) %s" % (q(BASE + "/install.ps1"), argv)
        code = (self.setup(dict(self.env, **env), cwd, home, prepend_path) + before + "\n$global:LASTEXITCODE = 99\n" + call +
                "\n$code = $LASTEXITCODE\n" + after +
                "\n[Console]::Out.Write('@@LEFT ' + @(Get-ChildItem -Force -LiteralPath %s).Count)\nexit $code\n" % q(self.tmp))
        p = ps(code, tty=tty, shell=self.test.shell)
        m = re.search(r"@@LEFT (\d+)\s*$", p.stdout)
        self.test.assertIsNotNone(m, p.stdout + p.stderr)
        self.test.assertEqual(m.group(1), "0", "temporary files left behind")
        p.stdout = p.stdout[:m.start()]
        return p

    def ok(self, *args, **kw):
        p = self.run(*args, **kw)
        self.test.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        return p

    def fails(self, *args, **kw):
        p = self.run(*args, **kw)
        self.test.assertNotEqual(p.returncode, 0, p.stdout + p.stderr)
        self.test.assertNotEqual(p.returncode, 99, "the script set no exit code: " + p.stdout + p.stderr)
        return p

    def state(self, d=None):
        """What is in the folder d: {files, record (base64 of install.json), version (`ttyak.exe --version`)}."""
        return json.loads(ps_ok(
            "$d = %s\n$s = @{ files = @(); record = $null; version = $null; exists = (Test-Path -LiteralPath $d) }\n"
            "if ($s.exists) { $s.files = @(Get-ChildItem -Force -LiteralPath $d | ForEach-Object { $_.Name } | Sort-Object) }\n"
            "if (Test-Path -LiteralPath \"$d\\install.json\") { $s.record = [Convert]::ToBase64String([IO.File]::ReadAllBytes(\"$d\\install.json\")) }\n"
            "if (Test-Path -LiteralPath \"$d\\ttyak.exe\") { $s.version = (& \"$d\\ttyak.exe\" --version | Out-String).Trim() }\n"
            "[Console]::Out.Write((ConvertTo-Json $s -Compress))" % q(d or self.dir)))

    def exists(self, path):
        return ps_ok("Test-Path -LiteralPath %s" % q(path)).strip() == "True"

    def read(self, path):
        return base64.b64decode(ps_ok("[Console]::Out.Write([Convert]::ToBase64String([IO.File]::ReadAllBytes(%s)))" % q(path))).decode()

    def start(self, exe, seconds):
        """Starts `exe sleep seconds` outside the ssh session (so it outlives it); returns its PID."""
        out = ps_ok("$r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = %s }\n"
                    "[Console]::Out.Write($r.ProcessId)" % q('"%s" sleep %d' % (exe, seconds)))
        pid = int(out)
        PIDS.append(pid)
        self.test.addCleanup(self.stop, pid)
        return pid

    def stop(self, pid):
        ps("Stop-Process -Id %d -Force -ErrorAction SilentlyContinue; Wait-Process -Id %d -Timeout 10 -ErrorAction SilentlyContinue" % (pid, pid))


class InstallPs1:
    """The tests, run once per PowerShell (the classes at the end)."""
    shell = None

    @classmethod
    def setUpClass(cls):
        if not REACHABLE:
            raise unittest.SkipTest("needs the Windows VM (ssh %s), which CI does not have" % VM)
        # by its full path: the PATH tests give the VM's sessions another PATH
        found = ps("[Console]::Out.Write((Get-Command %s -CommandType Application -ErrorAction Stop)[0].Source)" % cls.shell)
        if found.returncode != 0:
            raise unittest.SkipTest("no %s on the VM" % cls.shell)
        cls.shell = found.stdout.strip()
        subprocess.run(["scp", "-q", "-o", "BatchMode=yes", SCRIPT, "%s:%s" % (VM, VM_ROOT.replace("\\", "/") + "/install.ps1")],
                       stdin=subprocess.DEVNULL, check=True, timeout=60)

    def runs(self, **kw):
        """A fresh run (one, as a loop, so that each case reads as a block)."""
        yield Run(self, **kw)

    def assertRecord(self, r, version, d=None):
        d = d or r.dir
        raw = base64.b64decode(r.state(d)["record"])
        self.assertEqual(raw, ('{"method":"script","version":"%s","dir":%s}\n' % (version, json.dumps(d, ensure_ascii=False))).encode())
        self.assertEqual(json.loads(raw), {"method": "script", "version": version, "dir": d})

    def assertCleanErr(self, p):
        """Nothing on stderr but the note that Administrator is not needed (an ssh session is elevated)."""
        err = p.stderr
        if ELEVATED:
            self.assertIn("ttyak installer: running as Administrator is not needed", err)
            err = "\n".join(l for l in err.splitlines() if "running as Administrator" not in l)
        self.assertEqual(err.strip(), "")

    def test_installs_latest(self):
        for r in self.runs():
            p = r.ok()
            s = r.state()
            self.assertEqual(s["version"], "ttyak " + LATEST)
            self.assertRecord(r, LATEST)
            self.assertEqual(s["files"], ["install.json", "ttyak.exe"])
            self.assertIn("Downloading ttyak %s for x86_64-windows" % LATEST, p.stdout)
            self.assertIn("Installed ttyak %s as %s\\ttyak.exe" % (LATEST, r.dir), p.stdout)
            self.assertIn("To remove it later: ttyak uninstall", p.stdout)
            self.assertIn("GET /latest.json", Handler.requests)
            self.assertIn("GET /gh/releases/download/v%s/ttyak-%s-x86_64-windows.zip" % (LATEST, LATEST), Handler.requests)
            self.assertNotIn("HEAD /gh/releases/latest", Handler.requests)
            self.assertCleanErr(p)

    def test_updates_in_place(self):
        for r in self.runs():
            r.ok("-Version", "0.9.0")
            self.assertEqual(r.state()["version"], "ttyak 0.9.0")
            p = r.ok()
            s = r.state()
            self.assertEqual(s["version"], "ttyak " + LATEST)
            self.assertRecord(r, LATEST)
            self.assertIn("Updated ttyak 0.9.0 to %s in %s" % (LATEST, r.dir), p.stdout)
            self.assertEqual(s["files"], ["install.json", "ttyak.exe"])
            p = r.ok()
            self.assertIn("Installed ttyak %s over the copy in %s" % (LATEST, r.dir), p.stdout)
            self.assertEqual(r.state()["files"], ["install.json", "ttyak.exe"])

    def test_updates_while_running(self):
        for r in self.runs():
            r.ok("-Version", "0.9.0")
            first = r.start(r.dir + "\\ttyak.exe", 120)
            p = r.ok()
            s = r.state()
            self.assertEqual(s["version"], "ttyak " + LATEST)
            self.assertRecord(r, LATEST)
            self.assertEqual(s["files"], ["install.json", "ttyak.exe", "ttyak.old.exe"])
            self.assertIn("Updated ttyak 0.9.0 to %s" % LATEST, p.stdout)
            self.assertIn("ttyak.old.exe", p.stdout)
            # the new copy runs too, while the first still holds ttyak.old.exe: another name for this one
            second = r.start(r.dir + "\\ttyak.exe", 120)
            r.ok("-Version", "0.11.0-rc.1")
            s = r.state()
            self.assertEqual(s["version"], "ttyak 0.11.0-rc.1")
            self.assertEqual(len(s["files"]), 4, s["files"])
            self.assertRegex(s["files"][3], r"^ttyak\.old-[0-9a-f]{8}\.exe$")
            self.assertEqual(s["files"][:3], ["install.json", "ttyak.exe", "ttyak.old.exe"])
            # once they quit, the next run clears both
            r.stop(first)
            r.stop(second)
            r.ok("-Version", "0.11.0-rc.1")
            self.assertEqual(r.state()["files"], ["install.json", "ttyak.exe"])

    def test_version_flag_and_env(self):
        for r in self.runs():
            for args, env, want in ((["-Version", "0.9.0"], {}, "0.9.0"), (["-Version", "0.11.0-rc.1"], {}, "0.11.0-rc.1"),
                                    ([], {"TTYAK_VERSION": "0.9.0"}, "0.9.0"), (["-Version", "v0.11.0-rc.1"], {}, "0.11.0-rc.1"),
                                    (["-Version", "0.9.0"], {"TTYAK_VERSION": "6.6.6"}, "0.9.0")):
                r.ok(*args, **env)
                self.assertEqual(r.state()["version"], "ttyak " + want, (args, env))
                self.assertRecord(r, want)
                self.assertNotIn("GET /latest.json", Handler.requests)
            for bad in ("1.2", "1.2.3;true", "..\\..\\etc", "1.2.3 4", "1.2.3\n4.5.6", "1.2.3\n"):
                p = r.fails("-Version", bad)
                self.assertIn("not a ttyak version", p.stderr)
                self.assertEqual(Handler.requests, [])
            p = r.fails(TTYAK_VERSION="0.9")
            self.assertIn("not a ttyak version", p.stderr)
            p = r.fails("-Version", "9.9.9")
            self.assertIn("could not download", p.stderr)
            p = r.fails("-Version", "0.9.1")
            self.assertIn("ttyak 0.9.1 has no build for x86_64-windows", p.stderr)
            p = r.fails("-Version", "0.9.3")
            self.assertIn("SHA256SUMS has no proper checksum for ttyak-0.9.3-x86_64-windows.zip", p.stderr)
            self.assertEqual(r.state()["version"], "ttyak 0.9.0")

    def test_dir_flag(self):
        for r in self.runs():
            target = r.top + "\\my tools\\it's here \u00fcn\u00ef"
            p = r.ok("-Dir", target)
            s = r.state(target)
            self.assertEqual(s["version"], "ttyak " + LATEST)
            self.assertRecord(r, LATEST, target)
            self.assertIn("Installed ttyak %s as %s\\ttyak.exe" % (LATEST, target), p.stdout)
            self.assertFalse(r.exists(r.dir))
            # the env var, a relative folder, ~\, a trailing backslash, forward slashes
            r.ok(TTYAK_DIR=r.top + "\\env-dir")
            self.assertRecord(r, LATEST, r.top + "\\env-dir")
            r.ok("-Dir", "rel", cwd=r.top)
            self.assertRecord(r, LATEST, r.top + "\\rel")
            r.ok("-Dir", "~\\apps", home=r.top + "\\home")
            self.assertRecord(r, LATEST, r.top + "\\home\\apps")
            r.ok("-Dir", r.top + "\\trail\\")
            self.assertRecord(r, LATEST, r.top + "\\trail")
            r.ok("-Dir", (r.top + "\\fwd").replace("\\", "/"))
            self.assertRecord(r, LATEST, r.top + "\\fwd")
            # names PATH (or JSON) cannot carry
            for bad in ("line\nbreak", "tab\there", "semi;colon"):
                p = r.fails("-Dir", r.top + "\\" + bad)
                self.assertIn("pick another with -Dir", p.stderr)
                self.assertEqual(Handler.requests, [])
            self.assertEqual(r.state(r.top)["files"], ["env-dir", "fwd", "home", "my tools", "rel", "tmp", "trail"])

    def test_dir_not_writable(self):
        for r in self.runs():
            ro = r.top + "\\ro"
            # a deny entry binds Administrators too
            ps_ok("[void][IO.Directory]::CreateDirectory(%s)\nicacls %s /deny '*S-1-1-0:(OI)(CI)(W,AD,WD)' | Out-Null" % (q(ro), q(ro)))
            self.addCleanup(ps, "icacls %s /remove:d '*S-1-1-0' | Out-Null" % q(ro))
            p = r.fails("-Dir", ro)
            self.assertIn("cannot write to " + ro, p.stderr)

    def test_refuses_bad_checksum(self):
        for r in self.runs():
            p = r.fails("-Version", "6.6.6")
            self.assertIn("checksum mismatch for ttyak-6.6.6-x86_64-windows.zip", p.stderr)
            self.assertIn("nothing was installed", p.stderr)
            self.assertFalse(r.exists(r.dir))
            # an existing install stays as it was
            r.ok("-Version", "0.9.0")
            r.fails("-Version", "6.6.6")
            s = r.state()
            self.assertEqual(s["version"], "ttyak 0.9.0")
            self.assertEqual(s["files"], ["install.json", "ttyak.exe"])
            self.assertRecord(r, "0.9.0")

    def test_path_added_once(self):
        test_path = "%USERPROFILE%\\ttyak-test-fake;C:\\ttyak-test-keep"
        for r in self.runs(modify_path=True):
            write_user_path({"kind": "ExpandString", "value": test_path})
            p = r.ok()
            self.assertEqual(read_user_path(), {"kind": "ExpandString", "value": r.dir + ";" + test_path})
            self.assertIn("Added %s to your user PATH" % r.dir, p.stdout)
            self.assertIn("ttyak uninstall", p.stdout)
            self.assertIn("Open a new terminal", p.stdout)
            # again: nothing added
            p = r.ok()
            self.assertEqual(read_user_path(), {"kind": "ExpandString", "value": r.dir + ";" + test_path})
            self.assertIn("%s is on your PATH already: open a new terminal to use ttyak" % r.dir, p.stdout)
            self.assertNotIn("Added", p.stdout)
        # already there in another case, with a trailing backslash, as %VAR%, or quoted; the kind is kept
        for r in self.runs(modify_path=True):
            for kind, value in (("String", "C:\\ttyak-test-keep;" + r.dir.upper() + "\\"),
                                ("ExpandString", "%TTYAK_TEST_DIR%;C:\\ttyak-test-keep"),
                                ("String", '"%s"' % r.dir)):
                state = {"kind": kind, "value": value}
                write_user_path(state)
                p = r.ok(TTYAK_TEST_DIR=r.dir)
                self.assertEqual(read_user_path(), state)
                self.assertIn("on your PATH already", p.stdout)
        # no user PATH at all; a REG_SZ one stays REG_SZ
        for r in self.runs(modify_path=True):
            write_user_path({"kind": None})
            r.ok()
            self.assertEqual(read_user_path(), {"kind": "ExpandString", "value": r.dir})
            write_user_path({"kind": "String", "value": "C:\\ttyak-test-keep"})
            r.ok()
            self.assertEqual(read_user_path(), {"kind": "String", "value": r.dir + ";C:\\ttyak-test-keep"})
        # on this session's PATH only: nothing added, and a note when another ttyak.exe comes first
        for r in self.runs(modify_path=True):
            write_user_path({"kind": "String", "value": "C:\\ttyak-test-keep"})
            p = r.ok(prepend_path=r.dir)
            self.assertIn("Run: ttyak", p.stdout)
            self.assertEqual(read_user_path(), {"kind": "String", "value": "C:\\ttyak-test-keep"})
            other = r.top + "\\other"
            r.ok("-Dir", other, "-NoModifyPath")
            p = r.ok(prepend_path=other + ";" + r.dir)
            self.assertIn("note: %s\\ttyak.exe comes before %s on PATH" % (other, r.dir), p.stderr)

    def test_no_modify_path(self):
        for r in self.runs(modify_path=True):
            write_user_path({"kind": "String", "value": "C:\\ttyak-test-keep"})
            for args, env in ((["-NoModifyPath"], {}), ([], {"TTYAK_NO_MODIFY_PATH": "1"}), ([], {"TTYAK_NO_MODIFY_PATH": "yes"})):
                p = r.ok(*args, **env)
                self.assertEqual(read_user_path(), {"kind": "String", "value": "C:\\ttyak-test-keep"})
                self.assertIn("%s is not on PATH" % r.dir, p.stdout)
                self.assertIn(MANUAL_PATH_HINT, p.stdout)

    def test_default_dir(self):
        for r in self.runs(modify_path=True, real_local=True):
            if r.exists(r.dir):
                self.skipTest("%s exists: a real install is not touched" % r.dir)
            self.addCleanup(ps, "Remove-Item -LiteralPath %s -Recurse -Force -ErrorAction SilentlyContinue" % q(r.dir))
            p = r.ok()
            self.assertIn("Installed ttyak %s as %s\\ttyak.exe" % (LATEST, r.dir), p.stdout)
            self.assertRegex(r.dir, r"^C:\\Users\\[^\\]+\\AppData\\Local\\Programs\\ttyak$")
            self.assertRecord(r, LATEST)
            path = read_user_path()
            if ORIGINAL_PATH["kind"] is None:
                self.assertEqual(path, {"kind": "ExpandString", "value": r.dir})
            else:
                self.assertEqual(path, {"kind": ORIGINAL_PATH["kind"], "value": r.dir + ";" + ORIGINAL_PATH["value"]})
            r.ok("-Uninstall", "-Yes")
            self.assertEqual(r.read(r.dir + "\\uninstall-args"), "args:--yes\nstdin:other\nstdout:other\n")
            ps_ok("Remove-Item -LiteralPath %s -Recurse -Force" % q(r.dir))
            self.assertFalse(r.exists(r.dir))

    def test_arch(self):
        for r in self.runs():
            for arch, words in (("ARM64", "no Windows on ARM build"), ("Arm64", "no Windows on ARM build"),
                                ("ARM", "no Windows on ARM build"), ("x86", "no build for 32-bit Windows"),
                                ("IA64", "no build for IA64")):
                p = r.fails("-Arch", arch)
                self.assertIn(words, p.stderr)
                self.assertEqual(Handler.requests, [])
                self.assertFalse(r.exists(r.dir))
            for arch in ("AMD64", "X64"):
                r.ok("-Arch", arch)
                self.assertIn("GET /gh/releases/download/v%s/ttyak-%s-x86_64-windows.zip" % (LATEST, LATEST), Handler.requests)
            p = r.fails("-Arch", "ARM64", "-Version", "0.9.0", mode="sb")
            self.assertIn("no Windows on ARM build", p.stderr)

    def test_latest_falls_back_to_github(self):
        for r in self.runs():
            Handler.redirect_tag = "v0.9.0"
            try:
                p = r.ok(TTYAK_LATEST_URL=BASE + "/missing.json")
                self.assertIn("asking GitHub", p.stdout)
                self.assertEqual(r.state()["version"], "ttyak 0.9.0")
                self.assertIn("HEAD /gh/releases/latest", Handler.requests)
                r.ok(TTYAK_LATEST_URL="http://%s:1/latest.json" % HOST)
                self.assertEqual(r.state()["version"], "ttyak 0.9.0")
                # broken feeds: the same, never anything from them
                for feed in sorted(os.listdir(os.path.join(FIXTURES, "feeds"))):
                    Handler.redirect_tag = "v0.11.0-rc.1"
                    r.ok(TTYAK_LATEST_URL=BASE + "/feeds/" + feed)
                    self.assertEqual(r.state()["version"], "ttyak 0.11.0-rc.1", feed)
                self.assertFalse(r.exists(r.top + "\\pwned"))
                # neither tells: a clear error
                Handler.redirect_tag = None
                p = r.fails(TTYAK_LATEST_URL=BASE + "/missing.json")
                self.assertIn("could not find out the latest ttyak version", p.stderr)
                self.assertIn("-Version", p.stderr)
                Handler.redirect_tag = "v1.0;rm"
                p = r.fails(TTYAK_LATEST_URL=BASE + "/missing.json")
                self.assertIn("could not find out the latest ttyak version", p.stderr)
                # a download URL that is not GitHub's shape: no redirect to ask
                p = r.fails(TTYAK_LATEST_URL=BASE + "/missing.json", TTYAK_DOWNLOAD_URL=BASE + "/mirror")
                self.assertIn("could not find out the latest ttyak version", p.stderr)
            finally:
                Handler.redirect_tag = "v" + LATEST

    def test_crlf_files(self):
        for r in self.runs():
            r.ok(TTYAK_LATEST_URL=BASE + "/feeds-ok/crlf.json")
            self.assertEqual(r.state()["version"], "ttyak 0.9.2")
            self.assertNotIn("HEAD /gh/releases/latest", Handler.requests)

    def test_staged_files_never_followed(self):
        for r in self.runs():
            victim = r.top + "\\victim"
            made = ps("[void][IO.Directory]::CreateDirectory(%s)\n[IO.File]::WriteAllText(%s, 'keep')\n"
                      "foreach ($n in '.ttyak.exe.new', '.install.json.new') {\n"
                      "  New-Item -ItemType SymbolicLink -Path (Join-Path %s $n) -Target %s -ErrorAction Stop | Out-Null }"
                      % (q(r.dir), q(victim), q(r.dir), q(victim)))
            if made.returncode != 0:
                self.skipTest("cannot make symbolic links here: " + made.stderr)
            r.ok()
            self.assertEqual(r.read(victim), "keep")
            self.assertEqual(r.state()["files"], ["install.json", "ttyak.exe"])
            self.assertRecord(r, LATEST)

    def test_uninstall_passes_flags(self):
        for r in self.runs():
            p = r.fails("-Uninstall", "-Yes")
            self.assertIn("no ttyak in " + r.dir, p.stderr)
            r.ok()
            args = r.dir + "\\uninstall-args"
            for flags, env, want in ((["-Uninstall", "-Yes"], {}, "args:--yes"),
                                     (["-Uninstall", "-Yes", "-KeepData"], {}, "args:--yes --keep-data"),
                                     (["-KeepData", "-Yes", "-Uninstall"], {}, "args:--yes --keep-data"),
                                     ([], {"TTYAK_UNINSTALL": "1", "TTYAK_YES": "1"}, "args:--yes"),
                                     ([], {"TTYAK_UNINSTALL": "1", "TTYAK_YES": "1", "TTYAK_KEEP_DATA": "1"},
                                      "args:--yes --keep-data")):
                r.ok(*flags, **env)
                self.assertEqual(r.read(args), want + "\nstdin:other\nstdout:other\n", (flags, env))
                ps_ok("Remove-Item -LiteralPath %s" % q(args))
            # its exit code is the script's
            p = r.run("-Uninstall", "-Yes", TTYAK_STUB_EXIT="3")
            self.assertEqual(p.returncode, 3)
            target = r.top + "\\elsewhere"
            r.ok("-Dir", target)
            r.ok("-Uninstall", "-Yes", "-Dir", target)
            self.assertEqual(r.read(target + "\\uninstall-args"), "args:--yes\nstdin:other\nstdout:other\n")
            # no terminal and no -Yes: it stops before asking
            ps_ok("Remove-Item -LiteralPath %s" % q(args))
            p = r.fails("-Uninstall")
            self.assertIn("no terminal", p.stderr)
            self.assertIn("-Yes", p.stderr)
            self.assertIn("-KeepData", p.stderr)
            self.assertFalse(r.exists(args))
            # on a terminal, ttyak asks there itself
            r.run("-Uninstall", "-KeepData", tty=True)
            self.assertEqual(r.read(args), "args:--keep-data\nstdin:tty\nstdout:tty\n")

    def test_iex_forms(self):
        for r in self.runs():
            # irm | iex: the environment only, and the session's own variables stay as they were
            mine = "$Version = 'mine'; $Dir = 'mine'; $Yes = 'mine'"
            shown = "\n[Console]::Out.WriteLine('vars:' + $Version + ',' + $Dir + ',' + $Yes)"
            p = r.ok(mode="iex", TTYAK_DIR=r.top + "\\iex", before=mine, after=shown)
            self.assertIn("vars:mine,mine,mine", p.stdout)
            self.assertRecord(r, LATEST, r.top + "\\iex")
            self.assertIn("GET /install.ps1", Handler.requests)
            # a failure: the session goes on, with the exit code in $LASTEXITCODE
            p = r.fails(mode="iex", TTYAK_DIR=r.top + "\\iex", TTYAK_VERSION="6.6.6", after="\n[Console]::Out.WriteLine('still here')")
            self.assertIn("checksum mismatch", p.stderr)
            self.assertIn("still here", p.stdout)
            self.assertEqual(p.returncode, 1)
            # served as text/plain
            p = ps(r.setup(dict(r.env, TTYAK_DIR=r.top + "\\text")) + "irm %s | iex\nexit $LASTEXITCODE" % q(BASE + "/text/install.ps1"),
                   shell=self.shell)
            self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
            self.assertRecord(r, LATEST, r.top + "\\text")
            # the scriptblock form takes the flags
            r.ok("-Yes", "-Version", "0.9.0", "-Dir", r.top + "\\sb", mode="sb")
            self.assertRecord(r, "0.9.0", r.top + "\\sb")
            r.ok("-Uninstall", "-Yes", "-KeepData", "-Dir", r.top + "\\sb", mode="sb")
            self.assertEqual(r.read(r.top + "\\sb\\uninstall-args"), "args:--yes --keep-data\nstdin:other\nstdout:other\n")
            p = r.ok("-Help", mode="sb")
            self.assertIn("-Uninstall", p.stdout)
            self.assertEqual(Handler.requests, ["GET /install.ps1"])

    def test_truncated_script_runs_nothing(self):
        with open(SCRIPT, "rb") as f:
            data = f.read()
        last = data.rstrip(b"\r\n").rfind(b"\n") + 1
        cuts = list(range(300, last, 700)) + list(range(last, len(data.rstrip(b"\r\n"))))
        os.makedirs(os.path.join(FIXTURES, "cut"), exist_ok=True)
        for cut in cuts:
            with open(os.path.join(FIXTURES, "cut", "%d.ps1" % cut), "wb") as f:
                f.write(data[:cut])
        for r in self.runs():
            target = r.top + "\\cut"
            code = r.setup(dict(r.env, TTYAK_DIR=target)) + (
                "foreach ($cut in %s) { try { irm (%s + $cut + '.ps1') | iex } catch { } }" % (",".join(map(str, cuts)), q(BASE + "/cut/")))
            Handler.requests.clear()
            ps(code, timeout=300, shell=self.shell)
            self.assertEqual([x for x in Handler.requests if not x.startswith("GET /cut/")], [])
            self.assertEqual(len(Handler.requests), len(cuts))
            self.assertFalse(r.exists(target))

    def test_flags(self):
        for r in self.runs():
            p = r.ok("-Help")
            self.assertIn("-Uninstall", p.stdout)
            self.assertIn("TTYAK_KEEP_DATA", p.stdout)
            self.assertIn("[scriptblock]::Create", p.stdout)
            self.assertEqual(Handler.requests, [])
            p = r.run("-Bogus")
            self.assertIn("Bogus", p.stderr)
            self.assertEqual(Handler.requests, [])
            self.assertFalse(r.exists(r.dir))

    def test_ascii_only(self):
        # Windows PowerShell 5.1 reads a .ps1 without a BOM in the ANSI code page
        with open(SCRIPT, "rb") as f:
            data = f.read()
        self.assertTrue(all(b < 128 for b in data))
        self.assertFalse(data.startswith(b"\xef\xbb\xbf"))


# Windows PowerShell 5.1 and PowerShell 7
for _shell in ("powershell", "pwsh"):
    _name = "InstallPs1_" + _shell
    globals()[_name] = type(_name, (InstallPs1, unittest.TestCase), {"shell": _shell})


class ScriptAnalyzer(unittest.TestCase):
    @unittest.skipUnless(shutil.which("podman"), "needs podman")
    def test_script_analyzer_and_other_systems(self):
        # a copy, so that the container never relabels the repository
        d = tempfile.mkdtemp(prefix="ttyak-install-pssa-")
        self.addCleanup(shutil.rmtree, d)
        shutil.copy(SCRIPT, d)
        check = ("$ErrorActionPreference = 'Stop'\n"
                 "Install-Module PSScriptAnalyzer -Scope CurrentUser -Force | Out-Null\n"
                 "$found = @(Invoke-ScriptAnalyzer -Path /mnt/install.ps1 -Severity Information, Warning, Error)\n"
                 "$found | Format-Table -AutoSize -Wrap RuleName, Line, Message | Out-String -Width 200\n"
                 "'found: ' + $found.Count\n"
                 "& /mnt/install.ps1 2>&1 | Out-String\n"
                 "'exit: ' + $LASTEXITCODE\n")
        p = subprocess.run(["podman", "run", "--rm", "-v", d + ":/mnt:ro,Z", "mcr.microsoft.com/powershell:7.5-ubuntu-24.04",
                            "pwsh", "-NoProfile", "-Command", check], capture_output=True, text=True, timeout=600)
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertIn("found: 0", p.stdout, p.stdout + p.stderr)
        # PowerShell on Linux: pointed to install.sh
        self.assertIn("curl -fsSL https://ttyak.sh/install.sh | sh", p.stdout + p.stderr)
        self.assertIn("exit: 1", p.stdout)


if __name__ == "__main__":
    unittest.main()

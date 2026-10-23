"""Tests for packaging/render.py: every recipe rendered from a fixture SHA256SUMS for 0.11.0.

Run: python3 -m unittest discover -s packaging -p 'test_*.py' (the folder is no package: `packaging` is a PyPI library).
The .SRCINFO is also compared with `makepkg --printsrcinfo` in a podman archlinux container when podman is there (in
CI, env CI set, only with TTYAK_PODMAN_TESTS=1)."""
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)
import render  # noqa: E402

V = "0.11.0"
DATE = "2026-11-01"
PLATFORMS = ["x86_64-linux", "aarch64-linux", "x86_64-macos", "aarch64-macos", "x86_64-windows"]
SHA = {p: hashlib.sha256(p.encode()).hexdigest() for p in PLATFORMS}
IDENTITY = render.load_identity()
ARCH_IMAGE = "docker.io/library/archlinux:latest"
# The Rust side's copy of the rendered latest.json (update::tests parses it).
RUST_FEED = os.path.join(ROOT, "crates", "ttyak", "testdata", "latest.json")


def sums(platforms=PLATFORMS, version=V):
    """A SHA256SUMS as the release workflow writes it (sha256sum's two spaces)."""
    return "".join("%s  ttyak-%s-%s.zip\n" % (SHA[p], version, p) for p in platforms)


def url(p, version=V):
    return "https://github.com/dmytrobsp/ttyak/releases/download/v%s/ttyak-%s-%s.zip" % (version, version, p)


class Rendered:
    """One render into a temporary folder; read(path) gives a rendered file's text."""

    def __init__(self, platforms=PLATFORMS, identity=None):
        self.dir = tempfile.mkdtemp(prefix="ttyak-render-")
        self.paths = render.render_all(V, sums(platforms), self.dir, DATE, identity or IDENTITY)

    def read(self, rel):
        with open(os.path.join(self.dir, rel), encoding="utf-8", newline="") as f:
            return f.read()

    def files(self):
        out = []
        for d, _, names in os.walk(self.dir):
            out += [os.path.relpath(os.path.join(d, n), self.dir) for n in names]
        return sorted(out)

    def close(self):
        shutil.rmtree(self.dir)


WINGET_DIR = "winget/manifests/d/dmytrobsp/ttyak/%s" % V
EVERY_FILE = sorted([
    "homebrew/ttyak.rb", "scoop/ttyak.json", "chocolatey/ttyak.nuspec", "chocolatey/tools/chocolateyInstall.ps1",
    "chocolatey/tools/chocolateyUninstall.ps1", "aur/PKGBUILD", "aur/.SRCINFO", "nfpm.yaml", "latest.json",
    WINGET_DIR + "/dmytrobsp.ttyak.yaml", WINGET_DIR + "/dmytrobsp.ttyak.installer.yaml",
    WINGET_DIR + "/dmytrobsp.ttyak.locale.en-US.yaml",
])


def block(text, opener):
    """The lines inside the Ruby block that starts at the line `opener` (up to its own `end`)."""
    lines = text.splitlines()
    start = next(i for i, l in enumerate(lines) if l.strip() == opener)
    indent = len(lines[start]) - len(lines[start].lstrip())
    for j in range(start + 1, len(lines)):
        if lines[j].strip() == "end" and len(lines[j]) - len(lines[j].lstrip()) == indent:
            return "\n".join(lines[start + 1:j])
    raise AssertionError("no end for " + opener)


class AllPlatforms(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.r = Rendered()

    @classmethod
    def tearDownClass(cls):
        cls.r.close()

    def test_every_file_is_rendered(self):
        self.assertEqual(self.r.files(), EVERY_FILE)
        self.assertEqual(sorted(os.path.relpath(p, self.r.dir) for p in self.r.paths), EVERY_FILE)

    def test_formula(self):
        rb = self.r.read("homebrew/ttyak.rb")
        self.assertRegex(rb, r'(?m)^class Ttyak < Formula$')
        self.assertIn('  homepage "https://dmytrobsp.github.io/ttyak/"\n', rb)
        self.assertNotIn("  version ", rb, "brew reads the version from the urls")
        self.assertIn('  license any_of: ["MIT", "Apache-2.0"]\n', rb)
        for os_block, arm, intel in [("on_macos do", "aarch64-macos", "x86_64-macos"),
                                     ("on_linux do", "aarch64-linux", "x86_64-linux")]:
            outer = block(rb, os_block)
            for inner, p in [("on_arm do", arm), ("on_intel do", intel)]:
                self.assertEqual(block(outer, inner).split(), ["url", '"%s"' % url(p), "sha256", '"%s"' % SHA[p]], p)
        self.assertNotIn('depends_on "unzip"', rb, "Homebrew adds unzip itself where it is missing")
        self.assertNotIn("depends_on arch:", rb, "both Linux archives")
        self.assertNotRegex(rb, r"(?m)^  (url|sha256) ", "every url in its OS and CPU block")
        self.assertIn('bin.install "ttyak"', block(rb, "def install"))
        self.assertIn('assert_match version.to_s, shell_output("#{bin}/ttyak --version")', block(rb, "test do"))
        caveats = block(rb, "def caveats")
        self.assertIn("ttyak purge", caveats)
        self.assertIn("brew uninstall ttyak", caveats)
        self.assertNotIn("install.json", rb)

    def test_scoop(self):
        m = json.loads(self.r.read("scoop/ttyak.json"))
        self.assertEqual((m["version"], m["url"], m["hash"], m["bin"]), (V, url("x86_64-windows"), SHA["x86_64-windows"], "ttyak.exe"))
        self.assertEqual((m["homepage"], m["license"]), ("https://dmytrobsp.github.io/ttyak/", "MIT|Apache-2.0"))
        for absent in ["checkver", "autoupdate", "extract_dir", "architecture", "installer", "pre_uninstall", "post_uninstall"]:
            self.assertNotIn(absent, m)
        self.assertIn("deletes your ttyak data", " ".join(m["notes"]))
        self.assertTrue(m["description"])
        script = m["uninstaller"]["script"]
        self.assertEqual(script[0], "if ($cmd -eq 'uninstall') {")
        self.assertEqual(script[-1], "}")
        purges = [i for i, l in enumerate(script) if "purge" in l]
        self.assertEqual([script[i].strip() for i in purges], ['& "$dir\\ttyak.exe" purge --yes'])
        self.assertTrue(all(0 < i < len(script) - 1 for i in purges), "the purge only inside the uninstall guard")
        self.assertEqual(sum(l.count("{") - l.count("}") for l in script), 0)

    def test_chocolatey(self):
        ns = {"n": "http://schemas.microsoft.com/packaging/2015/06/nuspec.xsd"}
        meta = ET.fromstring(self.r.read("chocolatey/ttyak.nuspec").encode()).find("n:metadata", ns)
        get = lambda tag: meta.find("n:" + tag, ns).text
        self.assertEqual((get("id"), get("version"), get("authors"), get("owners")), ("ttyak", V, IDENTITY["name"], IDENTITY["name"]))
        self.assertEqual(get("projectUrl"), "https://dmytrobsp.github.io/ttyak/")
        self.assertEqual(get("releaseNotes"), "https://github.com/dmytrobsp/ttyak/releases/tag/v0.11.0")
        self.assertTrue(get("licenseUrl").startswith("https://github.com/dmytrobsp/ttyak"))
        self.assertIn("slack", get("tags").split())
        for tag in ["summary", "description"]:
            self.assertIn("choco uninstall ttyak", get(tag), tag)
        install = self.r.read("chocolatey/tools/chocolateyInstall.ps1")
        self.assertIn("Install-ChocolateyZipPackage @packageArgs", install)
        self.assertIn("url64bit       = '%s'" % url("x86_64-windows"), install)
        self.assertIn("checksum64     = '%s'" % SHA["x86_64-windows"], install)
        self.assertIn("checksumType64 = 'sha256'", install)
        self.assertIn("unzipLocation  = $toolsDir", install)
        self.assertNotRegex(install, r"(?m)^\s*url\s*=", "64-bit only")
        uninstall = self.r.read("chocolatey/tools/chocolateyUninstall.ps1")
        lines = [l.strip() for l in uninstall.splitlines()]
        call = lines.index("$output = & $exe purge --yes 2>&1")
        self.assertIn("$ErrorActionPreference = 'Continue'", lines[:call], "a purge writing to stderr is no error")
        self.assertIn("$output | ForEach-Object { Write-Host $_ }", lines[call:])
        self.assertTrue(any(l.startswith("if ($code -ne 0) {") for l in lines[call:]))
        self.assertTrue(any(l.startswith("Write-Warning ") for l in lines[call:]), "a refused purge warns")
        self.assertIn("} catch {", lines, "a purge that cannot start warns too")
        self.assertNotIn("throw", uninstall)
        for ps1 in [install, uninstall]:
            self.assertNotIn("install.json", ps1)

    def test_pkgbuild(self):
        pb = self.r.read("aur/PKGBUILD")
        self.assertRegex(pb, r"(?m)^# Maintainer: %s$" % re.escape("%s <%s>" % (IDENTITY["name"], IDENTITY["email"])))
        for line in ["pkgname='ttyak-bin'", "pkgver='0.11.0'", "pkgrel=1", "arch=('x86_64' 'aarch64')",
                     "provides=('ttyak')", "conflicts=('ttyak')", "depends=('alsa-lib' 'glibc' 'libgcc' 'libstdc++')", "options=('!debug' '!strip')",
                     "license=('MIT OR Apache-2.0')",
                     "source_x86_64=('ttyak-0.11.0-x86_64-linux.zip::%s')" % url("x86_64-linux"),
                     "sha256sums_x86_64=('%s')" % SHA["x86_64-linux"],
                     "source_aarch64=('ttyak-0.11.0-aarch64-linux.zip::%s')" % url("aarch64-linux"),
                     "sha256sums_aarch64=('%s')" % SHA["aarch64-linux"]]:
            self.assertIn(line + "\n", pb)
        self.assertNotIn("noextract", pb)
        package = pb[pb.index("package() {"):]
        for line in ['install -Dm755 ttyak "$pkgdir/usr/bin/ttyak"',
                     'install -Dm644 README.md "$pkgdir/usr/share/doc/$pkgname/README.md"',
                     'install -Dm644 LICENSE-MIT "$pkgdir/usr/share/licenses/$pkgname/LICENSE-MIT"',
                     'install -Dm644 LICENSE-APACHE "$pkgdir/usr/share/licenses/$pkgname/LICENSE-APACHE"']:
            self.assertIn("\t" + line + "\n", package)
        self.assertNotIn("ln ", package, "the real file at /usr/bin/ttyak, not a link")

    def test_srcinfo(self):
        want = ("pkgbase = ttyak-bin\n"
                "\tpkgdesc = %s\n" % render.AUR["pkgdesc"] +
                "\tpkgver = 0.11.0\n\tpkgrel = 1\n\turl = https://dmytrobsp.github.io/ttyak/\n"
                "\tarch = x86_64\n\tarch = aarch64\n\tlicense = MIT OR Apache-2.0\n"
                "\tdepends = alsa-lib\n\tdepends = glibc\n\tdepends = libgcc\n\tdepends = libstdc++\n"
                "\tprovides = ttyak\n\tconflicts = ttyak\n\toptions = !debug\n\toptions = !strip\n"
                "\tsource_x86_64 = ttyak-0.11.0-x86_64-linux.zip::%s\n" % url("x86_64-linux") +
                "\tsha256sums_x86_64 = %s\n" % SHA["x86_64-linux"] +
                "\tsource_aarch64 = ttyak-0.11.0-aarch64-linux.zip::%s\n" % url("aarch64-linux") +
                "\tsha256sums_aarch64 = %s\n" % SHA["aarch64-linux"] +
                "\npkgname = ttyak-bin\n")
        self.assertEqual(self.r.read("aur/.SRCINFO"), want)

    def test_winget(self):
        version = self.r.read(WINGET_DIR + "/dmytrobsp.ttyak.yaml")
        installer = self.r.read(WINGET_DIR + "/dmytrobsp.ttyak.installer.yaml")
        locale = self.r.read(WINGET_DIR + "/dmytrobsp.ttyak.locale.en-US.yaml")
        for text, kind, schema in [(version, "version", "version"), (installer, "installer", "installer"),
                                   (locale, "defaultLocale", "defaultLocale")]:
            self.assertTrue(text.startswith("# yaml-language-server: $schema=https://aka.ms/winget-manifest.%s.1.28.0.schema.json\n" % schema), kind)
            self.assertIn('\nPackageIdentifier: "dmytrobsp.ttyak"\n', text)
            self.assertIn('\nPackageVersion: "0.11.0"\n', text)
            self.assertIn("\nManifestType: %s\nManifestVersion: 1.28.0\n" % kind, text)
        self.assertIn("\nDefaultLocale: en-US\n", version)
        for line in ["InstallerType: zip", "NestedInstallerType: portable", "  - RelativeFilePath: ttyak.exe",
                     "    PortableCommandAlias: ttyak", "  - Architecture: x64",
                     '    InstallerUrl: "%s"' % url("x86_64-windows"),
                     '    InstallerSha256: "%s"' % SHA["x86_64-windows"].upper()]:
            self.assertIn("\n" + line + "\n", installer)
        for line in ['Publisher: "%s"' % IDENTITY["name"], 'Author: "%s"' % IDENTITY["name"],
                     'PublisherUrl: "https://dmytrobsp.github.io/ttyak/"', 'License: "MIT OR Apache-2.0"',
                     'ReleaseNotesUrl: "https://github.com/dmytrobsp/ttyak/releases/tag/v0.11.0"', "PackageLocale: en-US",
                     "PackageName: ttyak"]:
            self.assertIn("\n" + line + "\n", locale)
        self.assertRegex(locale, r"\nShortDescription: .+\n")
        self.assertIn("ttyak purge", locale)

    def test_nfpm(self):
        y = self.r.read("nfpm.yaml")
        for line in ["name: ttyak", 'version: "0.11.0"', "arch: ${TTYAK_ARCH}",
                     'maintainer: "%s <%s>"' % (IDENTITY["name"], IDENTITY["email"]),
                     'homepage: "https://dmytrobsp.github.io/ttyak/"', 'license: "MIT OR Apache-2.0"',
                     "  - src: ${TTYAK_DIR}/ttyak", "    dst: /usr/bin/ttyak", "      mode: 0755",
                     "    dst: /usr/share/doc/ttyak/README.md", "    dst: /usr/share/doc/ttyak/LICENSE-MIT",
                     "    dst: /usr/share/doc/ttyak/LICENSE-APACHE"]:
            self.assertIn("\n" + line + "\n", y)
        self.assertNotIn("type: symlink", y, "the real file at /usr/bin/ttyak")
        self.assertIn("ttyak purge", y)
        for deps, want in [("deb", ["libasound2 (>= 1.0.16)", "libc6 (>= 2.34)", "libgcc-s1", "libstdc++6"]),
                           ("rpm", ["libasound.so.2()(64bit)", "libc.so.6(GLIBC_2.34)(64bit)", "libgcc_s.so.1()(64bit)", "libstdc++.so.6()(64bit)"])]:
            m = re.search(r"\n  %s:\n    depends:\n((?:      - .*\n)+)" % deps, y)
            self.assertEqual(re.findall(r'      - "([^"]+)"', m.group(1)), want, deps)
        for hook in ["scripts:", "postinstall", "preremove", "postremove", "$HOME", "/home"]:
            self.assertNotIn(hook, y, "no scripts touching home folders")

    def test_latest_json(self):
        raw = self.r.read("latest.json")
        feed = json.loads(raw)
        self.assertEqual(list(feed), ["name", "version", "date", "notes", "assets"])
        self.assertEqual((feed["name"], feed["version"], feed["date"], feed["notes"]),
                         ("ttyak", V, DATE, "https://github.com/dmytrobsp/ttyak/releases/tag/v0.11.0"))
        self.assertEqual(list(feed["assets"]), PLATFORMS)
        for p in PLATFORMS:
            self.assertEqual(feed["assets"][p], {"url": url(p), "sha256": SHA[p]})
        self.assertEqual(feed["assets"]["aarch64-macos"]["url"],
                         "https://github.com/dmytrobsp/ttyak/releases/download/v0.11.0/ttyak-0.11.0-aarch64-macos.zip")
        with open(RUST_FEED, encoding="utf-8", newline="") as f:
            self.assertEqual(raw, f.read(), "crates/ttyak/testdata/latest.json is this render's output")

    def test_winget_id_is_install_rs(self):
        with open(os.path.join(ROOT, "crates", "ttyak", "src", "install.rs"), encoding="utf-8") as f:
            m = re.search(r'macro_rules! winget_id \{\s*\(\) => \{\s*"([^"]+)"', f.read())
        self.assertEqual(m.group(1), IDENTITY["winget_id"], "ttyak's winget commands name the id the manifests carry")

    def test_no_install_json_anywhere(self):
        for rel in self.r.files():
            self.assertNotIn("install.json", self.r.read(rel), rel)

    def test_the_name_only_in_maintainer_publisher_author(self):
        allowed = {
            "aur/PKGBUILD": [r"^# Maintainer: "],
            "nfpm.yaml": [r"^maintainer: "],
            "chocolatey/ttyak.nuspec": [r"^\s*<authors>", r"^\s*<owners>"],
            WINGET_DIR + "/dmytrobsp.ttyak.locale.en-US.yaml": [r"^Publisher: ", r"^Author: "],
        }
        for rel in self.r.files():
            for line in self.r.read(rel).splitlines():
                # The repo's and the site's addresses and the winget id carry the handle too.
                rest = re.sub(r"(https?://)?[\w.-]+\.(com|io|sh)/\S*", "", line).replace(IDENTITY["winget_id"], "")
                if IDENTITY["name"] in rest or IDENTITY["email"] in line:
                    self.assertTrue(any(re.match(p, line) for p in allowed.get(rel, [])), "%s: %s" % (rel, line))
        self.assertIn(IDENTITY["email"], self.r.read("aur/PKGBUILD"))
        self.assertIn(IDENTITY["email"], self.r.read("nfpm.yaml"))


class WithoutArmLinux(unittest.TestCase):
    """aarch64-linux is not in the releases yet: it is simply absent where it would be needed."""

    @classmethod
    def setUpClass(cls):
        cls.r = Rendered([p for p in PLATFORMS if p != "aarch64-linux"])

    @classmethod
    def tearDownClass(cls):
        cls.r.close()

    def test_same_files(self):
        self.assertEqual(self.r.files(), EVERY_FILE)

    def test_absent_everywhere(self):
        for rel in self.r.files():
            text = self.r.read(rel)
            self.assertNotIn("aarch64-linux", text, rel)
            self.assertNotIn(SHA["aarch64-linux"], text, rel)

    def test_formula_linux_intel_only(self):
        rb = self.r.read("homebrew/ttyak.rb")
        # `url` may not stand in on_linux and arm Linux needs one to load: the Intel archive at the top (macOS's blocks
        # override it), arm Linux refused by depends_on (checked with brew audit and brew info --variations).
        self.assertIn('\n  url "%s"\n  sha256 "%s"\n  license ' % (url("x86_64-linux"), SHA["x86_64-linux"]), rb)
        self.assertEqual(block(rb, "on_linux do").split(), ["depends_on", "arch:", ":x86_64"], "brew refuses arm Linux, not fails to load")
        macos = block(rb, "on_macos do")
        self.assertIn("on_arm do", macos)
        self.assertIn("on_intel do", macos)

    def test_aur_x86_64_only(self):
        pb = self.r.read("aur/PKGBUILD")
        self.assertIn("\narch=('x86_64')\n", pb)
        self.assertNotIn("aarch64", pb)
        si = self.r.read("aur/.SRCINFO")
        self.assertIn("\tarch = x86_64\n\tlicense", si)
        self.assertNotIn("aarch64", si)

    def test_latest_json(self):
        feed = json.loads(self.r.read("latest.json"))
        self.assertEqual(list(feed["assets"]), ["x86_64-linux", "x86_64-macos", "aarch64-macos", "x86_64-windows"])


class Refusals(unittest.TestCase):
    def setUp(self):
        self.out = tempfile.mkdtemp(prefix="ttyak-render-")
        self.addCleanup(shutil.rmtree, self.out)

    def refused(self, version, text, message):
        with self.assertRaises(render.Refused) as cm:
            render.render_all(version, text, self.out, DATE, IDENTITY)
        self.assertIn(message, str(cm.exception))
        self.assertEqual(os.listdir(self.out), [], "nothing written")

    def test_invalid_semver(self):
        for bad in ["", "0.11", "v0.11.0", "0.11.0.1", "01.11.0", "0.011.0", "0.11.0-", "0.11.0-rc..1", "0.11.0-01",
                    "0.11.0+build.1", "0.11.0 ", "0.11.0\n", "٠.11.0", "0.11.0-rc_1"]:
            self.refused(bad, sums(version=bad), "not a release version")

    def test_valid_semver(self):
        for good in ["0.11.0", "1.0.0-rc.1", "10.20.30-alpha.beta-2", "0.0.0"]:
            self.assertEqual(render.check_version(good), good)

    def test_bad_sums_lines(self):
        good = sums()
        for bad, why in [
            (SHA["x86_64-linux"][:63] + "  ttyak-0.11.0-x86_64-linux.zip\n", "short hash"),
            ("g" + SHA["x86_64-linux"][1:] + "  ttyak-0.11.0-x86_64-linux.zip\n", "not hex"),
            (SHA["x86_64-linux"] + " ttyak-0.11.0-x86_64-linux.zip\n", "one space"),
            (SHA["x86_64-linux"] + "\tttyak-0.11.0-x86_64-linux.zip\n", "a tab"),
            (SHA["x86_64-linux"] + "  ttyak-0.10.0-x86_64-linux.zip\n", "another version"),
            (SHA["x86_64-linux"] + "  ttyak-0.11.0-riscv64-linux.zip\n", "an unknown platform"),
            (SHA["x86_64-linux"] + "  ttyak-0.11.0-x86_64-linux.zip\n", "a platform twice"),
            (SHA["x86_64-linux"] + "  dir/ttyak-0.11.0-x86_64-linux.zip\n", "a path"),
            (SHA["x86_64-linux"] + "  dir/ttyak_0.11.0_amd64.deb\n", "a path to another file"),
            (SHA["x86_64-linux"] + "  ..\\ttyak_0.11.0_amd64.deb\n", "a Windows path"),
            (SHA["x86_64-linux"] + "  ttyak-0.11.0-rc.1-x86_64-linux.zip\n", "a pre-release's archive"),
            (SHA["x86_64-linux"] + "  ttyak_0.11.0_amd64.deb\n" + SHA["x86_64-linux"] + "  ttyak_0.11.0_amd64.deb\n", "another file twice"),
            (SHA["x86_64-linux"][:40] + "  ttyak_0.11.0_amd64.deb\n", "another file's short hash"),
            ("SHA256 (ttyak-0.11.0-x86_64-linux.zip) = " + SHA["x86_64-linux"] + "\n", "BSD style"),
        ]:
            self.refused(V, good + bad, "SHA256SUMS line")

    def test_the_release_s_other_files_are_skipped(self):
        """The release's full SHA256SUMS: packages, the recipes archive and signatures besides the archives."""
        other = hashlib.sha256(b"other").hexdigest()
        for version, deb in [(V, V), ("0.11.0-rc.1", "0.11.0~rc.1")]:
            extra = ["ttyak_%s_amd64.deb" % deb, "ttyak_%s_arm64.deb" % deb, "ttyak-%s-1.x86_64.rpm" % deb,
                     "ttyak-%s-1.aarch64.rpm" % deb, "ttyak-%s-recipes.zip" % version, "ttyak-%s-x86_64-linux.zip.sig" % version,
                     "ttyak-%s-x86_64-windows.zip.intoto.jsonl" % version, "SHA256SUMS.asc"]
            text = sums(version=version) + "".join("%s  %s\n" % (other, n) for n in extra)
            self.assertEqual(render.parse_sums(text, version), SHA, version)
            out = tempfile.mkdtemp(prefix="ttyak-render-")
            self.addCleanup(shutil.rmtree, out)
            render.render_all(version, text, out, DATE, IDENTITY)
            with open(os.path.join(out, "latest.json")) as f:
                self.assertEqual(sorted(json.load(f)["assets"]), sorted(PLATFORMS), version)
        tar = SHA["x86_64-linux"] + "  ttyak-0.11.0-x86_64-linux.tar.gz\n"
        self.assertEqual(render.parse_sums(sums() + tar, V), SHA, "not a zip: another file")

    def test_out_must_be_empty(self):
        with open(os.path.join(self.out, "stale.txt"), "w") as f:
            f.write("x")
        with self.assertRaises(render.Refused) as cm:
            render.render_all(V, sums(), self.out, DATE, IDENTITY)
        self.assertIn("not empty", str(cm.exception))
        self.assertEqual(os.listdir(self.out), ["stale.txt"], "nothing written, nothing removed")

    def test_sums_forms_accepted(self):
        star = "".join("%s *ttyak-%s-%s.zip\r\n" % (SHA[p].upper(), V, p) for p in PLATFORMS) + "\n"
        self.assertEqual(render.parse_sums(star, V), SHA, "binary-mode `*`, CRLF, upper-case hex, a blank line")

    def test_missing_required_platform(self):
        for gone in ["x86_64-linux", "x86_64-macos", "aarch64-macos", "x86_64-windows"]:
            self.refused(V, sums([p for p in PLATFORMS if p != gone]), "no archive for " + gone)

    def test_bad_date(self):
        for bad in ["2026-13-01", "2026-11-1", "26-11-01", "today", "2026-02-30"]:
            with self.assertRaises(render.Refused):
                render.render_all(V, sums(), self.out, bad, IDENTITY)
        self.assertEqual(os.listdir(self.out), [])

    def test_identity_with_a_newline_or_missing_field(self):
        for broken in [dict(IDENTITY, name="A\nB"), {k: v for k, v in IDENTITY.items() if k != "email"},
                       dict(IDENTITY, winget_id="nodot")]:
            with self.assertRaises(render.Refused):
                render.render_all(V, sums(), self.out, DATE, broken)
        self.assertEqual(os.listdir(self.out), [])


class Escaping(unittest.TestCase):
    HOSTILE = "A \"q\" 'b' #{x} $(y) `z` \\ <&> ‘c’ ${d}"

    def test_quotes_per_format(self):
        q = render.quote
        self.assertEqual(q("ruby", 'a"b\\c#{d}'), '"a\\"b\\\\c\\#{d}"')
        self.assertEqual(q("shell", "it's $x"), "'it'\\''s $x'")
        self.assertEqual(q("powershell", "it's ‘x’ $y"), "'it''s ‘‘x’’ $y'")
        self.assertEqual(q("xml", "a<b>&\"c'"), "a&lt;b&gt;&amp;&quot;c&apos;")
        self.assertEqual(q("yaml", 'a: "b" #c'), '"a: \\"b\\" #c"')
        self.assertEqual(q("json", 'a"b'), '"a\\"b"')
        self.assertEqual(q("shell", ["x86_64", "a'b"]), "'x86_64' 'a'\\''b'")

    def test_control_characters_refused(self):
        for fmt in render.FORMATS:
            for bad in ["a\nb", "a\rb", "a\x00b", "a\x1bb", "a\u2028b"]:
                with self.assertRaises(render.Refused):
                    render.quote(fmt, bad)

    def test_hostile_identity_stays_data(self):
        """A name full of quotes and expansions renders as data in every format."""
        r = Rendered(identity=dict(IDENTITY, name=self.HOSTILE))
        try:
            ns = {"n": "http://schemas.microsoft.com/packaging/2015/06/nuspec.xsd"}
            meta = ET.fromstring(r.read("chocolatey/ttyak.nuspec").encode()).find("n:metadata", ns)
            self.assertEqual(meta.find("n:authors", ns).text, self.HOSTILE)
            locale = r.read(WINGET_DIR + "/dmytrobsp.ttyak.locale.en-US.yaml")
            line = next(l for l in locale.splitlines() if l.startswith("Publisher: "))
            self.assertEqual(json.loads(line[len("Publisher: "):]), self.HOSTILE)
            nfpm = r.read("nfpm.yaml")
            line = next(l for l in nfpm.splitlines() if l.startswith("maintainer: "))
            self.assertEqual(json.loads(line[len("maintainer: "):]), "%s <%s>" % (self.HOSTILE, IDENTITY["email"]))
            pb = r.read("aur/PKGBUILD")
            self.assertTrue(pb.startswith("# Maintainer: %s <%s>\n" % (self.HOSTILE, IDENTITY["email"])), "a comment line holds it")
            if shutil.which("bash"):
                out = subprocess.run(["bash", "-c", 'source "$1" && printf %s "$pkgver"', "-", os.path.join(r.dir, "aur", "PKGBUILD")],
                                     capture_output=True, text=True, check=True)
                self.assertEqual(out.stdout, V)
        finally:
            r.close()


class Templates(unittest.TestCase):
    def test_placeholders_and_blocks(self):
        t = "a {{x}}\n{{if aarch64-linux}}\nb {{y}}\n{{end}}\n# c {{x:raw}}\n"
        vals = {"x": "1'2", "y": "3"}
        self.assertEqual(render.fill(t, "shell", vals, {"aarch64-linux"}), "a '1'\\''2'\nb '3'\n# c 1'2\n")
        self.assertEqual(render.fill(t, "shell", vals, set()), "a '1'\\''2'\n# c 1'2\n")

    def test_unless_blocks(self):
        t = "{{unless aarch64-linux}}\nonly x86\n{{end}}\nboth\n"
        self.assertEqual(render.fill(t, "ruby", {}, set()), "only x86\nboth\n")
        self.assertEqual(render.fill(t, "ruby", {}, {"aarch64-linux"}), "both\n")

    def test_unknown_placeholder_or_unbalanced_block(self):
        for t in ["{{nope}}\n", "{{if aarch64-linux}}\nx\n", "x\n{{end}}\n", "{{if mars}}\nx\n{{end}}\n", "{{unless mars}}\nx\n{{end}}\n", "{{if aarch64-linux}}\n{{unless aarch64-linux}}\n{{end}}\n{{end}}\n",
                  "a {{x:raw}}\n", "{{x:upper}}\n"]:
            with self.assertRaises(render.Refused, msg=t):
                render.fill(t, "shell", {"x": "1"}, set())

    def test_every_template_used(self):
        names = sorted(os.listdir(os.path.join(HERE, "templates")))
        self.assertEqual(names, sorted(t for t, _, _ in render.TEMPLATES))


class Cli(unittest.TestCase):
    def run_cli(self, *args):
        return subprocess.run([sys.executable, os.path.join(HERE, "render.py")] + list(args), capture_output=True, text=True)

    def test_renders(self):
        d = tempfile.mkdtemp(prefix="ttyak-render-")
        self.addCleanup(shutil.rmtree, d)
        with open(os.path.join(d, "SHA256SUMS"), "w") as f:
            f.write(sums())
        r = self.run_cli("--version", V, "--sums", os.path.join(d, "SHA256SUMS"), "--out", os.path.join(d, "out"), "--date", DATE)
        self.assertEqual((r.returncode, r.stderr), (0, ""))
        self.assertIn("latest.json", r.stdout)
        with open(os.path.join(d, "out", "latest.json")) as f:
            self.assertEqual(json.load(f)["date"], DATE)
        r = self.run_cli("--version", V, "--sums", os.path.join(d, "SHA256SUMS"), "--out", os.path.join(d, "today"))
        self.assertEqual(r.returncode, 0, r.stderr)
        with open(os.path.join(d, "today", "latest.json")) as f:
            date = json.load(f)["date"]
        import datetime
        self.assertIn(date, {datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d"),
                             (datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(minutes=1)).strftime("%Y-%m-%d")})

    def test_refuses_with_a_message(self):
        d = tempfile.mkdtemp(prefix="ttyak-render-")
        self.addCleanup(shutil.rmtree, d)
        with open(os.path.join(d, "SHA256SUMS"), "w") as f:
            f.write(sums())
        r = self.run_cli("--version", "v0.11.0", "--sums", os.path.join(d, "SHA256SUMS"), "--out", os.path.join(d, "out"))
        self.assertEqual(r.returncode, 1)
        self.assertIn("not a release version", r.stderr)
        self.assertFalse(os.path.exists(os.path.join(d, "out")))
        r = self.run_cli("--version", V, "--sums", os.path.join(d, "missing"), "--out", os.path.join(d, "out"))
        self.assertEqual(r.returncode, 1)
        self.assertIn("missing", r.stderr)


def podman_tests(env=None):
    """Whether the podman-based tests run: when podman is there, and in CI (env CI set) only with
    TTYAK_PODMAN_TESTS=1."""
    env = os.environ if env is None else env
    return bool(shutil.which("podman")) and (not env.get("CI") or env.get("TTYAK_PODMAN_TESTS") == "1")


class PodmanSwitch(unittest.TestCase):
    def test_ci_runs_them_only_when_asked(self):
        if not shutil.which("podman"):
            self.skipTest("needs podman on PATH")
        self.assertTrue(podman_tests({}))
        self.assertFalse(podman_tests({"CI": "true"}))
        self.assertTrue(podman_tests({"CI": "true", "TTYAK_PODMAN_TESTS": "1"}))


@unittest.skipUnless(podman_tests(), "needs podman (makepkg runs in an archlinux container); in CI only with TTYAK_PODMAN_TESTS=1")
class SrcinfoMatchesMakepkg(unittest.TestCase):
    def test_byte_identical(self):
        for platforms in [PLATFORMS, [p for p in PLATFORMS if p != "aarch64-linux"]]:
            r = Rendered(platforms)
            try:
                aur = os.path.join(r.dir, "aur")
                out = subprocess.run(
                    ["podman", "run", "--rm", "-v", aur + ":/w:ro,Z", "-w", "/w", "--user", "65534:65534",
                     "-e", "HOME=/tmp", "-e", "BUILDDIR=/tmp/b", "-e", "SRCDEST=/tmp/s", "-e", "PKGDEST=/tmp/p",
                     ARCH_IMAGE, "makepkg", "--printsrcinfo"], capture_output=True, text=True, timeout=600)
                self.assertEqual(out.returncode, 0, out.stderr)
                self.assertEqual(out.stdout, r.read("aur/.SRCINFO"))
            finally:
                r.close()


if __name__ == "__main__":
    unittest.main()

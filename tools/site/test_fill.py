import copy
import io
import json
import subprocess
import sys
import tempfile
import unittest
import urllib.error
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import fill as F  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
RELEASE = json.loads((ROOT / "site/data/release.json").read_text())
LINUX = "https://github.com/dmytrobsp/klack/releases/download/v0.8.0/klack-0.8.0-x86_64-linux.zip"


class Resp(io.BytesIO):
    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False


class FillTests(unittest.TestCase):
    def test_fill_replaces_only_between_markers_and_is_idempotent(self):
        html = "a<!-- fill:x -->old<!-- /fill:x -->b<!-- fill:y -->keep<!-- /fill:y -->"
        once = F.fill(html, "x", "NEW")
        self.assertEqual(once, "a<!-- fill:x -->NEW<!-- /fill:x -->b<!-- fill:y -->keep<!-- /fill:y -->")
        self.assertEqual(F.fill(once, "x", "NEW"), once)

    def test_fill_missing_marker(self):
        with self.assertRaises(KeyError) as c:
            F.fill("nothing", "download-button", "x")
        self.assertIn("download-button", str(c.exception))

    def test_fmt_size(self):
        self.assertEqual(F.fmt_size(12667135), "12.7 MB")
        self.assertEqual(F.fmt_size(999), "0.0 MB")

    def test_button(self):
        h = F.render_download_button(RELEASE)
        self.assertIn(f'<a class="dl" data-os="linux" href="{LINUX}">DOWNLOAD FOR LINUX</a>', h)
        self.assertIn("v0.8.0 · 12.7 MB · ", h)
        self.assertIn('<span class="dl-others">', h)
        self.assertIn("klack-0.8.0-aarch64-macos.zip\">macOS</a>", h)
        self.assertIn("klack-0.8.0-x86_64-windows.zip\">Windows</a>", h)
        self.assertIn('<a href="#get">all downloads</a>', h)

    NEEDS = {"x86_64-linux": ("glibc 2.34+", "libasound <b>"), }

    def test_downloads_order_and_unknown_last(self):
        r = copy.deepcopy(RELEASE)
        r["assets"].insert(0, {"name": "klack-0.8.0-riscv64-linux.zip", "platform": "riscv64-linux",
                               "url": "https://example.test/r.zip", "size": 5_000_000})
        h = F.render_downloads(r, self.NEEDS)
        pos = [h.index(p) for p in ("x86_64-linux.zip", "aarch64-macos.zip", "x86_64-macos.zip",
                                    "x86_64-windows.zip", "riscv64-linux.zip")]
        self.assertEqual(pos, sorted(pos))
        self.assertIn("12.7 MB", h)
        self.assertIn("glibc 2.34+", h)
        self.assertIn("libasound &lt;b&gt;", h)
        self.assertIn(RELEASE["sums_url"], h)
        self.assertIn("SHA256SUMS", h)

    def test_readme_needs(self):
        text = (ROOT / "README.md").read_text()
        n = F.readme_needs(text)
        self.assertEqual(set(n), {"x86_64-linux", "aarch64-macos", "x86_64-macos", "x86_64-windows"})
        runs, needs = n["x86_64-linux"]
        self.assertIn("glibc 2.34", runs)
        self.assertNotIn("`", needs)
        self.assertIn("libasound.so.2", needs)

    def test_readme_needs_strips_links(self):
        t = "## Install\n\n| Download | Runs on | Needs |\n|---|---|---|\n| `a-b.zip` | [x](http://u) y | `z` |\n\n## Next\n"
        self.assertEqual(F.readme_needs(t), {"a-b": ("x y", "z")})

    def _api(self, status=200, body=None):
        def opener(req, timeout=None):
            self.last = req
            if status != 200:
                raise urllib.error.HTTPError(req.full_url, status, "boom", {}, None)
            return Resp(json.dumps(body).encode())
        return opener

    def test_api_500(self):
        with self.assertRaises(F.ReleaseFetchError):
            F.release_from_api("o/r", None, urlopen=self._api(500))

    def test_api_ok_and_token(self):
        base = "https://github.com/o/r/releases/download/v1.2.3/"
        body = {"tag_name": "v1.2.3", "html_url": "https://github.com/o/r/releases/tag/v1.2.3", "assets": [
            {"name": "klack-1.2.3-x86_64-linux.zip", "browser_download_url": base + "klack-1.2.3-x86_64-linux.zip", "size": 7},
            {"name": "SHA256SUMS", "browser_download_url": base + "SHA256SUMS", "size": 1}]}
        r = F.release_from_api("o/r", "tok", urlopen=self._api(200, body))
        self.assertEqual(self.last.get_header("Authorization"), "Bearer tok")
        self.assertEqual(r, {"version": "1.2.3", "url": body["html_url"], "sums_url": base + "SHA256SUMS",
                             "assets": [{"name": "klack-1.2.3-x86_64-linux.zip", "platform": "x86_64-linux",
                                         "url": base + "klack-1.2.3-x86_64-linux.zip", "size": 7}]})

    def test_fill_page(self):
        with tempfile.TemporaryDirectory() as d:
            site = Path(d)
            (site / "data").mkdir()
            (site / "index.html").write_text("<!-- fill:download-button -->x<!-- /fill:download-button --><!-- fill:downloads --><!-- /fill:downloads -->")
            (site / "data/release.json").write_text(json.dumps(RELEASE))
            F.fill_page(site)
            out = (site / "index.html").read_text()
            self.assertIn(LINUX, out)
            self.assertIn("SHA256SUMS", out)
            F.fill_page(site)
            self.assertEqual((site / "index.html").read_text(), out)

    def _capture_site(self, d, names):
        site = Path(d)
        (site / "data").mkdir()
        (site / "data/release.json").write_text(json.dumps(RELEASE))
        marks = "".join(f"<!-- fill:{m} -->old<!-- /fill:{m} -->" for m in
                        ["download-button", "downloads", "hero-capture", "capture-huddle", "capture-signin",
                         "capture-keys", "capture-ssh"])
        (site / "index.html").write_text(marks)
        (site / "captures").mkdir()
        for n in names:
            (site / "captures" / f"{n}.html").write_text(f"<div>{n} capture</div>\n")
        return site

    def test_fill_page_puts_in_every_capture(self):
        with tempfile.TemporaryDirectory() as d:
            site = self._capture_site(d, ["hero", "huddle", "signin", "keys", "ssh"])
            F.fill_page(site)
            out = (site / "index.html").read_text()
            self.assertIn("<!-- fill:hero-capture --><div>hero capture</div><!-- /fill:hero-capture -->", out)
            for n in ("huddle", "signin", "keys", "ssh"):
                self.assertIn(f"<!-- fill:capture-{n} --><div>{n} capture</div><!-- /fill:capture-{n} -->", out)
            F.fill_page(site)
            self.assertEqual((site / "index.html").read_text(), out)

    def test_fill_page_keeps_a_region_without_its_capture(self):
        with tempfile.TemporaryDirectory() as d:
            site = self._capture_site(d, ["keys"])
            F.fill_page(site)
            out = (site / "index.html").read_text()
            self.assertIn("<!-- fill:capture-keys --><div>keys capture</div><!-- /fill:capture-keys -->", out)
            self.assertIn("<!-- fill:hero-capture -->old<!-- /fill:hero-capture -->", out)
            self.assertIn("<!-- fill:capture-ssh -->old<!-- /fill:capture-ssh -->", out)

    def test_a_capture_without_its_region_is_an_error(self):
        with tempfile.TemporaryDirectory() as d:
            site = self._capture_site(d, ["ssh"])
            (site / "index.html").write_text("<!-- fill:download-button --><!-- /fill:download-button -->"
                                             "<!-- fill:downloads --><!-- /fill:downloads -->")
            with self.assertRaises(KeyError) as c:
                F.fill_page(site)
            self.assertIn("capture-ssh", str(c.exception))

    def test_the_site_has_all_five_captures(self):
        for n in ("hero", "huddle", "signin", "keys", "ssh"):
            t = (ROOT / "site/captures" / f"{n}.html").read_text()
            self.assertTrue(t.startswith('<div class="term capture"'), n)
            self.assertIn("<pre>", t)

    def test_cli_failed_fetch_keeps_file(self):
        with tempfile.TemporaryDirectory() as d:
            site = Path(d)
            (site / "data").mkdir()
            (site / "index.html").write_text("<!-- fill:download-button --><!-- /fill:download-button --><!-- fill:downloads --><!-- /fill:downloads -->")
            raw = (ROOT / "site/data/release.json").read_bytes()
            (site / "data/release.json").write_bytes(raw)
            def boom(req, timeout=None):
                raise urllib.error.URLError("offline")
            err = io.StringIO()
            code = F.main([str(site), "--fetch-release"], urlopen=boom, stderr=err, readme=ROOT / "README.md")
            self.assertEqual(code, 0)
            self.assertIn("release fetch failed:", err.getvalue())
            self.assertIn("keeping site/data/release.json", err.getvalue())
            self.assertEqual((site / "data/release.json").read_bytes(), raw)


if __name__ == "__main__":
    unittest.main()

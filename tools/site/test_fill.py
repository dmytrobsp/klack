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
VER = RELEASE["version"]
LINUX = next(a["url"] for a in RELEASE["assets"] if a["platform"] == "x86_64-linux")
LINUX_SIZE = next(a["size"] for a in RELEASE["assets"] if a["platform"] == "x86_64-linux")


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
        self.assertIn(f"v{VER} · {F.fmt_size(LINUX_SIZE)} · ", h)
        self.assertIn('<span class="dl-others">', h)
        self.assertIn(f"-{VER}-aarch64-macos.zip\">macOS</a>", h)
        self.assertIn(f"-{VER}-x86_64-windows.zip\">Windows</a>", h)
        self.assertIn('<a href="#get">all downloads</a>', h)

    NEEDS = {"x86_64-linux": ("glibc 2.34+", "libasound <b>"), }

    def test_downloads_order_and_unknown_last(self):
        r = copy.deepcopy(RELEASE)
        r["assets"].insert(0, {"name": f"ttyak-{VER}-riscv64-linux.zip", "platform": "riscv64-linux",
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
            {"name": "ttyak-1.2.3-x86_64-linux.zip", "browser_download_url": base + "ttyak-1.2.3-x86_64-linux.zip", "size": 7},
            {"name": "SHA256SUMS", "browser_download_url": base + "SHA256SUMS", "size": 1}]}
        r = F.release_from_api("o/r", "tok", urlopen=self._api(200, body))
        self.assertEqual(self.last.get_header("Authorization"), "Bearer tok")
        self.assertEqual(r, {"version": "1.2.3", "url": body["html_url"], "sums_url": base + "SHA256SUMS",
                             "assets": [{"name": "ttyak-1.2.3-x86_64-linux.zip", "platform": "x86_64-linux",
                                         "url": base + "ttyak-1.2.3-x86_64-linux.zip", "size": 7}]})
        self.assertEqual(self.last.get_header("User-agent"), "ttyak-site")

    def test_the_repo(self):
        self.assertEqual(F.REPO, "dmytrobsp/ttyak")

    def _release_of(self, version, names):
        base = f"https://github.com/o/r/releases/download/v{version}/"
        body = {"tag_name": f"v{version}", "html_url": f"https://github.com/o/r/releases/tag/v{version}",
                "assets": [{"name": n, "browser_download_url": base + n, "size": 7_000_000} for n in names]
                + [{"name": "SHA256SUMS", "browser_download_url": base + "SHA256SUMS", "size": 1}]}
        return base, F.release_from_api("o/r", None, urlopen=self._api(200, body))

    def test_assets_of_the_old_name_do_not_link(self):
        # only ttyak-<version>-<platform>.zip counts; a release with nothing else has no downloads
        old = "k" + "lack"
        with self.assertRaises(F.ReleaseFetchError):
            self._release_of("0.9.0", [f"{old}-0.9.0-x86_64-linux.zip", f"{old}-0.9.0-x86_64-windows.zip"])

    def test_assets_of_new_name_link(self):
        base, r = self._release_of("0.10.0", ["ttyak-0.10.0-x86_64-linux.zip", "ttyak-0.10.0-aarch64-linux.zip",
                                              "ttyak-0.9.0-x86_64-macos.zip", "ttyak_0.10.0_amd64.deb"])
        self.assertEqual([a["platform"] for a in r["assets"]], ["x86_64-linux", "aarch64-linux"])
        button, table = F.render_download_button(r), F.render_downloads(r, {})
        self.assertIn(f'href="{base}ttyak-0.10.0-x86_64-linux.zip">DOWNLOAD FOR LINUX</a>', button)
        self.assertIn(f'href="{base}ttyak-0.10.0-aarch64-linux.zip">aarch64-linux</a>', table)
        self.assertNotIn("old-name", button + table)

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
                         "capture-keys"])
        (site / "index.html").write_text(marks)
        (site / "captures").mkdir()
        for n in names:
            (site / "captures" / f"{n}.html").write_text(f"<div>{n} capture</div>\n")
        return site

    def test_fill_page_puts_in_every_capture(self):
        with tempfile.TemporaryDirectory() as d:
            site = self._capture_site(d, ["hero", "huddle", "signin", "keys"])
            F.fill_page(site)
            out = (site / "index.html").read_text()
            self.assertIn("<!-- fill:hero-capture --><div>hero capture</div><!-- /fill:hero-capture -->", out)
            for n in ("huddle", "signin", "keys"):
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
            self.assertIn("<!-- fill:capture-signin -->old<!-- /fill:capture-signin -->", out)

    def test_a_capture_without_its_region_is_an_error(self):
        with tempfile.TemporaryDirectory() as d:
            site = self._capture_site(d, ["signin"])
            (site / "index.html").write_text("<!-- fill:download-button --><!-- /fill:download-button -->"
                                             "<!-- fill:downloads --><!-- /fill:downloads -->")
            with self.assertRaises(KeyError) as c:
                F.fill_page(site)
            self.assertIn("capture-signin", str(c.exception))

    def test_the_site_has_all_four_captures(self):
        for n in ("hero", "huddle", "signin", "keys"):
            t = (ROOT / "site/captures" / f"{n}.html").read_text()
            self.assertTrue(t.startswith('<div class="term capture"'), n)
            self.assertIn('<svg class="cells"', t)

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

    def test_cli_strict_failed_fetch_exits_nonzero_and_writes_nothing(self):
        with tempfile.TemporaryDirectory() as d:
            site = Path(d)
            (site / "data").mkdir()
            page = "<!-- fill:download-button -->old<!-- /fill:download-button --><!-- fill:downloads --><!-- /fill:downloads -->"
            (site / "index.html").write_text(page)
            raw = (ROOT / "site/data/release.json").read_bytes()
            (site / "data/release.json").write_bytes(raw)
            def boom(req, timeout=None):
                raise urllib.error.URLError("offline")
            err = io.StringIO()
            code = F.main([str(site), "--fetch-release", "--strict"], urlopen=boom, stderr=err, readme=ROOT / "README.md")
            self.assertNotEqual(code, 0)
            self.assertIn("release fetch failed:", err.getvalue())
            self.assertEqual((site / "index.html").read_text(), page)
            self.assertEqual((site / "data/release.json").read_bytes(), raw)


NUMBERS = {
    "measured_on": "2026-01-02",
    "machine": "Some CPU (8 cores), 16 GB RAM, Some Linux (Linux 6.1)",
    "versions": {"ttyak": "9.9.9", "slack": "1.2.3"},
    "rows": [
        {"id": "download", "label": "download", "unit": "bytes", "ttyak": 12667135, "slack": 95292754},
        {"id": "disk", "label": "on disk", "unit": "bytes", "ttyak": 30099064, "slack": 314438634},
        {"id": "startup", "label": "startup", "unit": "seconds", "ttyak": 1.69, "slack": 1.85},
        {"id": "ram_rest", "label": "RAM <at> rest", "unit": "kB", "ttyak": 32558, "slack": 1500000},
        {"id": "cpu_idle", "label": "CPU at rest", "unit": "percent", "ttyak": 0.63, "slack": 0.45},
        {"id": "cpu_share", "label": "CPU sharing", "unit": "percent", "ttyak": 18.53, "slack": 19.4},
        {"id": "vram", "label": "video memory", "unit": "kB", "ttyak": 0, "slack": 124564},
        {"id": "boot", "label": "cold boot", "unit": "seconds", "ttyak": 12.34, "slack": 40.0},
    ],
}


def rows_of(h):
    return h.split('<tr class="row')[1:]


class SheetTests(unittest.TestCase):
    def setUp(self):
        self.h = F.render_sheet(NUMBERS)
        self.rows = rows_of(self.h)

    def test_eight_rows_in_order_with_both_columns(self):
        self.assertEqual(len(self.rows), 8)
        self.assertIn("the official app", self.h)
        self.assertIn("ttyak", self.h)
        pos = [self.h.index(f'data-id="{r["id"]}"') for r in NUMBERS["rows"]]
        self.assertEqual(pos, sorted(pos))
        self.assertIn("RAM &lt;at&gt; rest", self.h)

    def test_units(self):
        self.assertEqual(F.fmt_value(12667135, "bytes"), "12.7 MB")
        self.assertEqual(F.fmt_value(314438634, "bytes"), "314.4 MB")
        self.assertEqual(F.fmt_value(1015882000, "bytes"), "1.0 GB")
        self.assertEqual(F.fmt_value(32558, "kB"), "32.6 MB")
        self.assertEqual(F.fmt_value(999949, "kB"), "999.9 MB")
        self.assertEqual(F.fmt_value(1015882, "kB"), "1.0 GB")
        self.assertEqual(F.fmt_value(1500000, "kB"), "1.5 GB")
        self.assertEqual(F.fmt_value(0.45, "percent"), "0.5%")
        self.assertEqual(F.fmt_value(15.83, "percent"), "15.8%")
        self.assertEqual(F.fmt_value(1.69, "seconds"), "1.69 s")
        self.assertEqual(F.fmt_value(12.34, "seconds"), "12.3 s")
        self.assertEqual(F.fmt_value(0, "kB"), "none")
        with self.assertRaises(ValueError):
            F.fmt_value(1, "furlongs")

    def test_ratio_text(self):
        self.assertEqual(F.fmt_ratio(95292754 / 12667135), "7.5")
        self.assertEqual(F.fmt_ratio(8.44), "8.4")
        self.assertEqual(F.fmt_ratio(19.33), "19")
        self.assertEqual(F.fmt_ratio(9.97), "10")

    def test_win_crossed_circled_with_ratio(self):
        r = self.rows[0]
        self.assertIn('<span class="crossed">95.3 MB</span>', r)
        self.assertIn('<span class="circled">12.7 MB</span>', r)
        self.assertIn("7.5× less", r)
        self.assertIn("10× less", self.rows[1])
        self.assertIn('<span class="crossed">1.5 GB</span>', self.rows[3])
        self.assertIn("46× less", self.rows[3])
        self.assertIn("3.2× faster", self.rows[7])
        self.assertIn('<span class="crossed">40.0 s</span>', self.rows[7])

    def test_near_tie_no_marks_and_weekend_scrawl(self):
        r = self.rows[2]
        self.assertIn("1.69 s", r)
        self.assertIn("1.85 s", r)
        self.assertNotIn("crossed", r)
        self.assertNotIn("circled", r)
        self.assertNotIn("×", r)
        self.assertIn('<span class="scrawl">a draw. not bad for a weekend.</span>', r)
        share = self.rows[5]
        self.assertNotIn("crossed", share)
        self.assertNotIn("×", share)
        self.assertNotIn("weekend", share)

    def test_default_tie_scrawl(self):
        h = F.render_sheet({**NUMBERS, "rows": [
            {"id": "other", "label": "x", "unit": "percent", "ttyak": 10.0, "slack": 12.0}]})
        self.assertIn("ok, a tie. it's a chat app.", h)
        self.assertNotIn("crossed", h)

    def test_loss_is_honest(self):
        r = self.rows[4]
        self.assertIn("0.63%", r)  # under 1 %: two decimals, so the gap isn't rounded away
        self.assertIn("0.45%", r)
        self.assertNotIn("crossed", r)
        self.assertNotIn("circled", r)
        self.assertIn('<span class="scrawl ratio more">1.4× more CPU</span>'
                      '<span class="scrawl">yeah, okay. now look at the next one</span>', r)  # how much, then the user's line
        other = F.render_sheet({**NUMBERS, "rows": [
            {"id": "other", "label": "x", "unit": "percent", "ttyak": 0.63, "slack": 0.45}]})
        self.assertIn('<span class="scrawl">fine, this one\'s theirs.</span>', other)

    def test_the_next_one_is_a_win(self):
        # "now look at the next one" points at the row after CPU at rest: it has to be there and be a win
        real = json.loads((ROOT / "site/data/numbers.json").read_text())["rows"]
        i = [r["id"] for r in real].index("cpu_idle")
        self.assertLess(i + 1, len(real))
        self.assertEqual(F.classify(real[i + 1]), "win")

    def test_small_percent_rows_use_two_decimals_on_both_sides(self):
        self.assertEqual(F.fmt_value(0.45, "percent", small=True), "0.45%")
        self.assertEqual(F.fmt_value(15.83, "percent", small=True), "15.83%")
        h = F.render_sheet({**NUMBERS, "rows": [
            {"id": "other", "label": "x", "unit": "percent", "ttyak": 0.8, "slack": 15.83}]})
        self.assertIn('<span class="circled">0.80%</span>', h)
        self.assertIn('<span class="crossed">15.83%</span>', h)
        self.assertIn("18.5%", self.rows[5])  # both sides at or above 1 %: one decimal
        self.assertIn("19.4%", self.rows[5])
        self.assertNotIn("18.53%", self.rows[5])

    def test_classify(self):
        row = lambda k, o: {"id": "x", "label": "x", "unit": "percent", "ttyak": k, "slack": o}
        self.assertEqual(F.classify(row(0, 5)), "zero")
        self.assertEqual(F.classify(row(1, 1.5)), "win")
        self.assertEqual(F.classify(row(1, 1.49)), "tie")
        self.assertEqual(F.classify(row(1, 1)), "tie")
        self.assertEqual(F.classify(row(0, 0)), "tie")
        self.assertEqual(F.classify(row(1.01, 1)), "loss")
        self.assertEqual(F.classify(row(1, 0)), "loss")

    def test_bespoke_scrawls_only_in_their_own_class(self):
        def one(id, k, o):
            return F.render_sheet({**NUMBERS, "rows": [
                {"id": id, "label": "x", "unit": "seconds", "ttyak": k, "slack": o}]})
        lost_startup = one("startup", 2.0, 1.8)  # the startup joke is for a draw, not a loss
        self.assertIn("fine, this one's theirs.", lost_startup)
        self.assertNotIn("weekend", lost_startup)
        tied_idle = one("cpu_idle", 1.0, 1.2)  # the fidget joke is for a loss, not a draw
        self.assertIn("ok, a tie. it's a chat app.", tied_idle)
        self.assertNotIn("next one", tied_idle)
        won_idle = one("cpu_idle", 1.0, 3.0)
        self.assertIn("3.0× faster", won_idle)
        self.assertNotIn("next one", won_idle)

    def test_cpu_rows_lead_with_the_ratio(self):
        h = F.render_sheet({**NUMBERS, "rows": [
            {"id": "cpu_scroll", "label": "x", "unit": "percent", "ttyak": 1.92, "slack": 15.83}]})
        self.assertIn('<span class="scrawl ratio">8.2× less CPU</span>', h)
        self.assertIn('<td class="them evidence" data-side="the official app"><span class="crossed">15.8%</span>', h)
        self.assertIn('<td class="us evidence" data-side="ttyak"><span class="circled">1.9%</span>', h)
        share = self.rows[5]  # a CPU draw: small cells, its own scrawl, no ratio
        self.assertIn('class="them evidence"', share)
        self.assertIn('class="us evidence"', share)
        self.assertNotIn("×", share)
        self.assertIn("video costs what it costs", share)
        far = F.render_sheet({**NUMBERS, "rows": [
            {"id": "other", "label": "x", "unit": "percent", "ttyak": 5.0, "slack": 0}]})
        self.assertNotIn("×", far)  # a loss against zero states no ratio
        self.assertIn("theirs", far)

    def test_other_rows_unchanged_by_the_cpu_rule(self):
        for i in (0, 1, 2, 3, 6, 7):
            self.assertNotIn("evidence", self.rows[i])
            self.assertNotIn("CPU<", self.rows[i])
            self.assertNotIn("more", self.rows[i])
        self.assertIn('<span class="scrawl ratio">7.5× less</span>', self.rows[0])
        lost = F.render_sheet({**NUMBERS, "rows": [
            {"id": "other", "label": "x", "unit": "kB", "ttyak": 9000, "slack": 1000}]})
        self.assertNotIn("×", lost)

    def test_loss_even_when_far_behind(self):
        h = F.render_sheet({**NUMBERS, "rows": [
            {"id": "other", "label": "x", "unit": "kB", "ttyak": 9000, "slack": 1000}]})
        self.assertNotIn("crossed", h)
        self.assertNotIn("circled", h)
        self.assertIn("theirs", h)

    def test_zero_is_none_without_a_ratio(self):
        r = self.rows[6]
        self.assertIn('<span class="crossed">124.6 MB</span>', r)
        self.assertIn('<span class="circled">none</span>', r)
        self.assertIn("it's text.", r)
        self.assertNotIn("×", r)

    def test_empty_rows_keep_the_placeholder(self):
        h = F.render_sheet({**NUMBERS, "rows": []})
        self.assertIn("measuring…", h)
        self.assertIn('class="standin-note"', h)
        self.assertNotRegex(h, r"\d")
        self.assertEqual(F.render_measured_note({**NUMBERS, "rows": []}), "")

    def test_no_draft_comments(self):
        self.assertNotIn("<!-- draft -->", self.h)
        self.assertNotIn("<!-- draft -->", F.render_sheet({**NUMBERS, "rows": []}))

    def test_note(self):
        n = F.render_measured_note(NUMBERS)
        for s in ("Linux", "measured on Linux", "9.9.9", "1.2.3", "2026-01-02", "Some CPU (8 cores)",
                  "App against app", "the terminal ttyak runs in isn't counted",
                  "https://github.com/dmytrobsp/ttyak/blob/master/tools/site/MEASURING.md",
                  "ttyak (version 9.9.9) against Slack's desktop app 1.2.3", "Slack's RAM figures are a lower bound"):
            self.assertIn(s, n, s)
        self.assertNotRegex(n.lower(), r"workspace|\bteam\b|channel")
        self.assertNotIn("old-name", n)

    def test_note_of_an_older_build_names_no_old_name(self):
        n = F.render_measured_note({**NUMBERS, "versions": {"ttyak": "0.8.0", "slack": "1.2.3"}})
        self.assertIn("ttyak (version 0.8.0) against Slack's desktop app 1.2.3", n)
        self.assertNotIn("released as", n)
        self.assertNotRegex(n.lower(), "k" + "lack|old-name")
        self.assertFalse(n.endswith("-->"), n)

    def test_real_numbers_render(self):
        real = json.loads((ROOT / "site/data/numbers.json").read_text())
        h = F.render_sheet(real)
        self.assertEqual(len(rows_of(h)), len(real["rows"]))
        self.assertIn('<span class="circled">none</span>', h)
        n = F.render_measured_note(real)
        self.assertIn("measured on Linux", n)
        self.assertNotRegex(n.lower(), r"workspace|\bteam\b|channel")

    def test_fill_page_fills_sheet_and_note(self):
        with tempfile.TemporaryDirectory() as d:
            site = Path(d)
            (site / "data").mkdir()
            (site / "data/release.json").write_text(json.dumps(RELEASE))
            (site / "data/numbers.json").write_text(json.dumps(NUMBERS))
            (site / "index.html").write_text("".join(f"<!-- fill:{m} -->old<!-- /fill:{m} -->" for m in
                                                     ["download-button", "downloads", "sheet", "measured-note"]))
            F.fill_page(site)
            out = (site / "index.html").read_text()
            self.assertIn("7.5× less", out)
            self.assertIn("measured on Linux", out)
            F.fill_page(site)
            self.assertEqual((site / "index.html").read_text(), out)
            (site / "data/numbers.json").write_text(json.dumps({**NUMBERS, "rows": []}))
            F.fill_page(site)
            out = (site / "index.html").read_text()
            self.assertIn("measuring…", out)
            self.assertNotIn("× less", out)
            self.assertIn("<!-- fill:measured-note --><!-- /fill:measured-note -->", out)


if __name__ == "__main__":
    unittest.main()

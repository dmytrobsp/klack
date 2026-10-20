import contextlib, io, os, subprocess, sys, tempfile, unittest
from pathlib import Path
HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
import measure
from measure import read_procs, tree, roots_matching, pss_kb, cpu_ticks, main


def add(proc, pid, ppid, comm, pss=None, ut=0, st=0, exe=None):
    d = proc / str(pid)
    d.mkdir()
    (d / "comm").write_text(comm + "\n")
    # fields after the comm: state ppid pgrp session tty tpgid flags minflt cminflt majflt cmajflt utime stime
    (d / "stat").write_text(f"{pid} ({comm}) S {ppid} 1 1 0 -1 0 0 0 0 0 {ut} {st} 0 0 20 0 1 0 5 1000 100\n")
    if pss is not None:
        (d / "smaps_rollup").write_text(f"00400000-ff Rss: 1 kB\nPss:            {pss} kB\nPss_Anon: 5 kB\n")
    if exe:
        os.symlink(exe, d / "exe")


class Fake(unittest.TestCase):
    def setUp(self):
        self.d = tempfile.TemporaryDirectory()
        self.p = Path(self.d.name)
        add(self.p, 10, 1, "slack", 100, 5, 5, "/app/slack")
        add(self.p, 11, 10, "slack", 200)
        add(self.p, 12, 10, "slack", 300)
        add(self.p, 13, 12, "zygote", 400)
        add(self.p, 20, 1, "other", 999)
        (self.p / "self").mkdir()  # non-numeric ignored

    def tearDown(self):
        self.d.cleanup()

    def test_tree_and_roots(self):
        procs = read_procs(self.p)
        self.assertEqual(set(procs), {10, 11, 12, 13, 20})
        self.assertEqual(procs[13].ppid, 12)
        self.assertEqual(procs[10].exe, "/app/slack")
        self.assertIsNone(procs[11].exe)
        self.assertEqual(roots_matching(procs, "slack"), {10})
        self.assertEqual(tree(procs, {10}), {10, 11, 12, 13})

    def test_comm_with_parens_and_spaces(self):
        add(self.p, 30, 10, "a) b (c")
        self.assertEqual(read_procs(self.p)[30].ppid, 10)
        self.assertEqual(read_procs(self.p)[30].comm, "a) b (c")

    def test_pss_sum_and_unreadable(self):
        self.assertEqual(pss_kb({10, 11, 12, 13}, self.p), 1000)
        (self.p / "11" / "smaps_rollup").unlink()
        self.assertEqual(pss_kb({10, 11, 12, 13}, self.p), 800)
        self.assertEqual(pss_kb({10, 999}, self.p), 100)

    def test_unreadable_count(self):
        (self.p / "11" / "smaps_rollup").unlink()
        self.assertEqual(measure.pss_detail({10, 11}, self.p), (100, 1))

    def test_cpu_ticks_delta(self):
        a = cpu_ticks({10, 11}, self.p)
        self.assertEqual(a, 10)
        add(self.p, 40, 10, "x", ut=7, st=1)
        (self.p / "10" / "stat").write_text("10 (slack) S 1 1 1 0 -1 0 0 0 0 0 9 6 0 0 20 0 1 0 5 1 1\n")
        self.assertEqual(cpu_ticks({10, 11}, self.p) - a, 5)

    def test_cpu_window_handles_comers_and_goers(self):
        before = measure.ticks_by_pid({10, 11, 12}, self.p)
        (self.p / "10" / "stat").write_text("10 (slack) S 1 1 1 0 -1 0 0 0 0 0 15 5 0 0 20 0 1 0 5 1 1\n")
        import shutil
        shutil.rmtree(self.p / "12")
        add(self.p, 41, 10, "new", ut=50)
        after = measure.ticks_by_pid({10, 11, 41}, self.p)
        delta, common, came, went = measure.window(before, after)
        self.assertEqual((delta, common, came, went), (10, 2, 1, 1))


class Cli(unittest.TestCase):
    def run_main(self, argv, proc):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            try:
                code = main(argv + ["--proc", str(proc)])
            except SystemExit as e:
                code = e.code
        return code, out.getvalue(), err.getvalue()

    def setUp(self):
        self.d = tempfile.TemporaryDirectory()
        self.p = Path(self.d.name)
        add(self.p, 10, 1, "slack", 100)
        add(self.p, 11, 10, "slack", 200)
        add(self.p, 50, 1, "slack", 7)

    def tearDown(self):
        self.d.cleanup()

    def test_ram_pid(self):
        import json
        code, out, _ = self.run_main(["ram", "--pid", "10"], self.p)
        j = json.loads(out)
        self.assertEqual((code, j["what"], j["pss_kb"], j["pids"], j["roots"]), (0, "ram", 300, 2, [10]))
        self.assertEqual(j["unreadable"], 0)

    def test_two_roots_refused(self):
        code, out, err = self.run_main(["ram", "--comm", "slack"], self.p)
        self.assertEqual(code, 2)
        self.assertEqual(out, "")
        self.assertIn("--all-roots", err)

    def test_all_roots(self):
        import json
        code, out, _ = self.run_main(["ram", "--comm", "slack", "--all-roots"], self.p)
        self.assertEqual(json.loads(out)["pss_kb"], 307)

    def test_no_match(self):
        code, _, err = self.run_main(["ram", "--comm", "nothing"], self.p)
        self.assertEqual(code, 2)

    def test_repeat(self):
        code, out, _ = self.run_main(["ram", "--pid", "10", "--repeat", "2", "--every", "0"], self.p)
        self.assertEqual(len(out.strip().splitlines()), 2)

    def test_cpu_zero_idle(self):
        import json
        code, out, _ = self.run_main(["cpu", "--pid", "10", "--seconds", "0.05"], self.p)
        j = json.loads(out)
        self.assertEqual((j["what"], j["percent_of_one_core"]), ("cpu", 0.0))
        self.assertEqual(j["came"], 0)


class Startup(unittest.TestCase):
    def test_until_file_and_line(self):
        import json
        d = tempfile.TemporaryDirectory()
        log = Path(d.name) / "log"
        cmd = ["sh", "-c", f"sleep 0.3; echo boot >> {log}; sleep 0.2; echo READY now >> {log}; sleep 30"]
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = main(["startup", "--until-file", str(log), "--until-line", "READY", "--timeout", "10", "--"] + cmd)
        j = json.loads(out.getvalue())
        os.kill(j["pid"], 15)
        self.assertEqual((code, j["what"]), (0, "startup"))
        self.assertGreater(j["seconds"], 0.4)
        d.cleanup()

    def test_timeout(self):
        d = tempfile.TemporaryDirectory()
        out = io.StringIO()
        err = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = main(["startup", "--until-file", str(Path(d.name) / "never"), "--timeout", "0.5", "--", "sleep", "30"])
        self.assertEqual(code, 1)
        d.cleanup()
        subprocess.run(["true"])


if __name__ == "__main__":
    unittest.main()

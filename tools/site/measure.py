#!/usr/bin/env python3
"""Measure an app's RAM (PSS), idle CPU and startup time over its whole process tree. Linux /proc, stdlib only."""
import argparse, json, os, re, subprocess, sys, time
from datetime import datetime, timezone
from pathlib import Path
from typing import NamedTuple, Optional

PROC = Path("/proc")


class Proc(NamedTuple):
    pid: int
    ppid: int
    comm: str
    exe: Optional[str]


def _parse_stat(text):
    """Return (comm, ppid, utime, stime); the comm may hold spaces and parentheses."""
    lp, rp = text.index("("), text.rindex(")")
    rest = text[rp + 1:].split()  # rest[0] is state (field 3)
    return text[lp + 1:rp], int(rest[1]), int(rest[11]), int(rest[12])


def read_procs(proc=PROC):
    out = {}
    for d in Path(proc).iterdir():
        if not d.name.isdigit():
            continue
        try:
            comm, ppid, _, _ = _parse_stat((d / "stat").read_text())
            try:
                exe = os.readlink(d / "exe")
            except OSError:
                exe = None
            out[int(d.name)] = Proc(int(d.name), ppid, comm, exe)
        except (OSError, ValueError, IndexError):
            continue
    return out


def tree(procs, roots):
    kids = {}
    for p in procs.values():
        kids.setdefault(p.ppid, []).append(p.pid)
    seen, todo = set(), [r for r in roots if r in procs]
    while todo:
        pid = todo.pop()
        if pid in seen:
            continue
        seen.add(pid)
        todo.extend(kids.get(pid, []))
    return seen


def roots_matching(procs, comm):
    return {p.pid for p in procs.values()
            if p.comm == comm and not (p.ppid in procs and procs[p.ppid].comm == comm)}


def pss_detail(pids, proc=PROC):
    total, unreadable = 0, 0
    for pid in pids:
        try:
            for line in (Path(proc) / str(pid) / "smaps_rollup").read_text().splitlines():
                if line.startswith("Pss:"):
                    total += int(line.split()[1])
                    break
        except (OSError, ValueError):
            unreadable += 1
    return total, unreadable


def pss_kb(pids, proc=PROC):
    return pss_detail(pids, proc)[0]


def ticks_by_pid(pids, proc=PROC):
    out = {}
    for pid in pids:
        try:
            _, _, ut, st = _parse_stat((Path(proc) / str(pid) / "stat").read_text())
            out[pid] = ut + st
        except (OSError, ValueError, IndexError):
            pass
    return out


def cpu_ticks(pids, proc=PROC):
    return sum(ticks_by_pid(pids, proc).values())


def window(before, after):
    """(ticks delta over pids present at both ends, how many such, came, went)"""
    common = set(before) & set(after)
    delta = sum(after[p] - before[p] for p in common)
    return delta, len(common), len(set(after) - set(before)), len(set(before) - set(after))


def now():
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def resolve(args, proc):
    procs = read_procs(proc)
    if args.pid:
        roots = {p for p in args.pid if p in procs}
        if not roots:
            sys.exit(_die("none of the --pid processes exists"))
    elif args.comm:
        roots = roots_matching(procs, args.comm)
        if not roots:
            sys.exit(_die(f"no process named {args.comm!r}"))
        if len(roots) > 1 and not args.all_roots:
            sys.exit(_die(f"{len(roots)} unrelated roots match {args.comm!r} ({sorted(roots)}); "
                          "pass --pid for the one you started, or --all-roots to sum them all"))
    else:
        sys.exit(_die("give --pid or --comm"))
    return procs, roots


def _die(msg):
    print("measure: " + msg, file=sys.stderr)
    return 2


def cmd_ram(args):
    for i in range(args.repeat):
        if i:
            time.sleep(args.every)
        procs, roots = resolve(args, args.proc)
        pids = tree(procs, roots)
        kb, unreadable = pss_detail(pids, args.proc)
        print(json.dumps({"what": "ram", "comm": args.comm, "roots": sorted(roots), "pids": len(pids),
                          "pss_kb": kb, "unreadable": unreadable, "at": now()}))
        sys.stdout.flush()
    return 0


def cmd_cpu(args):
    procs, roots = resolve(args, args.proc)
    before = ticks_by_pid(tree(procs, roots), args.proc)
    t0 = time.monotonic()
    time.sleep(args.seconds)
    procs2 = read_procs(args.proc)
    after = ticks_by_pid(tree(procs2, roots), args.proc)
    elapsed = time.monotonic() - t0
    delta, common, came, went = window(before, after)
    hz = os.sysconf("SC_CLK_TCK")
    print(json.dumps({"what": "cpu", "comm": args.comm, "roots": sorted(roots), "pids": common,
                      "percent_of_one_core": round(delta / hz / elapsed * 100, 2), "seconds": round(elapsed, 2),
                      "came": came, "went": went, "at": now()}))
    return 0


def cmd_startup(args):
    cmd = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not cmd:
        return _die("give the command to launch after --")
    rx = re.compile(args.until_line) if args.until_line else None
    t0 = time.monotonic()
    child = subprocess.Popen(cmd, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                             stderr=subprocess.DEVNULL, start_new_session=True)
    path = Path(args.until_file)
    while True:
        try:
            if rx is None:
                ok = path.exists()
            else:
                ok = any(rx.search(l) for l in path.read_text(errors="replace").splitlines())
        except OSError:
            ok = False
        if ok:
            secs = time.monotonic() - t0
            print(json.dumps({"what": "startup", "seconds": round(secs, 3), "pid": child.pid, "at": now()}))
            return 0
        if time.monotonic() - t0 > args.timeout:
            child.kill()
            child.wait()
            print("measure: timed out waiting for the marker", file=sys.stderr)
            return 1
        if child.poll() is not None and not path.exists():
            print("measure: the command exited before the marker appeared", file=sys.stderr)
            return 1
        time.sleep(0.02)


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--proc", default=str(PROC), help=argparse.SUPPRESS)
    sub = ap.add_subparsers(dest="cmd", required=True)

    def roots_opts(p):
        p.add_argument("--comm")
        p.add_argument("--pid", type=int, action="append", help="explicit root pid (repeatable)")
        p.add_argument("--all-roots", action="store_true")
        p.add_argument("--proc", default=argparse.SUPPRESS, help=argparse.SUPPRESS)
    r = sub.add_parser("ram"); roots_opts(r)
    r.add_argument("--repeat", type=int, default=1); r.add_argument("--every", type=float, default=1.0)
    c = sub.add_parser("cpu"); roots_opts(c)
    c.add_argument("--seconds", type=float, required=True)
    s = sub.add_parser("startup")
    s.add_argument("--until-file", required=True)
    s.add_argument("--until-line")
    s.add_argument("--timeout", type=float, default=120)
    s.add_argument("command", nargs=argparse.REMAINDER)
    args = ap.parse_args(argv)
    args.proc = Path(args.proc)
    return {"ram": cmd_ram, "cpu": cmd_cpu, "startup": cmd_startup}[args.cmd](args)


if __name__ == "__main__":
    sys.exit(main())

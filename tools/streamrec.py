#!/usr/bin/env python3
"""Record klack's own playback streams (never a microphone/source).

usage: streamrec.py OUT_DIR SECONDS_TOTAL [SECONDS_PER_STREAM]
Polls PipeWire (pactl) every 50 ms; each new sink-input whose application name contains
"klack" is recorded with pw-record (s16, 48 kHz, mono) into OUT_DIR/<n>-<serial>.wav,
with the time it appeared (seconds since start) in the name.
"""
import json, os, subprocess, sys, time

out, total = sys.argv[1], float(sys.argv[2])
per = float(sys.argv[3]) if len(sys.argv) > 3 else 3.0
os.makedirs(out, exist_ok=True)
seen, procs, t0 = set(), [], time.time()
while time.time() - t0 < total:
    try:
        inputs = json.loads(subprocess.run(["pactl", "-f", "json", "list", "sink-inputs"], capture_output=True, text=True).stdout or "[]")
    except json.JSONDecodeError:
        inputs = []
    for si in inputs:
        props = si.get("properties", {})
        serial = props.get("object.serial")
        if serial is None or serial in seen or "klack" not in props.get("application.name", "").lower():
            continue
        seen.add(serial)
        at = time.time() - t0
        path = os.path.join(out, f"{at:06.2f}s-{serial}.wav")
        procs.append(subprocess.Popen(["timeout", str(per), "pw-record", "--target", str(serial), "--format", "s16", "--rate", "48000", "--channels", "1", path], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
        print(f"recording stream {serial} (appeared at {at:.2f} s) -> {path}", flush=True)
    time.sleep(0.05)
for p in procs:
    p.wait()

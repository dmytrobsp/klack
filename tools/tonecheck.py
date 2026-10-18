#!/usr/bin/env python3
"""Peak level (dBFS) of 50 ms windows at given frequencies in s16 mono WAV files.

usage: tonecheck.py HZ[,HZ…] FILE.wav…   (prints, per file: the loudest window per frequency)
"""
import math, struct, sys, wave

freqs = [float(f) for f in sys.argv[1].split(",")]
for path in sys.argv[2:]:
    with wave.open(path) as w:
        sr, n = w.getframerate(), w.getnframes()
        x = struct.unpack(f"<{n}h", w.readframes(n))
    win = sr // 20
    best = {}
    for f in freqs:
        c = 2 * math.cos(2 * math.pi * f / sr)
        peak = 0.0
        for i in range(0, max(len(x) - win, 0) + 1, win):
            s1 = s2 = 0.0
            for v in x[i:i + win]:
                s = v + c * s1 - s2
                s2, s1 = s1, s
            peak = max(peak, 2 * math.sqrt(max(s1 * s1 + s2 * s2 - c * s1 * s2, 0)) / win)
        best[f] = 20 * math.log10(max(peak, 1) / 32768)
    print(path.split("/")[-1], f"{len(x)/sr:.2f} s", "  ".join(f"{f:g} Hz {db:.0f} dBFS" for f, db in best.items()))

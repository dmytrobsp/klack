"""Print unique matches of a regex with context from minified JS files.
usage: jsgrep.py PATTERN CONTEXT MAX FILE..."""
import re, sys
pat, ctx, mx = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
seen = 0
for path in sys.argv[4:]:
    s = open(path, encoding="utf-8", errors="replace").read()
    for m in re.finditer(pat, s):
        print(f"{path.split('/')[-1]}@{m.start()}: {s[max(0, m.start()-ctx):m.end()+ctx]!r}")
        seen += 1
        if seen >= mx:
            sys.exit()

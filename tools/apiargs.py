"""Show call sites (with their args) of a Slack Web API method in the web client bundles.
usage: apiargs.py METHOD DIR..."""
import os, re, sys
method, dirs = sys.argv[1], sys.argv[2:]
files = {os.path.join(d, f): open(os.path.join(d, f), encoding="utf-8", errors="replace").read() for d in dirs for f in os.listdir(d)}
defs = set()
for path, s in files.items():
    for m in re.finditer(r'"' + re.escape(method) + r'"', s):
        head = re.search(r'(\w+|"[^"]+")\(\w+,\w+,\w+\)\{"use strict";[^{]*$', s[max(0, m.start() - 160):m.start()])
        tail = re.search(r'\{\},\{(\w+):', s[m.end():m.end() + 160])
        if head and tail:
            defs.add((head.group(1).strip('"'), tail.group(1)))
print("definitions:", defs)
for mod, key in defs:
    for path, s in files.items():
        for v in re.finditer(r'(\w+)=\w+\("' + re.escape(mod) + r'"\)', s):
            for u in re.finditer(r'\b' + re.escape(v.group(1)) + r'\.' + re.escape(key) + r'\b', s):
                print(f"--- {os.path.basename(path)[:50]}@{u.start()}")
                print(s[max(0, u.start() - 250):u.start() + 350])

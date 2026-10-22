#!/bin/sh
# Name guard: finds the old name "klack" (any case) in tracked files and file names under the given paths.
# Usage: sh tools/check-name.sh PATH...
# Exit 0: clean. Exit 1: hits, printed as file:line:text (or the file name). Exit 2: usage or git error.
# Allowed: PROGRESS.md, docs/superpowers/, tools/move-to-ttyak.sh, this script, and lines marked old-name-ok.
set -u

[ $# -ge 1 ] || { echo "usage: sh tools/check-name.sh PATH..." >&2; exit 2; }
for p in "$@"; do
    git ls-files --error-unmatch -- "$p" >/dev/null 2>&1 || { echo "no tracked files at: $p" >&2; exit 2; }
done

set -- "$@" \
    ':(top,exclude)PROGRESS.md' \
    ':(top,exclude)docs/superpowers/' \
    ':(top,exclude)tools/move-to-ttyak.sh' \
    ':(top,exclude)tools/check-name.sh'

lines=$(git --no-pager grep -nIi -e klack -- "$@")
rc=$?
[ $rc -le 1 ] || { echo "git grep failed (exit $rc)" >&2; exit 2; }
lines=$(printf '%s\n' "$lines" | grep -v -e '^[^:]*:[0-9]*:.*old-name-ok')

names=$(git ls-files -- "$@" | grep -i -e klack)

[ -z "$lines" ] && [ -z "$names" ] && exit 0
[ -z "$lines" ] || printf '%s\n' "$lines"
[ -z "$names" ] || printf '%s\n' "$names"
exit 1

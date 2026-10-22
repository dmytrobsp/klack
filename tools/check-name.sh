#!/bin/sh
# Name guard: finds the old name "klack" (any case) in tracked files and file names under the given paths.
# Usage: sh tools/check-name.sh PATH...
#        sh tools/check-name.sh [--except PATH]... [PATH...]   (no PATH: every tracked file, minus the exceptions)
# Exit 0: clean. Exit 1: hits, printed as file:line:text (or the file name). Exit 2: usage or git error.
# Allowed: PROGRESS.md, docs/superpowers/, tools/move-to-ttyak.sh, this script, and lines marked old-name-ok.
set -u

usage() { echo "usage: sh tools/check-name.sh [--except PATH]... [PATH...]" >&2; exit 2; }
[ $# -ge 1 ] || usage

# One pass over the arguments: paths stay, each --except PATH turns into an exclude pathspec at the end.
n=$#; paths=0
while [ "$n" -gt 0 ]; do
    if [ "$1" = "--except" ]; then
        [ $# -ge 2 ] || usage
        git ls-files --error-unmatch -- "$2" >/dev/null 2>&1 || { echo "no tracked files at: $2" >&2; exit 2; }
        set -- "$@" ":(top,exclude)$2"
        shift 2; n=$((n - 2))
    else
        git ls-files --error-unmatch -- "$1" >/dev/null 2>&1 || { echo "no tracked files at: $1" >&2; exit 2; }
        set -- "$@" "$1"; paths=$((paths + 1))
        shift; n=$((n - 1))
    fi
done
[ "$paths" -gt 0 ] || set -- ":(top)" "$@"

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

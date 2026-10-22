#!/bin/sh
# One-off: moves the klack folders of this user to the ttyak names.
# Usage: sh tools/move-to-ttyak.sh [--dry-run]
# Exit 0: every folder moved or had nothing to move. Exit 1: something was refused.
set -eu

[ -n "${HOME:-}" ] || { echo "HOME is not set" >&2; exit 1; }
H=${HOME:?}
[ -d "$H" ] || { echo "HOME is not a directory: $H" >&2; exit 1; }

DRY=0
[ $# -le 1 ] || { echo "usage: sh move-to-ttyak.sh [--dry-run]" >&2; exit 1; }
if [ $# -eq 1 ]; then
    [ "$1" = --dry-run ] || { echo "usage: sh move-to-ttyak.sh [--dry-run]" >&2; exit 1; }
    DRY=1
fi

FAILED=0

# Refuse while either name runs; stop when pgrep cannot tell.
for name in klack ttyak; do
    rc=0
    pgrep -x "$name" >/dev/null || rc=$?
    case $rc in
        0) echo "refused: $name is running; quit it first"; exit 1 ;;
        1) ;;
        *) echo "cannot tell whether $name runs (pgrep exit $rc)"; exit 1 ;;
    esac
done

# exists PATH: true for anything there, a dangling symlink too.
exists() { [ -e "$1" ] || [ -L "$1" ]; }

# move_dir SRC DST: moves SRC to DST when SRC is a folder and DST does not exist.
move_dir() {
    if [ ! -d "$1" ]; then
        echo "nothing to move: $1"
    elif exists "$2"; then
        echo "refused: $2 exists already (left $1 where it is)"
        FAILED=1
    elif [ "$DRY" = 1 ]; then
        echo "would move $1 -> $2"
    else
        mv "$1" "$2"
        echo "moved $1 -> $2"
    fi
}

# rename_logs DIR: klack.log -> ttyak.log in each profile folder of DIR, unless ttyak.log exists.
rename_logs() {
    for log in "$1"/*/klack.log; do
        [ -f "$log" ] || continue
        new="${log%klack.log}ttyak.log"
        if exists "$new"; then
            echo "refused: $new exists already (left $log where it is)"
            FAILED=1
        elif [ "$DRY" = 1 ]; then
            echo "would rename $log -> $new"
        else
            mv "$log" "$new"
            echo "renamed $log -> $new"
        fi
    done
}

move_dir "$H/.config/klack" "$H/.config/ttyak"
# The logs are renamed in the old folder, before it moves, and only when it will move.
if [ -d "$H/.cache/klack" ] && ! exists "$H/.cache/ttyak"; then
    rename_logs "$H/.cache/klack"
fi
move_dir "$H/.cache/klack" "$H/.cache/ttyak"
move_dir "$H/.local/share/klack" "$H/.local/share/ttyak"
move_dir "$H/.config/klack-site" "$H/.config/ttyak-site"

# Leftovers that still say klack: file names only, never contents.
echo
echo "check by hand (files that still mention klack or KLACK_; not edited):"
FOUND=0
list_matches() {
    for f in "$@"; do
        [ -f "$f" ] || continue
        if grep -qE 'klack|KLACK_' "$f" 2>/dev/null; then
            echo "  $f"
            FOUND=1
        fi
    done
}
list_matches "$H/.config/ttyak/settings.toml" "$H/.config/ttyak/keys.toml" \
    "$H/.config/klack/settings.toml" "$H/.config/klack/keys.toml"
if [ -d "$H/.config/fish" ]; then
    if grep -rlE 'klack|KLACK_' "$H/.config/fish" 2>/dev/null | sed 's/^/  /' | grep . ; then
        FOUND=1
    fi
fi
list_matches "$H/.bashrc" "$H/.zshrc" "$H/.profile"
[ "$FOUND" = 1 ] || echo "  (none)"

exit "$FAILED"

#!/usr/bin/env bash
# Packs everything needed to sign, publish and host ttyak into one passphrase-encrypted file (gpg, AES-256):
# the signing keys and certificates, the Apple API key, the accounts notes, the site's private lists.
# Run it in your own terminal: gpg asks for the passphrase itself, so it never passes through anything else.
# Usage: tools/backup-secrets.sh [OUTPUT]   (default: ~/ttyak-secrets-YYYY-MM-DD.tar.gpg)
# Check a backup (lists names only): gpg -d FILE | tar -tvf -
set -euo pipefail
umask 077

H=${HOME:?}
[ -d "$H" ] || { echo "no home folder: $H" >&2; exit 1; }
out=${1:-$H/ttyak-secrets-$(date +%F).tar.gpg}
[ -e "$out" ] && { echo "$out exists already: give another name" >&2; exit 1; }

# Folders under the home folder that hold what a release needs; a missing one is named and skipped.
items=(.config/ttyak-signing .config/ttyak-secrets .config/ttyak-site)

have=()
for p in "${items[@]}"; do
	if [ -e "$H/$p" ]; then have+=("$p"); else echo "not there, skipped: ~/$p"; fi
done
[ ${#have[@]} -gt 0 ] || { echo "nothing to back up" >&2; exit 1; }
for f in "$H"/Downloads/AuthKey_*.p8; do
	[ -e "$f" ] && echo "note: $f is outside the backup; move it into ~/.config/ttyak-secrets/ and run again"
done

echo "Packing (names only):"
for p in "${have[@]}"; do echo "  ~/$p"; done
tar -C "$H" -cf - -- "${have[@]}" |
	gpg --symmetric --cipher-algo AES256 --s2k-digest-algo SHA512 --s2k-count 65011712 --output "$out"
echo "Written: $out"
echo "Check it opens (lists names only): gpg -d $out | tar -tvf -"
echo "Keep it somewhere other than this machine, and the passphrase in your password manager."

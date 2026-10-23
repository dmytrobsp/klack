#!/usr/bin/env bash
# The SSH key the release workflow pushes ttyak-bin to the AUR with (docs/distribution/accounts.md, AUR step 5).
# Usage: tools/publish/aur-key.sh [--force]
# Makes a new ed25519 key (no passphrase) in a fresh 0700 folder, pipes the private key into the secret AUR_SSH_KEY of
# dmytrobsp/ttyak, deletes it and prints only the public key: paste that line into the AUR account's SSH Public Key.
# Refuses when AUR_SSH_KEY is set already, unless --force (the old key then stops working for the workflow).
set -euo pipefail
umask 077

ME="tools/publish/aur-key.sh"
REPO="dmytrobsp/ttyak"
SECRET="AUR_SSH_KEY"
COMMENT="ttyak-bin AUR push ($REPO release workflow)"

usage() { echo "usage: $ME [--force]" >&2; exit 2; }
die() { echo "$ME: $*" >&2; exit 1; }

force=false
case $# in
    0) ;;
    1) [ "$1" = --force ] || usage; force=true ;;
    *) usage ;;
esac

command -v gh > /dev/null || die "gh (GitHub CLI) is not installed"
command -v ssh-keygen > /dev/null || die "ssh-keygen (OpenSSH) is not installed"
gh auth status > /dev/null 2>&1 < /dev/null || die "gh is not logged in: run gh auth login, then $ME again"
admin=$(gh api "repos/$REPO" --jq .permissions.admin 2> /dev/null < /dev/null) \
    || die "gh cannot read the repository $REPO: check gh auth status"
[ "$admin" = true ] || die "the gh account is not an admin of $REPO, so it cannot set the secret"
names=$(gh secret list -R "$REPO" --json name --jq '.[].name' < /dev/null) \
    || die "gh cannot list the secrets of $REPO, so it cannot tell whether $SECRET is set"
if grep -qx "$SECRET" <<< "$names" && [ "$force" != true ]; then
    die "$SECRET is set already. To replace it: $ME --force, then put the new public key on the AUR"
fi

# The key lives only in this folder, only until it is in the secret.
DIR=$(mktemp -d "${TMPDIR:-/tmp}/ttyak-aur-key.XXXXXX")
cleanup() { rm -f -- "$DIR/aur" "$DIR/aur.pub"; rmdir -- "$DIR" 2> /dev/null || true; }
trap cleanup EXIT
trap 'exit 130' INT TERM HUP
chmod 700 -- "$DIR"

env -u SSH_AUTH_SOCK ssh-keygen -q -t ed25519 -N '' -C "$COMMENT" -f "$DIR/aur" < /dev/null > /dev/null \
    || die "ssh-keygen could not make the key"
gh secret set "$SECRET" -R "$REPO" < "$DIR/aur" > /dev/null \
    || die "gh secret set $SECRET failed; GitHub keeps its old value. Check 'gh auth status', then run $ME again"
rm -f -- "$DIR/aur"
cat -- "$DIR/aur.pub"
echo "$ME: $SECRET is set. Paste the line above into the AUR: My Account, SSH Public Key." >&2

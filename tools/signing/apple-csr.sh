#!/usr/bin/env bash
# The Developer ID Application certificate for signing macOS builds (docs/distribution/accounts.md, Apple 17-22).
# Usage: tools/signing/apple-csr.sh request [--renew]   new key + certificate request in ~/.config/ttyak-signing/
#        tools/signing/apple-csr.sh install FILE.cer    Apple's certificate -> p12 -> APPLE_P12, APPLE_P12_PASSWORD
#        tools/signing/apple-csr.sh status              what the folder holds; nothing secret
# The key and the p12 password stay in files (0600); they reach GitHub only by a pipe into `gh secret set`.
# G2 certificates last one year: run `request --renew` (old files kept, dated), then `install` again.
# TTYAK_SIGNING_CA: a local file used instead of downloading Apple's G2 intermediate (for tests; not pinned).
set -euo pipefail
umask 077

ME="tools/signing/apple-csr.sh"
DIR="${HOME:?}/.config/ttyak-signing"
KEY="$DIR/developer-id.key"
CSR="$DIR/developer-id.certSigningRequest"
CER="$DIR/developer-id.cer"
P12="$DIR/developer-id.p12"
PASS="$DIR/developer-id.p12-password"
REPO="dmytrobsp/ttyak"
# Apple sets the certificate's subject from the account and ignores the request's; this one only names the key.
SUBJECT="/CN=ttyak Developer ID/emailAddress=dmytrobsp@gmail.com"
G2_URL="https://www.apple.com/certificateauthority/DeveloperIDG2CA.cer"
# SHA-256 of that file (DER) as downloaded on 2026-10-23: "Developer ID Certification Authority", OU G2, until 2031.
G2_SHA256="f16cd3c54c7f83cea4bf1a3e6a0819c8aaa8e4a1528fd144715f350643d2df3a"

usage() { echo "usage: $ME request [--renew] | install FILE.cer | status" >&2; exit 2; }
die() { echo "$ME: $*" >&2; exit 1; }

# Single files made on the way, removed on exit.
TEMPS=()
cleanup() { if [ ${#TEMPS[@]} -gt 0 ]; then rm -f -- "${TEMPS[@]}"; fi; }
trap cleanup EXIT
trap 'exit 130' INT TERM HUP

# Sets the variable named $1 to a new empty file in the folder (so the final mv stays on one file system).
temp() {
    local t
    t=$(mktemp "$DIR/.tmp.XXXXXX")
    TEMPS+=("$t")
    printf -v "$1" '%s' "$t"
}

folder() {
    mkdir -p -- "$DIR"
    chmod 700 -- "$DIR"
}

sha256_of() { openssl dgst -sha256 -r "$1" | cut -d' ' -f1; }

# Public-key digests: the certificate's and the key's. Only public halves pass through the pipes.
cert_pub() { openssl x509 -in "$1" -noout -pubkey | openssl pkey -pubin -outform DER | openssl dgst -sha256 -r; }
key_pub() { openssl pkey -in "$KEY" -pubout -outform DER | openssl dgst -sha256 -r; }

subject_cn() { openssl x509 -in "$1" -noout -subject -nameopt sep_multiline,utf8,sname | sed -n 's/^ *CN=//p'; }

not_after() { local na; na=$(openssl x509 -in "$1" -noout -enddate); printf '%s\n' "${na#notAfter=}"; }

expiry() { date -u -d "$(not_after "$1")" +%F; }

days_left() { echo $(( ($(date -u -d "$(not_after "$1")" +%s) - $(date -u +%s)) / 86400 )); }

# A date suffix that no kept file has yet: 2026-10-23, then 2026-10-23-2, ...
aside_suffix() {
    local day n=1 s f taken
    day=$(date +%F)
    s=$day
    while :; do
        taken=0
        for f in "$KEY" "$CSR" "$CER" "$P12" "$PASS"; do
            if [ -e "$f.$s" ]; then taken=1; fi
        done
        [ $taken = 1 ] || break
        n=$((n + 1))
        s="$day-$n"
    done
    printf '%s\n' "$s"
}

# Moves the named files that exist aside to <name>.<date>, listed in KEPT; never deletes them.
KEPT=()
move_aside() {
    local s f
    s=$(aside_suffix)
    for f in "$@"; do
        if [ -e "$f" ]; then
            mv -- "$f" "$f.$s"
            KEPT+=("$f.$s")
        fi
    done
}

print_kept() {
    local f
    if [ ${#KEPT[@]} -gt 0 ]; then
        for f in "${KEPT[@]}"; do echo "Kept: $f"; done
    fi
}

request() {
    local renew=0 key csr
    [ $# -le 1 ] || usage
    case "${1-}" in
        "") ;;
        --renew) renew=1 ;;
        *) usage ;;
    esac
    folder
    if [ -e "$KEY" ] && [ $renew = 0 ]; then
        echo "$ME: a key exists already: $KEY" >&2
        if [ -e "$CSR" ]; then echo "Its request, to upload to Apple: $CSR" >&2; fi
        echo "For a new key (the yearly renewal): $ME request --renew (the old files are kept, dated)." >&2
        exit 1
    fi
    temp key
    temp csr
    openssl genpkey -quiet -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "$key"
    openssl req -new -key "$key" -subj "$SUBJECT" -sha256 -out "$csr"
    openssl req -in "$csr" -noout -text | grep -q 'Public-Key: (2048 bit)' || die "the request has no RSA 2048 key"
    if [ $renew = 1 ]; then move_aside "$KEY" "$CSR" "$CER" "$P12" "$PASS"; fi
    mv -- "$key" "$KEY"
    mv -- "$csr" "$CSR"
    print_kept
    cat <<EOF
Made a new private key (RSA 2048) and certificate request in $DIR
(only you can read it; it is your backup).

Upload this file to Apple: $CSR

  1. Open https://developer.apple.com/account/resources/certificates/list and click +.
  2. Under Software, choose Developer ID, then Developer ID Application, then Continue.
  3. If it asks for a Developer ID Certificate Intermediary: G2 Sub-CA (Xcode 11.4.1 or later).
  4. Choose File: the file above. Continue, then Download.

Then: $ME install ~/Downloads/developerID_application.cer
EOF
}

# Writes Apple's Developer ID G2 intermediate (PEM) to $1: downloaded and checked against the pin.
intermediate() {
    local out=$1 raw
    if [ -n "${TTYAK_SIGNING_CA-}" ]; then
        openssl x509 -in "$TTYAK_SIGNING_CA" -out "$out" 2>/dev/null \
            || openssl x509 -inform DER -in "$TTYAK_SIGNING_CA" -out "$out" 2>/dev/null \
            || die "TTYAK_SIGNING_CA is not a certificate: $TTYAK_SIGNING_CA"
        return
    fi
    command -v curl >/dev/null || die "curl is not installed"
    temp raw
    curl -fsSL --proto '=https' -o "$raw" "$G2_URL" || die "could not download $G2_URL"
    [ "$(sha256_of "$raw")" = "$G2_SHA256" ] \
        || die "$G2_URL is not the pinned file (its SHA-256 changed): check it before changing the pin"
    openssl x509 -inform DER -in "$raw" -out "$out"
}

install_cer() {
    local given pem ca p12 pass der cn why
    [ $# -eq 1 ] || usage
    given=$1
    [ -f "$given" ] || die "no such file: $given"
    [ -f "$KEY" ] || die "no key in $DIR yet: run $ME request first"
    command -v gh >/dev/null || die "gh (GitHub CLI) is not installed"
    folder
    temp pem
    openssl x509 -inform DER -in "$given" -out "$pem" 2>/dev/null \
        || openssl x509 -inform PEM -in "$given" -out "$pem" 2>/dev/null \
        || die "not a certificate (DER or PEM): $given"
    cn=$(subject_cn "$pem")
    case "$cn" in
        "Developer ID Application: "*) ;;
        *) die "not a Developer ID Application certificate: $cn (make it with Software > Developer ID > Developer ID Application)" ;;
    esac
    openssl x509 -in "$pem" -noout -ext extendedKeyUsage 2>/dev/null | grep -q 'Code Signing' \
        || die "the certificate is not for Code Signing"
    [ "$(cert_pub "$pem")" = "$(key_pub)" ] \
        || die "the certificate does not match the key $KEY (Apple must get the request made with it: $CSR)"
    openssl x509 -in "$pem" -noout -checkend 0 >/dev/null || die "the certificate expired on $(expiry "$pem")"
    temp ca
    intermediate "$ca"
    why=$(openssl verify -partial_chain -trusted "$ca" "$pem" 2>&1) \
        || die "not issued by Apple's Developer ID G2 authority: make it again and choose G2 Sub-CA (Xcode 11.4.1 or later). openssl: $(printf '%s\n' "$why" | grep -m 1 -o 'error [0-9]* at .*' || true)"

    # PBE-SHA1-3DES keys and certificates with a SHA-1 MAC: `security import` rejects OpenSSL 3's AES/PBKDF2 default.
    temp p12
    temp pass
    temp der
    openssl rand -base64 32 | tr -d '\n' > "$pass"
    openssl pkcs12 -export -inkey "$KEY" -in "$pem" -certfile "$ca" -name "$cn" \
        -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES -macalg sha1 -passout "file:$pass" -out "$p12"
    openssl pkcs12 -in "$p12" -noout -passin "file:$pass" 2>/dev/null || die "the new p12 does not open"
    openssl x509 -in "$pem" -outform DER -out "$der"

    if [ -e "$CER" ] && ! cmp -s "$der" "$CER"; then move_aside "$CER" "$P12" "$PASS"; fi
    mv -- "$der" "$CER"
    mv -- "$p12" "$P12"
    mv -- "$pass" "$PASS"

    base64 < "$P12" | tr -d '\n' | gh secret set APPLE_P12 -R "$REPO" >/dev/null \
        || die "gh secret set APPLE_P12 failed; GitHub keeps its old value. Check 'gh auth status', then run install again"
    gh secret set APPLE_P12_PASSWORD -R "$REPO" < "$PASS" >/dev/null \
        || die "gh secret set APPLE_P12_PASSWORD failed after APPLE_P12 was set: run install again"
    print_kept
    echo "Certificate: $cn, expires $(expiry "$pem")"
    echo "APPLE_P12 and APPLE_P12_PASSWORD are set (expires $(expiry "$pem"))"
}

status() {
    local f days pem kept=""
    [ $# -eq 0 ] || usage
    if [ ! -d "$DIR" ]; then
        echo "Nothing yet: $DIR does not exist. Start with: $ME request"
        return
    fi
    echo "$DIR:"
    if [ -e "$KEY" ]; then echo "  developer-id.key                 the private key"; else echo "  (no key: $ME request)"; fi
    if [ -e "$CSR" ]; then echo "  developer-id.certSigningRequest  the request to upload to Apple"; fi
    if [ -e "$CER" ]; then
        temp pem
        openssl x509 -inform DER -in "$CER" -out "$pem"
        days=$(days_left "$pem")
        printf '  developer-id.cer                 %s, expires %s (%s)' "$(subject_cn "$pem")" "$(expiry "$pem")" \
            "$([ "$days" -ge 0 ] && echo "$days days left" || echo "expired")"
        if [ -e "$KEY" ] && [ "$(cert_pub "$pem")" = "$(key_pub)" ]; then
            echo "; matches the key"
        else
            echo "; does NOT match the key"
        fi
    elif [ -e "$KEY" ]; then
        echo "  (no certificate yet: upload the request to Apple, then $ME install FILE.cer)"
    fi
    if [ -e "$P12" ]; then echo "  developer-id.p12                 key and certificate, as in APPLE_P12"; fi
    if [ -e "$PASS" ]; then echo "  developer-id.p12-password        its password, as in APPLE_P12_PASSWORD"; fi
    for f in "$DIR"/developer-id.*.[0-9]*; do
        if [ -e "$f" ]; then kept="$kept ${f##*/}"; fi
    done
    if [ -n "$kept" ]; then echo "Kept from before:$kept"; fi
}

command -v openssl >/dev/null || die "openssl is not installed"
case "${1-}" in
    request) shift; request "$@" ;;
    install) shift; install_cer "$@" ;;
    status) shift; status "$@" ;;
    *) usage ;;
esac

#!/bin/sh
# Installs ttyak for the current user on Linux and macOS, or updates it in place:
#   curl -fsSL https://ttyak.sh/install.sh | sh
#   curl -fsSL https://ttyak.sh/install.sh | sh -s -- --yes --version 0.10.0 --dir ~/bin
# The archive comes from the GitHub release and must match the release's SHA256SUMS. `--help` lists the flags.
set -eu

MARKER='# added by the ttyak installer'
LATEST_URL=${TTYAK_LATEST_URL:-https://ttyak.sh/latest.json}
DOWNLOAD_URL=${TTYAK_DOWNLOAD_URL:-https://github.com/dmytrobsp/ttyak/releases/download}
tmp=''
# A file being put in place in the install folder, removed when the script stops before it is moved.
staged=''

usage() {
	cat <<'EOF'
Installs ttyak into ~/.local/bin, or updates the copy there.

usage: install.sh [--yes] [--version X.Y.Z] [--dir PATH]
       install.sh --uninstall [--yes] [--keep-data] [--dir PATH]

  -y, --yes          answer yes to every question (needed with no terminal for --uninstall, or as root)
  --version X.Y.Z    install this version instead of the latest
  --dir PATH         install into PATH instead of ~/.local/bin
  --uninstall        remove ttyak and its data (runs `ttyak uninstall`)
  --keep-data        with --uninstall: keep settings, sign-ins, drafts and caches
  -h, --help         this text

The environment variables TTYAK_YES=1, TTYAK_VERSION, TTYAK_DIR, TTYAK_UNINSTALL=1 and TTYAK_KEEP_DATA=1 do the
same as the flags; the flags win.
EOF
}

say() { printf '%s\n' "$*"; }
warn() { printf 'ttyak installer: %s\n' "$*" >&2; }
die() {
	warn "$*"
	exit 1
}
has() { command -v "$1" >/dev/null 2>&1; }

# Whether a flag's environment variable says yes.
is_on() {
	case ${1:-} in 1 | true | yes) return 0 ;; esac
	return 1
}

# Whether there is a terminal to ask on; stdin is the script itself under `curl | sh`.
have_tty() { (: </dev/tty) 2>/dev/null; }

# Asks a yes/no question on the terminal; no is the default.
ask() {
	printf '%s [y/N] ' "$1" >/dev/tty
	answer=''
	read -r answer </dev/tty || true
	case $answer in y | Y | yes | Yes | YES) return 0 ;; esac
	return 1
}

# Whether $1 is a release version: X.Y.Z, or X.Y.Z-pre.
is_version() {
	case $1 in '' | *[!0-9A-Za-z.-]*) return 1 ;; esac
	printf '%s\n' "$1" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?$'
}

# The first "version" string in a JSON file (latest.json, install.json); nothing when there is none.
json_version() {
	[ -f "$1" ] || return 0
	tr '\r\n\t' '   ' <"$1" | awk '{
		if (match($0, /"version"[ ]*:[ ]*"[^"]*"/)) {
			s = substr($0, RSTART, RLENGTH)
			sub(/^"version"[ ]*:[ ]*"/, "", s)
			sub(/"$/, "", s)
			print s
		}
	}'
}

# The release platform name of this machine, as the archives name it.
detect_platform() {
	os=$(uname -s 2>/dev/null || true)
	arch=$(uname -m 2>/dev/null || true)
	case $os in
	Linux) os=linux ;;
	Darwin)
		os=macos
		# A shell under Rosetta says x86_64 on an arm64 Mac: take the native build.
		if [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || true)" = 1 ]; then arch=arm64; fi
		;;
	MINGW* | MSYS* | CYGWIN* | Windows_NT)
		die "this script is for Linux and macOS; on Windows run in PowerShell: irm https://ttyak.sh/install.ps1 | iex"
		;;
	*) die "ttyak has no build for ${os:-this system}: there are builds for Linux, macOS and Windows" ;;
	esac
	case $arch in
	x86_64 | amd64) arch=x86_64 ;;
	aarch64 | arm64) arch=aarch64 ;;
	*) die "ttyak has no build for ${arch:-this CPU} on $os: there are builds for x86_64 and arm64" ;;
	esac
	platform=$arch-$os
}

# Picks the downloader, the SHA256 tool and the unzipper, before anything is downloaded.
pick_tools() {
	if has curl; then
		fetcher=curl
	elif has wget; then
		fetcher=wget
	else
		die "needs curl or wget to download ttyak: install curl with your package manager"
	fi
	if has sha256sum; then
		summer=sha256sum
	elif has shasum; then
		summer=shasum
	else
		die "needs sha256sum or shasum to check the download: install coreutils with your package manager"
	fi
	if has unzip; then
		unzipper=unzip
	elif has bsdtar; then
		unzipper=bsdtar
	elif has python3; then
		unzipper=python3
	else
		die "needs unzip to unpack the download (bsdtar or python3 also do): install unzip with your package manager"
	fi
}

# Downloads $1 into the file $2.
fetch() {
	if [ "$fetcher" = curl ]; then
		case $1 in
		https://*) curl -fsSL --proto '=https' -o "$2" "$1" </dev/null ;;
		*) curl -fsSL -o "$2" "$1" </dev/null ;;
		esac
	else
		wget -q -O "$2" "$1" </dev/null
	fi
}

sha256_of() {
	if [ "$summer" = sha256sum ]; then
		sha256sum "$1" </dev/null
	else
		shasum -a 256 "$1" </dev/null
	fi | awk '{ print tolower($1) }'
}

# Unpacks the zip $1 into the folder $2.
unpack() {
	case $unzipper in
	unzip) unzip -q -o "$1" -d "$2" </dev/null ;;
	bsdtar) bsdtar -xf "$1" -C "$2" </dev/null ;;
	python3) python3 -m zipfile -e "$1" "$2" </dev/null ;;
	esac
}

# The latest version as latest.json names it; nothing when it cannot be read.
version_from_feed() {
	fetch "$LATEST_URL" "$tmp/latest.json" 2>/dev/null || return 0
	json_version "$tmp/latest.json"
}

# The latest version from where GitHub's /releases/latest redirects (its Location names the tag).
version_from_github() {
	releases=${DOWNLOAD_URL%/download}
	[ "$releases" != "$DOWNLOAD_URL" ] || return 0
	if [ "$fetcher" = curl ]; then
		headers=$(curl -sSI "$releases/latest" </dev/null 2>/dev/null || true)
	else
		headers=$(wget -S --max-redirect=0 -O /dev/null "$releases/latest" </dev/null 2>&1 || true)
	fi
	tag=$(printf '%s\n' "$headers" | tr -d '\r' | awk 'tolower($1) == "location:" { loc = $2 } END { print loc }' |
		sed -n 's|^.*/releases/tag/\([^/?#]*\)$|\1|p')
	printf '%s\n' "${tag#v}"
}

# Sets $version: the one asked for, else the latest.
choose_version() {
	if [ -n "$version" ]; then
		version=${version#v}
		is_version "$version" || die "not a ttyak version: $version (expected X.Y.Z, e.g. 0.10.0)"
		return 0
	fi
	version=$(version_from_feed)
	if ! is_version "$version"; then
		say "Could not read the latest version from $LATEST_URL; asking GitHub instead"
		version=$(version_from_github)
		is_version "$version" || die "could not find out the latest ttyak version (from $LATEST_URL or GitHub); pass --version X.Y.Z"
	fi
}

# Makes the install folder if it is missing, for this user only, and sets $dir to its absolute path.
make_dir() {
	case $dir in *[[:cntrl:]]*) die "the folder's name has control characters in it: pick another with --dir" ;; esac
	if [ ! -d "$dir" ]; then
		(umask 077 && mkdir -p -- "$dir") || die "could not create $dir"
	fi
	dir=$(CDPATH='' cd -- "$dir" && pwd) || die "could not open $dir"
	[ -w "$dir" ] || die "cannot write to $dir: pick a folder you own with --dir"
}

# Writes install.json next to the binary, atomically.
write_record() {
	escaped=$(printf '%s' "$dir" | sed 's/\\/\\\\/g; s/"/\\"/g')
	staged=$dir/.install.json.new
	printf '{"method":"script","version":"%s","dir":"%s"}\n' "$version" "$escaped" >"$staged"
	mv -f -- "$staged" "$dir/install.json"
	staged=''
}

# $1 quoted for sh.
sh_quote() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"; }

# $1 quoted for fish (in its single quotes only \\ and \' are special).
fish_quote() { printf "'%s'" "$(printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e "s/'/\\\\'/g")"; }

# Puts $dir on PATH in the shell's profile when it is not on PATH already, once.
add_to_path() {
	case ":${PATH:-}:" in
	*":$dir:"* | *":$dir/:"*)
		found=$(command -v ttyak 2>/dev/null || true)
		if [ -n "$found" ] && [ "$found" != "$dir/ttyak" ]; then
			warn "note: $found comes before $dir on PATH, so \`ttyak\` runs that one"
		fi
		say "Run: ttyak"
		return 0
		;;
	esac
	shell_name=${SHELL:-}
	shell_name=${shell_name##*/}
	if [ "$dir" = "$HOME/.local/bin" ]; then
		# shellcheck disable=SC2016 # $HOME and $PATH are for the profile to expand
		posix_line='export PATH="$HOME/.local/bin:$PATH"'
		fish_line='fish_add_path --path ~/.local/bin'
	else
		posix_line="export PATH=$(sh_quote "$dir"):\"\$PATH\""
		fish_line="fish_add_path --path $(fish_quote "$dir")"
	fi
	line=$posix_line
	case $shell_name in
	bash)
		# macOS terminals start login shells, which read ~/.bash_profile and not ~/.bashrc.
		if [ "$os" = macos ]; then profile=$HOME/.bash_profile; else profile=$HOME/.bashrc; fi
		;;
	zsh) profile=${ZDOTDIR:-$HOME}/.zshrc ;;
	fish)
		profile=${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d/ttyak.fish
		line=$fish_line
		;;
	*) profile=$HOME/.profile ;;
	esac
	if [ -f "$profile" ] && grep -Fqx -- "$line $MARKER" "$profile"; then
		say "$profile already puts $dir on PATH: open a new terminal to use ttyak"
		return 0
	fi
	(umask 077 && mkdir -p -- "${profile%/*}") || die "could not create ${profile%/*}"
	if [ -s "$profile" ] && [ -n "$(tail -c 1 -- "$profile")" ]; then printf '\n' >>"$profile"; fi
	printf '%s %s\n' "$line" "$MARKER" >>"$profile"
	say "$dir was not on PATH: added it in $profile."
	say "Open a new terminal to use ttyak, or run this line in this one first:"
	say "  $line"
}

# As root: install only when asked (--yes, --dir, or yes on the terminal).
root_gate() {
	if [ -n "$dir_given" ] || is_on "$yes"; then
		warn "running as root: installing into $dir"
		return 0
	fi
	if have_tty; then
		ask "Running as root: install ttyak for root into $dir?" && return 0
		die "nothing installed: run the installer as your own user, or pick a folder with --dir"
	fi
	die "running as root with no terminal to ask: pass --yes to install for root into $dir, or --dir PATH, or run it as your own user"
}

install_ttyak() {
	detect_platform
	pick_tools
	[ "$(id -u)" != 0 ] || root_gate
	tmp=$(mktemp -d "${TMPDIR:-/tmp}/ttyak-install.XXXXXX") || die "could not make a temporary folder"
	choose_version
	asset=ttyak-$version-$platform.zip
	base=$DOWNLOAD_URL/v$version
	say "Downloading ttyak $version for $platform"
	fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS" || die "could not download $base/SHA256SUMS: is $version a ttyak release?"
	want=$(awk -v f="$asset" '$2 == f || $2 == "*" f { print tolower($1); exit }' "$tmp/SHA256SUMS")
	[ -n "$want" ] || die "ttyak $version has no build for $platform"
	case $want in *[!0-9a-f]*) die "SHA256SUMS has no proper checksum for $asset" ;; esac
	[ ${#want} -eq 64 ] || die "SHA256SUMS has no proper checksum for $asset"
	fetch "$base/$asset" "$tmp/$asset" || die "could not download $base/$asset"
	got=$(sha256_of "$tmp/$asset")
	[ "$got" = "$want" ] || die "checksum mismatch for $asset: SHA256SUMS says $want, the download is $got; nothing was installed"
	mkdir -- "$tmp/unpacked"
	unpack "$tmp/$asset" "$tmp/unpacked" || die "could not unpack $asset"
	[ -f "$tmp/unpacked/ttyak" ] || die "$asset has no ttyak in it"
	make_dir
	[ ! -d "$dir/ttyak" ] || die "$dir/ttyak is a folder"
	updating=''
	old=''
	if [ -e "$dir/ttyak" ]; then
		updating=1
		old=$(json_version "$dir/install.json")
	fi
	staged=$dir/.ttyak.new
	cp -- "$tmp/unpacked/ttyak" "$staged"
	chmod 755 "$staged"
	mv -f -- "$staged" "$dir/ttyak"
	staged=''
	write_record
	if [ -z "$updating" ]; then
		say "Installed ttyak $version as $dir/ttyak"
	elif [ -n "$old" ] && [ "$old" != "$version" ]; then
		say "Updated ttyak $old to $version in $dir"
	else
		say "Installed ttyak $version over the copy in $dir"
	fi
	if ! ran=$("$dir/ttyak" --version </dev/null 2>&1); then
		warn "installed, but \`$dir/ttyak --version\` failed: $ran"
	fi
	add_to_path
	say "To remove it later: ttyak uninstall"
}

# Hands over to `ttyak uninstall`, which asks whether to keep the data unless told.
uninstall_ttyak() {
	exe=$dir/ttyak
	[ -f "$exe" ] && [ -x "$exe" ] || die "no ttyak in $dir: pass --dir if it was installed somewhere else"
	set -- uninstall
	if is_on "$yes"; then set -- "$@" --yes; fi
	if is_on "$keep_data"; then set -- "$@" --keep-data; fi
	if is_on "$yes"; then
		exec "$exe" "$@" </dev/null
	fi
	have_tty || die "no terminal to ask whether to keep your data: pass --yes to remove ttyak and its data, or --yes --keep-data to keep the data"
	exec "$exe" "$@" </dev/tty
}

cleanup() {
	if [ -n "$staged" ]; then rm -f -- "$staged"; fi
	if [ -n "$tmp" ] && [ -d "$tmp" ]; then rm -rf -- "$tmp"; fi
}

main() {
	yes=${TTYAK_YES:-}
	version=${TTYAK_VERSION:-}
	dir=${TTYAK_DIR:-}
	uninstall=${TTYAK_UNINSTALL:-}
	keep_data=${TTYAK_KEEP_DATA:-}
	while [ $# -gt 0 ]; do
		case $1 in
		-y | --yes) yes=1 ;;
		--version)
			[ $# -ge 2 ] || die "--version needs a version, e.g. --version 0.10.0"
			version=$2
			shift
			;;
		--version=*) version=${1#*=} ;;
		--dir)
			[ $# -ge 2 ] || die "--dir needs a folder"
			dir=$2
			shift
			;;
		--dir=*) dir=${1#*=} ;;
		--uninstall) uninstall=1 ;;
		--keep-data) keep_data=1 ;;
		-h | --help)
			usage
			exit 0
			;;
		*) die "unknown option: $1 (see --help)" ;;
		esac
		shift
	done
	[ -n "${HOME:-}" ] || die "HOME is not set"
	dir_given=$dir
	# shellcheck disable=SC2088 # a literal ~ from --dir=~/…
	case $dir in
	'') dir=$HOME/.local/bin ;;
	'~') dir=$HOME ;;
	'~/'*) dir=$HOME/${dir#'~/'} ;;
	/*) ;;
	*) dir=$(pwd)/$dir ;;
	esac
	trap cleanup EXIT
	trap 'exit 1' HUP INT TERM
	if is_on "$uninstall"; then
		uninstall_ttyak
	else
		install_ttyak
	fi
}

# Last, so that a download cut short runs nothing.
main "$@"

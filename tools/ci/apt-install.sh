#!/usr/bin/env bash
# CI's apt-get install, bounded and verbose (an apt-get once sat silent for 84 minutes): the packages are downloaded
# into DIR (the workflow may cache DIR/*.deb), then installed from it. A download that fails or takes too long is tried
# again from http://archive.ubuntu.com/ubuntu/ instead of the runner's own mirror: x86_64 runners read
# mirror+file:/etc/apt/apt-mirrors.txt, Azure's mirror first, which fails over only on errors, never when it is slow
# (arm64 runners' ports.ubuntu.com has no other mirror and is tried again as it is). Afterwards DIR keeps only the
# .debs that can still be downloaded.
# Usage: bash tools/ci/apt-install.sh DIR PACKAGE...   (DIR absolute)
# At most 3 x (120 s + 240 s) + 180 s + 60 s = 22 min: the step's limit must be above that.
set -euo pipefail

[ $# -ge 2 ] && [ "${1#/}" != "$1" ] || { echo "usage: bash tools/ci/apt-install.sh DIR PACKAGE...  (DIR absolute)" >&2; exit 2; }
dir=$1
shift
fallback=http://archive.ubuntu.com/ubuntu/
# The runner's own mirror as its Ubuntu sources name it (22.04: sources.list, 24.04: deb822 ubuntu.sources).
default='mirror\+file:/etc/apt/apt-mirrors\.txt/?|https?://azure\.archive\.ubuntu\.com/ubuntu/?'
sources=(/etc/apt/sources.list /etc/apt/sources.list.d/ubuntu.sources)
apt=(env DEBIAN_FRONTEND=noninteractive apt-get -o Acquire::Retries=3 -o Acquire::http::Timeout=30
	-o DPkg::Lock::Timeout=120 -o "Dir::Cache::archives=$dir" -o APT::Keep-Downloaded-Packages=true)

# The Ubuntu archive URIs the sources use now.
mirrors() {
	grep -hE '^(URIs:|deb )' "${sources[@]}" 2> /dev/null | grep -oE '(mirror\+file:|https?://)[^[:space:]]+' |
		sort -u | paste -sd ' ' - || true
}

# Once: the runner's mirror replaced by the fallback in the Ubuntu sources, when they name it.
switched=false
use_fallback() {
	[ "$switched" = false ] || return 0
	switched=true
	local files
	mapfile -t files < <(grep -lE "$default" "${sources[@]}" 2> /dev/null || true)
	if [ ${#files[@]} -eq 0 ]; then
		echo "::notice::apt: no Azure mirror in the sources ($(mirrors)) to switch from; trying them again"
		return 0
	fi
	sudo sed -i -E "s#$default#$fallback#g" "${files[@]}"
	echo "apt: ${files[*]} switched to $fallback"
}

sudo mkdir -p "$dir/partial"
echo "apt: $(find "$dir" -maxdepth 1 -name '*.deb' | wc -l) .deb files in $dir before downloading; mirrors: $(mirrors)"
# Each attempt: the lists under 120 s, the download under 240 s (a later attempt picks up a stalled file where it
# stopped).
for attempt in 1 2 3; do
	[ "$attempt" -eq 1 ] || use_fallback
	if sudo timeout 120 "${apt[@]}" update -qq &&
		sudo timeout 240 "${apt[@]}" install -y --no-install-recommends --download-only "$@"; then
		echo "apt: downloaded on attempt $attempt of 3 from $(mirrors)"
		break
	fi
	[ "$attempt" -lt 3 ] || { echo "::error::apt-get could not download the packages in 3 attempts"; exit 1; }
	echo "::warning::apt-get attempt $attempt failed or took too long (lists over 120 s or download over 240 s); trying again"
done
sudo timeout 180 "${apt[@]}" install -y --no-install-recommends --no-download "$@"
# Older versions out. (A cache takes DIR/*.deb: apt's lock and partial folder there are root's and _apt's.)
sudo timeout 60 "${apt[@]}" autoclean -qq
echo "apt: installed; $(find "$dir" -maxdepth 1 -name '*.deb' | wc -l) .deb files in $dir"

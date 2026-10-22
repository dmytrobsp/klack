#!/bin/sh
# Build and test a clean checkout of this repository in a fresh container, the way a contributor
# on another distribution would: only the packages listed below are installed, then rustup's
# stable toolchain. Usage: tools/container-build.sh debian|ubuntu|fedora|arch [cargo args…]
# (default: `cargo build --locked && cargo test --locked`). Needs podman; the repository is
# mounted read-only and cloned inside, so only committed work is built.
set -eu
distro=${1:?usage: container-build.sh debian|ubuntu|fedora|arch [cargo args…]}
shift
case "$distro" in
  debian) image=docker.io/library/debian:trixie
          setup="apt-get update -qq && apt-get install -y -qq curl ca-certificates git build-essential pkg-config cmake libasound2-dev >/dev/null" ;;
  ubuntu) image=docker.io/library/ubuntu:24.04
          setup="apt-get update -qq && apt-get install -y -qq curl ca-certificates git build-essential pkg-config cmake libasound2-dev >/dev/null" ;;
  fedora) image=registry.fedoraproject.org/fedora:latest
          setup="dnf install -y -q curl git gcc gcc-c++ make pkgconf-pkg-config cmake alsa-lib-devel >/dev/null" ;;
  arch)   image=docker.io/library/archlinux:latest
          setup="pacman -Syu --noconfirm --quiet curl git base-devel cmake alsa-lib >/dev/null" ;;
  *) echo "unknown distro: $distro" >&2; exit 2 ;;
esac
repo=$(cd "$(dirname "$0")/.." && pwd)
cmd=${*:-"build --locked && cargo test --locked"}
exec podman run --rm -v "$repo:/src:ro,Z" -v ttyak-cargo-registry:/root/.cargo/registry "$image" sh -ec "
  $setup
  curl -sSf https://sh.rustup.rs | sh -s -- -y -q --profile minimal >/dev/null
  . /root/.cargo/env
  git clone -q /src /build && cd /build
  echo \"== $distro: \$(rustc --version)\"
  cargo $cmd
"

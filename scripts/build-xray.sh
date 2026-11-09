#!/usr/bin/env bash
# Builds Xray-core from UniRay's fork for a Rust target triple:
#
#   scripts/build-xray.sh <target triple> <output file>
#
# The fork (mobilitylabsorg/Xray-core, branch uniray-v26.11.9) is the pinned
# Xray release, whose TUN runs on Xray's own small TCP/IP stack, with loss
# handling for a lossy way to the clients: DupThresh instead of the first SACK,
# a cwnd floor from the delivery rate (as BBR), and a quick resend of a lost
# retransmission, both off in congestion. Stock v26.11.9 drops to 7 MB/s once
# the LAN leg loses 2% of its packets, the fork keeps 94. See README.md#tun-bbr.
#
# Xray needs no cgo, so every target cross-compiles from anywhere with Go
# (the version the fork's go.mod asks for; GOTOOLCHAIN=auto fetches it).
# Android is the exception: as upstream's Android release, it links with cgo
# against the NDK's clang (NDK_HOME or ANDROID_NDK_HOME, or CC). For 32-bit
# ARM Android, where upstream publishes no Android build, use the Linux one
# (armv7-unknown-linux-*): Go's static Linux binaries run on Android as they
# are. The source is kept in build/cache/xray-<commit>.
set -euo pipefail

XRAY_REPO="https://github.com/mobilitylabsorg/Xray-core.git"
# Branch uniray-v26.11.9: Xray-core v26.11.9 + go1.27.3 + loss handling in TUN.
XRAY_COMMIT="cf2685b4c306946ccb1d21f7c0f04496d3870b01"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target="$1" out="$2"

case "$target" in
  x86_64-unknown-linux-*) goos=linux goarch=amd64 ;;
  aarch64-unknown-linux-*) goos=linux goarch=arm64 ;;
  armv7-unknown-linux-*) goos=linux goarch=arm GOARM=7 ;;
  x86_64-pc-windows-*) goos=windows goarch=amd64 ;;
  aarch64-pc-windows-*) goos=windows goarch=arm64 ;;
  x86_64-apple-darwin) goos=darwin goarch=amd64 ;;
  aarch64-apple-darwin) goos=darwin goarch=arm64 ;;
  aarch64-linux-android) goos=android goarch=arm64 ;;
  *) echo "build-xray.sh: no Xray build for $target" >&2; exit 1 ;;
esac
export GOARM="${GOARM:-}"

cgo=0
ldflags=""
if [[ "$goos" == android ]]; then
  cgo=1
  # Xray reaches into net internals (anet) that the linker otherwise refuses.
  ldflags="-checklinkname=0"
  if [[ -z "${CC:-}" ]]; then
    ndk="${NDK_HOME:-${ANDROID_NDK_HOME:-}}"
    host="$(uname -s | tr '[:upper:]' '[:lower:]')-x86_64"
    # API 24: the app's minSdk.
    CC="$ndk/toolchains/llvm/prebuilt/$host/bin/aarch64-linux-android24-clang"
    [[ -n "$ndk" && -x "$CC" ]] || {
      echo "build-xray.sh: $target needs the Android NDK: set NDK_HOME (or CC to its aarch64-linux-android24-clang)" >&2
      exit 1
    }
    export CC
  fi
fi

if ! command -v go >/dev/null; then
  echo "build-xray.sh: Go is needed to build Xray (https://go.dev/dl/)" >&2
  exit 1
fi

src="$root/build/cache/xray-$XRAY_COMMIT"
if [[ ! -f "$src/.uniray-fetched" ]]; then
  rm -rf "$src"
  mkdir -p "$src"
  git -C "$src" init -q
  git -C "$src" fetch -q --depth 1 "$XRAY_REPO" "$XRAY_COMMIT"
  git -C "$src" checkout -q FETCH_HEAD
  touch "$src/.uniray-fetched"
fi

mkdir -p "$(dirname "$out")"
out="$(cd "$(dirname "$out")" && pwd)/$(basename "$out")"
cd "$src"
export GOTOOLCHAIN="${GOTOOLCHAIN:-auto}"
CGO_ENABLED=$cgo GOOS=$goos GOARCH=$goarch \
  go build -trimpath -buildvcs=false \
  -ldflags "-X github.com/xtls/xray-core/core.build=uniray-${XRAY_COMMIT:0:7} $ldflags -s -w -buildid=" \
  -o "$out" ./main

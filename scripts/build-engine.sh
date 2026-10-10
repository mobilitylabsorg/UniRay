#!/usr/bin/env bash
# Builds uniray-engine (engine/, Go >= 1.25) for a Rust target triple:
#
#   scripts/build-engine.sh <target triple> <output file>
#
# Desktop builds are sidecars (prepare-sidecars.sh). The Android ones go to
#   src-tauri/gen/android/app/src/main/jniLibs/<abi>/libuniray_engine.so
# where Android extracts it as a native library, the only place an app may
# run a program from.
#
# Linux, Windows and 64-bit Android builds are static (no cgo) and
# cross-compile from anywhere. 32-bit Android (armeabi-v7a, most TV boxes)
# needs cgo, as Go links android/arm only externally: the NDK's clang
# (NDK_HOME or ANDROID_NDK_HOME, or CC). macOS needs cgo too (libproc, to
# check which process connects), so it is built on a Mac; CC can point at a
# cross toolchain otherwise.
set -euo pipefail

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
  armv7-linux-androideabi) goos=android goarch=arm GOARM=7 ;;
  *) echo "build-engine.sh: no engine build for $target" >&2; exit 1 ;;
esac
export GOARM="${GOARM:-}"

if ! command -v go >/dev/null; then
  echo "build-engine.sh: Go 1.25 or newer is needed to build the engine (https://go.dev/dl/)" >&2
  exit 1
fi

cgo=0
if [[ "$goos" == darwin ]]; then
  cgo=1
  # Apple's clang builds either architecture with -arch.
  [[ -z "${CC:-}" && "$(uname -s)" == Darwin ]] && export CC="clang -arch ${goarch/amd64/x86_64}"
fi
if [[ "$goos" == android && "$goarch" == arm ]]; then
  cgo=1
  if [[ -z "${CC:-}" ]]; then
    ndk="${NDK_HOME:-${ANDROID_NDK_HOME:-}}"
    host="$(uname -s | tr '[:upper:]' '[:lower:]')-x86_64"
    # API 24: the app's minSdk.
    CC="$ndk/toolchains/llvm/prebuilt/$host/bin/armv7a-linux-androideabi24-clang"
    [[ -n "$ndk" && -x "$CC" ]] || {
      echo "build-engine.sh: $target needs the Android NDK: set NDK_HOME (or CC to its armv7a-linux-androideabi24-clang)" >&2
      exit 1
    }
    export CC
  fi
fi

mkdir -p "$(dirname "$out")"
out="$(cd "$(dirname "$out")" && pwd)/$(basename "$out")"
cd "$root/engine"
export GOTOOLCHAIN="${GOTOOLCHAIN:-auto}"
tags=()

# ENGINE_LOWMEM=1: for a small memory limit (the MikroTik image's -lowmem
# engine). amneziawg-go's packet buffer pools, unbounded on Linux, hold at
# most 384 buffers of 64 KiB, and a reader waits for a free one instead of
# growing (as amneziawg-go does on Android and iOS); TCP's send buffer stays
# at gVisor's 1 MiB. Both come from the uniray_lowmem build tag of the forks
# go.mod points at (see engine/tcp_lowmem.go and mobilitylabsorg/amneziawg-go).
if [[ "${ENGINE_LOWMEM:-}" == 1 ]]; then
  tags=(-tags uniray_lowmem)
fi

CGO_ENABLED=$cgo GOOS=$goos GOARCH=$goarch \
  go build ${tags[@]+"${tags[@]}"} -trimpath -buildvcs=false -ldflags "-s -w -buildid=" -o "$out" .

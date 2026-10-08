#!/usr/bin/env bash
# Builds uniray-engine (engine/, Go >= 1.25) for a Rust target triple:
#
#   scripts/build-engine.sh <target triple> <output file>
#
# Desktop builds are sidecars (prepare-sidecars.sh). The Android one goes to
#   src-tauri/gen/android/app/src/main/jniLibs/arm64-v8a/libuniray_engine.so
# where Android extracts it as a native library, the only place an app may
# run a program from.
#
# Linux, Windows and Android builds are static (no cgo) and cross-compile
# from anywhere. macOS needs cgo (libproc, to check which process connects),
# so it is built on a Mac; CC can point at a cross toolchain otherwise.
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

mkdir -p "$(dirname "$out")"
out="$(cd "$(dirname "$out")" && pwd)/$(basename "$out")"
cd "$root/engine"
CGO_ENABLED=$cgo GOOS=$goos GOARCH=$goarch GOTOOLCHAIN="${GOTOOLCHAIN:-auto}" \
  go build -trimpath -buildvcs=false -ldflags "-s -w -buildid=" -o "$out" .

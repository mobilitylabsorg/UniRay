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
export GOTOOLCHAIN="${GOTOOLCHAIN:-auto}"
modfile=()

# ENGINE_MAX_BUFFERS=<n>: at most n packet buffers per pool, for a small
# memory limit (the MikroTik container). Each in-flight packet takes a 64 KiB
# buffer, and on Linux amneziawg-go's pools are unbounded: a fast download
# fills its 1024-packet queues, 64 MiB each. With a bound, a full pool makes
# the receiver wait instead, as amneziawg-go does on Android (4096) and iOS
# (1024). The readers hold a batch of buffers each before any packet comes
# (two UDP readers, IPv4 and IPv6, and the TUN reader): ENGINE_BATCH_SIZE
# (default 64, amneziawg-go's is 128) keeps that part of the bound small, or
# the TUN reader would wait for a buffer forever. It must be a multiple of
# 64, the most datagrams the kernel coalesces into one read (UDP GRO).
# The constants are changed in a copy of the module, built in through a
# replace directive in a copy of go.mod; the sources here stay as they are.
if [[ -n "${ENGINE_MAX_BUFFERS:-}" ]]; then
  batch="${ENGINE_BATCH_SIZE:-64}"
  [[ "$ENGINE_MAX_BUFFERS" =~ ^[1-9][0-9]*$ && "$batch" =~ ^[1-9][0-9]*$ ]] \
    || { echo "build-engine.sh: bad ENGINE_MAX_BUFFERS or ENGINE_BATCH_SIZE" >&2; exit 1; }
  if (( batch % 64 != 0 )); then
    echo "build-engine.sh: ENGINE_BATCH_SIZE must be a multiple of 64" >&2
    exit 1
  fi
  # Three readers' batches, and as much again for packets on their way.
  if (( ENGINE_MAX_BUFFERS < 6 * batch )); then
    echo "build-engine.sh: ENGINE_MAX_BUFFERS must be at least 6 * ENGINE_BATCH_SIZE ($((6 * batch)))" >&2
    exit 1
  fi
  awg_module=github.com/amnezia-vpn/amneziawg-go/v3
  work="$(mktemp -d)"
  trap 'chmod -R u+w "$work" 2>/dev/null; rm -rf "$work"' EXIT
  go mod download "$awg_module"
  cp -R "$(go list -m -f '{{.Dir}}' "$awg_module")" "$work/amneziawg-go"
  chmod -R u+w "$work/amneziawg-go"
  constants="$work/amneziawg-go/device/queueconstants_default.go"
  sed -i.orig -E "s/^([[:space:]]*PreallocatedBuffersPerPool[[:space:]]*=[[:space:]]*)0([[:space:]]|$)/\1$ENGINE_MAX_BUFFERS\2/" "$constants"
  batches="$work/amneziawg-go/conn/conn.go"
  sed -i.orig -E "s/^([[:space:]]*IdealBatchSize[[:space:]]*=[[:space:]]*)128([[:space:]]|$)/\1$batch\2/" "$batches"
  if ! grep -Eq "PreallocatedBuffersPerPool[[:space:]]*=[[:space:]]*$ENGINE_MAX_BUFFERS([[:space:]]|$)" "$constants" \
    || ! grep -Eq "IdealBatchSize[[:space:]]*=[[:space:]]*$batch([[:space:]]|$)" "$batches"; then
    echo "build-engine.sh: $awg_module changed; cannot bound its buffers (see $constants, $batches)" >&2
    exit 1
  fi
  cp go.mod go.sum "$work/"
  go mod edit -modfile="$work/go.mod" -replace "$awg_module=$work/amneziawg-go"
  modfile=(-modfile="$work/go.mod")
fi

CGO_ENABLED=$cgo GOOS=$goos GOARCH=$goarch \
  go build "${modfile[@]}" -trimpath -buildvcs=false -ldflags "-s -w -buildid=" -o "$out" .

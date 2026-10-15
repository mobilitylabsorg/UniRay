#!/usr/bin/env bash
# Puts the desktop sidecars where Tauri's `externalBin` expects them:
#
#   src-tauri/binaries/uniray-xray-<target triple>     built from UniRay's Xray fork, see build-xray.sh
#   src-tauri/binaries/uniray-helper-<target triple>   built from crates/uniray-helper
#   src-tauri/binaries/uniray-engine-<target triple>   built from engine/ (Go), see build-engine.sh
#   src-tauri/binaries/uniray-naive-<target triple>    downloaded, see fetch-naive.sh
#
# Usage: scripts/prepare-sidecars.sh [--release] [target triple]
# Runs before `tauri dev` and `tauri build` (see package.json).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
profile="debug"
cargo_args=()
if [[ "${1:-}" == "--release" ]]; then
  profile="release"
  cargo_args+=(--release)
  shift
fi
host="$(rustc -vV | sed -n 's/^host: //p')"
target="${1:-$host}"

exe=""
[[ "$target" == *windows* ]] && exe=".exe"

needs_wintun=false
[[ "$target" == *windows* && ! -f "$root/src-tauri/resources/wintun.dll" ]] && needs_wintun=true
# The geo databases (and wintun) from the Xray release; Xray itself from the
# fork, every time, as the engine: the Go build cache makes it quick, and a
# workspace that kept an older sidecar (CI's) gets the pinned one.
if [[ ! -f "$root/src-tauri/resources/geoip.dat" || $needs_wintun == true ]]; then
  "$root/scripts/fetch-xray.sh" "$target"
fi
"$root/scripts/build-xray.sh" "$target" "$root/src-tauri/binaries/uniray-xray-$target$exe"

out="$root/target/$profile"
if [[ "$target" != "$host" ]]; then
  cargo_args+=(--target "$target")
  out="$root/target/$target/$profile"
fi
# Windows MSVC binaries are cross-built with cargo-xwin, which supplies the
# Windows SDK and CRT.
build=(cargo build)
[[ "$target" == *windows-msvc && "$host" != *windows* ]] && build=(cargo xwin build)
"${build[@]}" --manifest-path "$root/Cargo.toml" -p uniray-helper "${cargo_args[@]}"
install -m 755 "$out/uniray-helper$exe" "$root/src-tauri/binaries/uniray-helper-$target$exe"
"$root/scripts/build-engine.sh" "$target" "$root/src-tauri/binaries/uniray-engine-$target$exe"
naive="$root/src-tauri/binaries/uniray-naive-$target$exe"
[[ -f "$naive" ]] || "$root/scripts/fetch-naive.sh" "$target" "$naive"

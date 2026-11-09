#!/usr/bin/env bash
# Downloads the pinned Xray-core release for a desktop target and installs the
# data files the app bundles:
#
#   src-tauri/resources/{geoip.dat,geosite.dat}   (and wintun.dll + LICENSE-Wintun on Windows)
#
# Usage: scripts/fetch-xray.sh [target triple]    (default: the host triple)
#
# The Xray executable itself comes from UniRay's fork, see build-xray.sh
# (prepare-sidecars.sh runs both); keep XRAY_VERSION the release that fork is
# based on. Archives are verified against the SHA-256 digests published with
# the release; bump XRAY_VERSION and the digests together.
set -euo pipefail

XRAY_VERSION="v26.11.9"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target="${1:-$(rustc -vV | sed -n 's/^host: //p')}"

case "$target" in
  x86_64-unknown-linux-gnu)   asset="linux-64";          sha256="b651f7e1d9966ee2c1b1ee1bf8b52c72771824d3065dce96bbc8bcc19ed95f82" ;;
  aarch64-unknown-linux-gnu)  asset="linux-arm64-v8a";   sha256="64ed6858982a7f5eb85d88b811089063bd4f17561f4c8f44e0f1dc8e6f6430be" ;;
  x86_64-pc-windows-msvc)     asset="windows-64";        sha256="1262522266337b824f07beb8e1375786b437683b3890105469088f8e17e0a69d" ;;
  aarch64-pc-windows-msvc)    asset="windows-arm64-v8a"; sha256="9fdd48ff890e54ffdc37f92df9037c940fe4d4f914755b70cc644b97e9678a83" ;;
  x86_64-apple-darwin)        asset="macos-64";          sha256="488963e676e06f5e48547f9dd38116aaca8de4234959b4272ae8f284e658dfd0" ;;
  aarch64-apple-darwin)       asset="macos-arm64-v8a";   sha256="233f0c66681cd0726f81a6b2b6ed6a5710869e312047b522e46ed53035bebd26" ;;
  *) echo "fetch-xray: unsupported target $target" >&2; exit 1 ;;
esac

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

url="https://github.com/XTLS/Xray-core/releases/download/${XRAY_VERSION}/Xray-${asset}.zip"
echo "fetch-xray: downloading $url"
curl -fL --retry 3 -o "$work/xray.zip" "$url"

if command -v sha256sum >/dev/null; then
  actual="$(sha256sum "$work/xray.zip" | cut -d' ' -f1)"
else
  actual="$(shasum -a 256 "$work/xray.zip" | cut -d' ' -f1)"
fi
if [[ "$actual" != "$sha256" ]]; then
  echo "fetch-xray: checksum mismatch for Xray-${asset}.zip" >&2
  echo "  expected $sha256" >&2
  echo "  actual   $actual" >&2
  exit 1
fi

unzip -q "$work/xray.zip" -d "$work/xray"
mkdir -p "$root/src-tauri/resources"
install -m 644 "$work/xray/geoip.dat" "$work/xray/geosite.dat" "$root/src-tauri/resources/"
# Windows TUN: Xray loads wintun.dll from its own directory (the Windows
# bundle config puts it there), and its license travels with it.
if [[ -f "$work/xray/wintun.dll" ]]; then
  install -m 644 "$work/xray/wintun.dll" "$work/xray/LICENSE-Wintun" "$root/src-tauri/resources/"
fi

echo "fetch-xray: installed the Xray $XRAY_VERSION data files for $target"

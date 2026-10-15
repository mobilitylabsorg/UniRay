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

XRAY_VERSION="v26.9.30"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target="${1:-$(rustc -vV | sed -n 's/^host: //p')}"

case "$target" in
  x86_64-unknown-linux-gnu)   asset="linux-64";          sha256="f851110beaff16e78d643f0ccfd9524b4a44dfd59bae3e34bb52bba378f7690e" ;;
  aarch64-unknown-linux-gnu)  asset="linux-arm64-v8a";   sha256="9886f077f9fd8e6713b84c377c1c7db4e53b9bfa8c276a5bd12561139522b473" ;;
  x86_64-pc-windows-msvc)     asset="windows-64";        sha256="b17a619343c11b89d8faf36278298856749b15c52277d875d32051b33dcda617" ;;
  aarch64-pc-windows-msvc)    asset="windows-arm64-v8a"; sha256="5cf7ad4fd9aac5aa248ae7e598f3f0a0d0f79a6195089d611ddef9653830f1b0" ;;
  x86_64-apple-darwin)        asset="macos-64";          sha256="1f366aaf21d3c3003d1556d066c4e08f9dd62d1f96555ecedde85b397df0f684" ;;
  aarch64-apple-darwin)       asset="macos-arm64-v8a";   sha256="4b363bd924df5bf09f87bd445755ff8e5742b9a8a0480cda261061416c3c7dce" ;;
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

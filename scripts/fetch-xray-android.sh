#!/usr/bin/env bash
# Downloads the pinned Xray-core release for Android arm64 and installs the
# standalone `xray` binary as an app asset:
#
#   src-tauri/gen/android/app/src/main/assets/stealth/xray
#
# This binary is only used in stealth mode, where it runs as a separate root
# process (it needs CAP_NET_ADMIN for tproxy/TUN, which the in-process libXray
# cannot have). The geo databases it uses are the ones fetch-xray.sh already
# installs; this script only fetches the executable.
#
# The archive is verified against the SHA-256 digest published with the
# release; bump XRAY_VERSION and the digest together with fetch-xray.sh.
set -euo pipefail

XRAY_VERSION="v26.9.30"
asset="android-arm64-v8a"
sha256="dbe43ff5a3547ecf8bce2f10af802cdbb97f1eb9af5b6dddd04dbcf3105ede8e"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dest="$root/src-tauri/gen/android/app/src/main/assets/stealth"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

url="https://github.com/XTLS/Xray-core/releases/download/${XRAY_VERSION}/Xray-${asset}.zip"
echo "fetch-xray-android: downloading $url"
curl -fL --retry 3 -o "$work/xray.zip" "$url"

if command -v sha256sum >/dev/null; then
  actual="$(sha256sum "$work/xray.zip" | cut -d' ' -f1)"
else
  actual="$(shasum -a 256 "$work/xray.zip" | cut -d' ' -f1)"
fi
if [[ "$actual" != "$sha256" ]]; then
  echo "fetch-xray-android: checksum mismatch for Xray-${asset}.zip" >&2
  echo "  expected $sha256" >&2
  echo "  actual   $actual" >&2
  exit 1
fi

unzip -q "$work/xray.zip" xray -d "$work"
mkdir -p "$dest"
# Packaged read-only: the exec bit does not survive APK packaging anyway, so
# the service chmods the copy it unpacks into its private dir before running.
install -m 644 "$work/xray" "$dest/xray"

echo "fetch-xray-android: installed Xray $XRAY_VERSION ($asset) at $dest/xray"

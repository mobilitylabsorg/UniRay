#!/usr/bin/env bash
# Downloads the pinned Xray-core release for arm64 and installs two standalone
# builds as app assets:
#
#   src-tauri/gen/android/app/src/main/assets/stealth/xray      Android build (tproxy)
#   src-tauri/gen/android/app/src/main/assets/stealth/xray-tun  Linux build (root TUN)
#
# The Android build of Xray never creates a TUN interface: it expects the file
# descriptor of one opened by a VpnService. The root TUN mode needs Xray to
# make its own, which only the Linux build does; Go's static Linux binaries
# run on Android as they are.
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

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dest="$root/src-tauri/gen/android/app/src/main/assets/stealth"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# fetch <asset> <sha256> <installed name>
fetch() {
  local asset="$1" sha256="$2" name="$3" actual
  local url="https://github.com/XTLS/Xray-core/releases/download/${XRAY_VERSION}/Xray-${asset}.zip"
  echo "fetch-xray-android: downloading $url"
  curl -fL --retry 3 -o "$work/$asset.zip" "$url"

  if command -v sha256sum >/dev/null; then
    actual="$(sha256sum "$work/$asset.zip" | cut -d' ' -f1)"
  else
    actual="$(shasum -a 256 "$work/$asset.zip" | cut -d' ' -f1)"
  fi
  if [[ "$actual" != "$sha256" ]]; then
    echo "fetch-xray-android: checksum mismatch for Xray-${asset}.zip" >&2
    echo "  expected $sha256" >&2
    echo "  actual   $actual" >&2
    exit 1
  fi

  mkdir -p "$work/$asset" "$dest"
  unzip -q "$work/$asset.zip" xray -d "$work/$asset"
  # Packaged read-only: the exec bit does not survive APK packaging anyway, so
  # the service chmods the copy it unpacks into its private dir before running.
  install -m 644 "$work/$asset/xray" "$dest/$name"
  echo "fetch-xray-android: installed Xray $XRAY_VERSION ($asset) at $dest/$name"
}

fetch android-arm64-v8a dbe43ff5a3547ecf8bce2f10af802cdbb97f1eb9af5b6dddd04dbcf3105ede8e xray
fetch linux-arm64-v8a 9886f077f9fd8e6713b84c377c1c7db4e53b9bfa8c276a5bd12561139522b473 xray-tun

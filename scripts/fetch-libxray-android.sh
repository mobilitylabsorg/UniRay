#!/usr/bin/env bash
# Downloads the pinned libXray release for Android (the Xray core as a gomobile
# AAR) into src-tauri/gen/android/app/libs/, and makes sure the geo files the
# APK bundles are present.
#
# The archive is verified against the SHA-256 digest published with the
# release; bump LIBXRAY_VERSION and the digest together, and keep the version
# in step with XRAY_VERSION in fetch-xray.sh.
set -euo pipefail

LIBXRAY_VERSION="v26.9.30"
SHA256="394b59d147b37ab46003bd3f1d687ed3b2f637cabfc0cb863f179c5a87bddde3"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
libs="$root/src-tauri/gen/android/app/libs"

if [[ ! -f "$root/src-tauri/resources/geoip.dat" || ! -f "$root/src-tauri/resources/geosite.dat" ]]; then
  "$root/scripts/fetch-xray.sh"
fi

stamp="$libs/libXray.version"
if [[ -f "$libs/libXray.aar" && -f "$stamp" && "$(cat "$stamp")" == "$LIBXRAY_VERSION" ]]; then
  exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

url="https://github.com/XTLS/libXray/releases/download/${LIBXRAY_VERSION}/libxray-android.zip"
echo "fetch-libxray-android: downloading $url"
curl -fL --retry 3 -o "$work/libxray.zip" "$url"

if command -v sha256sum >/dev/null; then
  actual="$(sha256sum "$work/libxray.zip" | cut -d' ' -f1)"
else
  actual="$(shasum -a 256 "$work/libxray.zip" | cut -d' ' -f1)"
fi
if [[ "$actual" != "$SHA256" ]]; then
  echo "fetch-libxray-android: checksum mismatch for libxray-android.zip" >&2
  echo "  expected $SHA256" >&2
  echo "  actual   $actual" >&2
  exit 1
fi

unzip -q "$work/libxray.zip" -d "$work/libxray"
mkdir -p "$libs"
install -m 644 "$work/libxray/libxray-android/libXray.aar" "$libs/libXray.aar"
echo "$LIBXRAY_VERSION" > "$stamp"
echo "fetch-libxray-android: installed libXray $LIBXRAY_VERSION"

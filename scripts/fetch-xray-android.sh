#!/usr/bin/env bash
# Downloads the pinned Xray-core release for an Android ABI and installs two
# standalone builds as native libraries of that ABI:
#
#   scripts/fetch-xray-android.sh [target triple]   (default aarch64-linux-android)
#
#   src-tauri/gen/android/app/src/main/jniLibs/<abi>/libxray.so      tproxy mode
#   src-tauri/gen/android/app/src/main/jniLibs/<abi>/libxray_tun.so  root TUN mode
#
# As native libraries each per-ABI APK carries its own builds, and Android
# extracts them at install like libuniray_engine.so.
#
# The Android build of Xray never creates a TUN interface: it expects the file
# descriptor of one opened by a VpnService. The root TUN mode needs Xray to
# make its own, which only the Linux build does; Go's static Linux binaries
# run on Android as they are. For 32-bit ARM (most TV boxes) Xray publishes no
# Android build, so both modes run the static Linux one there; the stealth
# service gives it the Android CA directories (SSL_CERT_DIR), and Xray's own
# DNS resolves everything, so it needs no /etc/resolv.conf.
#
# These binaries are only used in stealth mode, where Xray runs as a separate
# root process (it needs CAP_NET_ADMIN for tproxy/TUN, which the in-process
# libXray cannot have). The geo databases it uses are the ones fetch-xray.sh
# already installs; this script only fetches the executables.
#
# The archives are verified against the SHA-256 digests published with the
# release; bump XRAY_VERSION and the digests together with fetch-xray.sh.
set -euo pipefail

XRAY_VERSION="v26.9.30"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target="${1:-aarch64-linux-android}"
case "$target" in
  aarch64-linux-android) abi=arm64-v8a ;;
  armv7-linux-androideabi) abi=armeabi-v7a ;;
  *) echo "fetch-xray-android: unsupported target $target" >&2; exit 1 ;;
esac
dest="$root/src-tauri/gen/android/app/src/main/jniLibs/$abi"

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
  unzip -oq "$work/$asset.zip" xray -d "$work/$asset"
  # The stealth service copies it into its private dir and runs that copy.
  install -m 755 "$work/$asset/xray" "$dest/$name"
  echo "fetch-xray-android: installed Xray $XRAY_VERSION ($asset) at $dest/$name"
}

case "$abi" in
  arm64-v8a)
    fetch android-arm64-v8a dbe43ff5a3547ecf8bce2f10af802cdbb97f1eb9af5b6dddd04dbcf3105ede8e libxray.so
    fetch linux-arm64-v8a 9886f077f9fd8e6713b84c377c1c7db4e53b9bfa8c276a5bd12561139522b473 libxray_tun.so ;;
  armeabi-v7a)
    fetch linux-arm32-v7a 0b9719471c7c69752857714e9711d4da57cf38a6beb75dcddfb21425f7919908 libxray.so
    fetch linux-arm32-v7a 0b9719471c7c69752857714e9711d4da57cf38a6beb75dcddfb21425f7919908 libxray_tun.so ;;
esac

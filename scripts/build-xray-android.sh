#!/usr/bin/env bash
# Builds UniRay's Xray (see build-xray.sh) for an Android ABI and installs two
# standalone builds as native libraries of that ABI:
#
#   scripts/build-xray-android.sh [target triple]   (default aarch64-linux-android)
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
# run on Android as they are. For 32-bit ARM (most TV boxes) there is no
# Android build, so both modes run the static Linux one there; the stealth
# service gives it the Android CA directories (SSL_CERT_DIR), and Xray's own
# DNS resolves everything, so it needs no /etc/resolv.conf. The arm64 Android
# build links with the NDK (NDK_HOME), as upstream's.
#
# These binaries are only used in stealth mode, where Xray runs as a separate
# root process (it needs CAP_NET_ADMIN for tproxy/TUN, which the in-process
# libXray cannot have). The geo databases it uses are the ones fetch-xray.sh
# already installs; this script only builds the executables.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target="${1:-aarch64-linux-android}"
case "$target" in
  aarch64-linux-android) abi=arm64-v8a ;;
  armv7-linux-androideabi) abi=armeabi-v7a ;;
  *) echo "build-xray-android: unsupported target $target" >&2; exit 1 ;;
esac
dest="$root/src-tauri/gen/android/app/src/main/jniLibs/$abi"
mkdir -p "$dest"

# The stealth service copies each into its private dir and runs that copy.
case "$abi" in
  arm64-v8a)
    "$root/scripts/build-xray.sh" aarch64-linux-android "$dest/libxray.so"
    "$root/scripts/build-xray.sh" aarch64-unknown-linux-musl "$dest/libxray_tun.so" ;;
  armeabi-v7a)
    "$root/scripts/build-xray.sh" armv7-unknown-linux-musleabihf "$dest/libxray.so"
    install -m 755 "$dest/libxray.so" "$dest/libxray_tun.so" ;;
esac
echo "build-xray-android: installed UniRay's Xray at $dest/libxray.so and libxray_tun.so"

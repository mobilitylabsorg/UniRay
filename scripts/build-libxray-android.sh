#!/usr/bin/env bash
# Builds libXray (the Xray core as a gomobile AAR, which the VPN mode runs in
# process) from UniRay's fork into src-tauri/gen/android/app/libs/, and makes
# sure the geo files the APK bundles are present.
#
# The fork (mobilitylabsorg/libXray, branch uniray) is libXray v26.9.30 with
# Xray-core and gVisor from the mobilitylabsorg forks, as build-xray.sh: the
# VPN mode's TUN gets BBRv3 and RACK. Keep LIBXRAY_COMMIT on the Xray commit
# build-xray.sh pins.
#
# Needs Go, the Android NDK (NDK_HOME or ANDROID_NDK_HOME) and the SDK
# (ANDROID_HOME). gomobile and gobind are built at the version libXray's go.mod
# pins. The source and the tools are kept in build/cache/.
set -euo pipefail

LIBXRAY_REPO="https://github.com/mobilitylabsorg/libXray.git"
# Branch uniray: libXray v26.9.30 + Xray-core uniray 14f6147 + mobilitylabsorg/gvisor.
LIBXRAY_COMMIT="db0f41f6346c447e40cde7baab5776802875a253"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
libs="$root/src-tauri/gen/android/app/libs"

if [[ ! -f "$root/src-tauri/resources/geoip.dat" || ! -f "$root/src-tauri/resources/geosite.dat" ]]; then
  "$root/scripts/fetch-xray.sh"
fi

stamp="$libs/libXray.version"
if [[ -f "$libs/libXray.aar" && -f "$stamp" && "$(cat "$stamp")" == "$LIBXRAY_COMMIT" ]]; then
  exit 0
fi

export ANDROID_NDK_HOME="${NDK_HOME:-${ANDROID_NDK_HOME:-}}"
if [[ -z "$ANDROID_NDK_HOME" || ! -d "$ANDROID_NDK_HOME" ]]; then
  echo "build-libxray-android: needs the Android NDK: set NDK_HOME" >&2
  exit 1
fi
if [[ -z "${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}" ]]; then
  echo "build-libxray-android: needs the Android SDK: set ANDROID_HOME" >&2
  exit 1
fi
export ANDROID_HOME="${ANDROID_HOME:-$ANDROID_SDK_ROOT}"

src="$root/build/cache/libxray-$LIBXRAY_COMMIT"
if [[ ! -f "$src/.uniray-fetched" ]]; then
  rm -rf "$src"
  mkdir -p "$src"
  git -C "$src" init -q
  git -C "$src" fetch -q --depth 1 "$LIBXRAY_REPO" "$LIBXRAY_COMMIT"
  git -C "$src" checkout -q FETCH_HEAD
  touch "$src/.uniray-fetched"
fi

cd "$src"
export GOTOOLCHAIN="${GOTOOLCHAIN:-auto}"
# gomobile runs gobind from PATH; both at the version go.mod pins (tools.go).
export GOBIN="$root/build/cache/gomobile-bin"
export PATH="$GOBIN:$PATH"
go install golang.org/x/mobile/cmd/gomobile golang.org/x/mobile/cmd/gobind
gomobile init

# As libXray's own Android build (build/app/android.py): API 21, 16 KiB pages.
rm -f libXray.aar libXray-sources.jar
gomobile bind -target android -androidapi 21 -trimpath \
  -ldflags="-checklinkname=0 -s -w -buildid= -extldflags=-Wl,-z,max-page-size=16384" \
  -o libXray.aar .

mkdir -p "$libs"
install -m 644 libXray.aar "$libs/libXray.aar"
echo "$LIBXRAY_COMMIT" > "$stamp"
echo "build-libxray-android: installed libXray ${LIBXRAY_COMMIT:0:7}"

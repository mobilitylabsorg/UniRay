#!/usr/bin/env bash
# Checks that a Google Play build (UNIRAY_STORE=play) carries none of what
# Play's policies rule out, so that it is not merely hidden:
#
#   scripts/check-play-edition.sh <app.aab | app.apk>
#
# - no stealth tunnel (StealthService, PrivilegedShell), libsu or Shizuku in
#   the code, and not its standalone root Xray (libxray.so, libxray_tun.so);
# - none of them, nor QUERY_ALL_PACKAGES or REQUEST_IGNORE_BATTERY_OPTIMIZATIONS,
#   in the manifest;
# - the Play application id, dev.uniray.app.play.
#
# An APK's manifest is binary XML (aapt2 from the Android SDK reads it);
# an AAB's is protobuf, whose strings grep finds as they are.
set -euo pipefail

package="$1"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

case "$package" in
  *.aab)
    unzip -q -o "$package" 'base/dex/*' 'base/manifest/AndroidManifest.xml' -d "$work"
    cat "$work"/base/dex/*.dex > "$work/code"
    manifest="$work/base/manifest/AndroidManifest.xml"
    ;;
  *.apk)
    unzip -q -o "$package" 'classes*.dex' -d "$work"
    cat "$work"/classes*.dex > "$work/code"
    sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
    aapt2="$(ls -d "$sdk"/build-tools/*/aapt2 2>/dev/null | sort -V | tail -1)"
    [[ -x "$aapt2" ]] || { echo "check-play-edition: aapt2 not found under \$ANDROID_HOME/build-tools" >&2; exit 1; }
    "$aapt2" dump xmltree --file AndroidManifest.xml "$package" > "$work/manifest.txt"
    manifest="$work/manifest.txt"
    ;;
  *) echo "check-play-edition: expected an .aab or an .apk" >&2; exit 1 ;;
esac

failed=0
for class in dev/uniray/app/StealthService dev/uniray/app/PrivilegedShell rikka/shizuku com/topjohnwu/superuser; do
  if grep -a -q "L$class" "$work/code"; then
    echo "check-play-edition: the code has $class" >&2
    failed=1
  fi
done
if unzip -l "$package" | grep -q -E 'lib/[^/]+/libxray(_tun)?\.so$'; then
  echo "check-play-edition: the package has stealth mode's standalone Xray (libxray.so, libxray_tun.so)" >&2
  failed=1
fi
for entry in StealthService ShizukuProvider moe.shizuku QUERY_ALL_PACKAGES REQUEST_IGNORE_BATTERY_OPTIMIZATIONS; do
  if grep -a -q "$entry" "$manifest"; then
    echo "check-play-edition: the manifest has $entry" >&2
    failed=1
  fi
done
if ! grep -a -q "dev.uniray.app.play" "$manifest"; then
  echo "check-play-edition: the application id is not dev.uniray.app.play" >&2
  failed=1
fi
[[ $failed == 0 ]] && echo "check-play-edition: $package is the Play edition, without stealth"
exit $failed

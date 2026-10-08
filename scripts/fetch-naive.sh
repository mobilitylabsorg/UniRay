#!/usr/bin/env bash
# Downloads NaiveProxy's client, `naive`, from the klzgrad/naiveproxy releases
# for a Rust target triple, and installs it where the app expects it:
#
#   scripts/fetch-naive.sh <target triple> <output file>
#
# Desktop: src-tauri/binaries/uniray-naive-<triple>[.exe] (a sidecar, see
# prepare-sidecars.sh); Android: the jniLibs as libnaive.so, extracted at
# install like libuniray_engine.so (see build-engine.sh). For Android the
# release has no archive, only the plugin APK for NekoBox and the like, whose
# lib/<abi>/libnaive.so is that same executable.
#
# The archive is checked against a pinned SHA-256 digest when pinned_sha256
# has one for the asset; otherwise against the digest GitHub publishes for the
# release asset, and the digest is printed so it can be pinned below. Pin
# NAIVE_VERSION and the digests together.
set -euo pipefail

# Empty: the latest release.
NAIVE_VERSION="${NAIVE_VERSION-v154.0.8037.49-4}"
# The digest of an asset of the pinned NAIVE_VERSION (a function, not an
# associative array: macOS ships bash 3).
pinned_sha256() {
  [[ "$NAIVE_VERSION" == v154.0.8037.49-4 ]] || return 0
  case "$1" in
    naiveproxy-v154.0.8037.49-4-win-x64.zip) echo 120b99474848d2f737515c1043249b0998e235fda750f61893d774b317bf1870 ;;
    *) echo "" ;;
  esac
}

target="$1" out="$2"
# The asset, with {v} for the version.
case "$target" in
  x86_64-unknown-linux-*) pattern="naiveproxy-{v}-linux-x64.tar.xz" ;;
  aarch64-unknown-linux-*) pattern="naiveproxy-{v}-linux-arm64.tar.xz" ;;
  armv7-unknown-linux-*) pattern="naiveproxy-{v}-linux-arm.tar.xz" ;;
  x86_64-pc-windows-*) pattern="naiveproxy-{v}-win-x64.zip" ;;
  aarch64-pc-windows-*) pattern="naiveproxy-{v}-win-arm64.zip" ;;
  x86_64-apple-darwin) pattern="naiveproxy-{v}-mac-x64-x64.tar.xz" ;;
  aarch64-apple-darwin) pattern="naiveproxy-{v}-mac-arm64-arm64.tar.xz" ;;
  aarch64-linux-android) pattern="naiveproxy-plugin-{v}-arm64-v8a.apk" ;;
  *) echo "fetch-naive.sh: no naive build for $target" >&2; exit 1 ;;
esac

api=(curl -fsSL -H "Accept: application/vnd.github+json")
[[ -n "${GITHUB_TOKEN:-}" ]] && api+=(-H "Authorization: Bearer $GITHUB_TOKEN")
release_url="https://api.github.com/repos/klzgrad/naiveproxy/releases/latest"
[[ -n "$NAIVE_VERSION" ]] && release_url="https://api.github.com/repos/klzgrad/naiveproxy/releases/tags/$NAIVE_VERSION"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
"${api[@]}" "$release_url" >"$work/release.json"

read -r version asset url digest < <(python3 - "$work/release.json" "$pattern" <<'PY'
import json, sys
release, pattern = json.load(open(sys.argv[1])), sys.argv[2]
version = release["tag_name"]
name = pattern.replace("{v}", version)
for asset in release["assets"]:
    if asset["name"] == name:
        print(version, name, asset["browser_download_url"], (asset.get("digest") or "-").removeprefix("sha256:"))
        break
else:
    names = "\n  ".join(sorted(asset["name"] for asset in release["assets"]))
    sys.exit(f"fetch-naive.sh: {version} has no {name}; its assets:\n  {names}")
PY
)
[[ -n "${url:-}" ]] || exit 1
# Python on Windows ends its lines with CRLF.
digest="${digest%$'\r'}"

want="$(pinned_sha256 "$asset")"
pinned=true
if [[ -z "$want" ]]; then
  pinned=false
  want="$digest"
fi
if [[ -z "$want" || "$want" == "-" ]]; then
  echo "fetch-naive.sh: no digest to check $asset against; pin one in pinned_sha256" >&2
  exit 1
fi

echo "fetch-naive.sh: downloading $url"
curl -fsSL -o "$work/$asset" "$url"
got="$( (sha256sum "$work/$asset" 2>/dev/null || shasum -a 256 "$work/$asset") | cut -d' ' -f1)"
if [[ "$got" != "$want" ]]; then
  echo "fetch-naive.sh: $asset has SHA-256 $got, expected $want" >&2
  exit 1
fi
if [[ "$pinned" == false ]]; then
  echo "fetch-naive.sh: $asset ($version) matches GitHub's digest; to pin: NAIVE_VERSION=$version, $asset) echo $got"
fi

mkdir -p "$work/x"
case "$asset" in
  *.zip)
    (cd "$work/x" && unzip -q "../$asset")
    binary="$(find "$work/x" -name naive.exe -type f | head -1)" ;;
  *.apk)
    (cd "$work/x" && unzip -q "../$asset" lib/arm64-v8a/libnaive.so)
    binary="$work/x/lib/arm64-v8a/libnaive.so" ;;
  *)
    tar -xJf "$work/$asset" -C "$work/x"
    binary="$(find "$work/x" -name naive -type f | head -1)" ;;
esac
[[ -n "$binary" && -f "$binary" ]] || { echo "fetch-naive.sh: no naive binary in $asset" >&2; exit 1; }
mkdir -p "$(dirname "$out")"
install -m 755 "$binary" "$out"
echo "fetch-naive.sh: installed $out"

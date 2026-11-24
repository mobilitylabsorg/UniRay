#!/usr/bin/env bash
# Builds uniray-cp (the Check Point corporate VPN client) for a Rust target
# triple:
#
#   scripts/build-cp.sh <target triple> <output file>
#
# The engine runs it beside the main VPN for a Check Point corporate network
# (engine/ovpn.go drives it over the same packet socket and settings protocol
# as uniray-ovpn). It is snx-rs (github.com/ancwrd1/snx-rs, AGPL-3.0) with the
# isakmp crate, both fetched at a pinned commit into build/cache/cp-deps and
# patched (cp/snxcore.patch, cp/isakmp.patch): the patch adds an engine mode in
# which the tunnel's IP packets are exchanged over a socketpair with the engine
# instead of a TUN device, and the settings (address, routes, DNS, domains) are
# printed for the engine to parse; host routing and DNS are left to the engine.
#
# Linux: a static musl binary built in Docker on Alpine, OpenSSL and SQLite
# vendored, so it runs on any distribution, as uniray-ovpn does. Windows
# (x86_64): cross-built with MinGW-w64, OpenSSL vendored and linked statically,
# so the .exe imports only system DLLs. The crates are fetched first; the
# build itself runs with no network.
set -euo pipefail

SNX_REPO="https://github.com/ancwrd1/snx-rs.git"
SNX_TAG="v6.7.0"
SNX_COMMIT="e127ccf724cbe81125ed8865277cc2ebfbae51bd"
ISAKMP_REPO="https://github.com/ancwrd1/isakmp.git"
ISAKMP_COMMIT="a287d15893fe7c50875de73eb0a891a1a2f2f52c"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target="$1" out="$2"
mkdir -p "$(dirname "$out")"
out="$(cd "$(dirname "$out")" && pwd)/$(basename "$out")"

deps="$root/build/cache/cp-deps"
snx="$deps/snx-rs"
isakmp="$deps/isakmp"

fetch() { # repo commit dir [tag]
  local repo="$1" commit="$2" dir="$3" tag="${4:-}"
  if [[ "$(git -C "$dir" rev-parse HEAD 2>/dev/null)" == "$commit" ]]; then return; fi
  rm -rf "$dir"
  if [[ -n "$tag" ]]; then
    git clone --quiet --depth 1 --branch "$tag" "$repo" "$dir"
  else
    git clone --quiet "$repo" "$dir" && git -C "$dir" checkout --quiet "$commit"
  fi
  local got
  got="$(git -C "$dir" rev-parse HEAD)"
  if [[ "$got" != "$commit" ]]; then
    echo "build-cp.sh: $repo is $got, not the pinned $commit" >&2
    exit 1
  fi
}

prepare() {
  fetch "$SNX_REPO" "$SNX_COMMIT" "$snx" "$SNX_TAG"
  fetch "$ISAKMP_REPO" "$ISAKMP_COMMIT" "$isakmp"
  # Reset to the pinned tree and (re)apply the UniRay patches.
  git -C "$snx" checkout --quiet -- . && git -C "$snx" clean -qfd
  git -C "$isakmp" checkout --quiet -- . && git -C "$isakmp" clean -qfd
  git -C "$snx" apply "$root/cp/snxcore.patch"
  git -C "$isakmp" apply "$root/cp/isakmp.patch"
  # snxcore pulls isakmp from git; use our patched local copy instead.
  if ! grep -q 'patch."https://github.com/ancwrd1/isakmp.git"' "$snx/Cargo.toml"; then
    cat >> "$snx/Cargo.toml" <<EOF

[patch."https://github.com/ancwrd1/isakmp.git"]
isakmp = { path = "../isakmp" }
EOF
  fi
}

cargo_home="$deps/cargo"

# build <image> <dockerfile> <platform> <rust target> [env...]: cargo fetch,
# then the build with no network; the binary to $out.
build() {
  local image="$1" dockerfile="$2" platform="$3" rust_target="$4"
  shift 4
  docker build --quiet --platform "$platform" -t "$image" - >/dev/null <<<"$dockerfile"
  # One target directory per image: build scripts run on its platform.
  local target_dir="$deps/target-$image"
  mkdir -p "$cargo_home" "$target_dir"
  local run=(docker run --rm --platform "$platform" --user "$(id -u):$(id -g)"
    -e CARGO_HOME=/cargo -e CARGO_TARGET_DIR=/target "$@"
    -v "$cargo_home:/cargo" -v "$target_dir:/target" -v "$snx:/deps/snx-rs" -v "$isakmp:/deps/isakmp:ro"
    -w /deps/snx-rs)
  local log="$deps/build.log"
  "${run[@]}" "$image" cargo fetch >"$log" 2>&1 || { tail -30 "$log"; exit 1; }
  "${run[@]}" --network none "$image" \
    cargo build --offline --release --target "$rust_target" -p snx-rs \
      --features snxcore/vendored-openssl,snxcore/vendored-sqlite >"$log" 2>&1 \
    || { tail -30 "$log"; exit 1; }
  local exe=""
  [[ "$rust_target" == *windows* ]] && exe=".exe"
  install -m 755 "$target_dir/$rust_target/release/snx-rs$exe" "$out"
}

linux() { # docker platform, rust target
  build "uniray-cp-alpine-${1//\//-}" 'FROM rust:1-alpine3.22
RUN apk add --no-cache musl-dev perl make linux-headers' "$1" "$2" \
    -e RUSTFLAGS="-C target-feature=+crt-static -C strip=symbols"
}

windows() {
  build uniray-cp-mingw 'FROM rust:1-trixie
RUN apt-get update && apt-get install -y --no-install-recommends g++-mingw-w64-x86-64-posix gcc-mingw-w64-x86-64-posix perl && rm -rf /var/lib/apt/lists/*
RUN rustup target add x86_64-pc-windows-gnu' linux/amd64 x86_64-pc-windows-gnu \
    -e CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc \
    -e CXX_x86_64_pc_windows_gnu=x86_64-w64-mingw32-g++ \
    -e AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar \
    -e CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
    -e RUSTFLAGS="-C strip=symbols"
}

prepare
case "$target" in
  x86_64-unknown-linux-*) linux linux/amd64 x86_64-unknown-linux-musl ;;
  aarch64-unknown-linux-*) linux linux/arm64 aarch64-unknown-linux-musl ;;
  x86_64-pc-windows-*) windows ;;
  *) echo "build-cp.sh: unsupported target $target" >&2; exit 1 ;;
esac
echo "build-cp.sh: built $out"

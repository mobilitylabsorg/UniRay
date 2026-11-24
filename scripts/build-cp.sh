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
# Linux: built in Docker on Debian (OpenSSL and SQLite from the distro). Windows
# (x86_64): cross-built with MinGW-w64, OpenSSL vendored and linked statically,
# so the .exe imports only system DLLs.
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

linux() {
  local image="uniray-cp-build"
  docker build --quiet -t "$image" - >/dev/null <<'EOF'
FROM rust:1-trixie
RUN apt-get update && apt-get install -y --no-install-recommends libssl-dev libsqlite3-dev pkg-config && rm -rf /var/lib/apt/lists/*
EOF
  local work
  work="$(mktemp -d)"
  docker run --rm --user "$(id -u):$(id -g)" \
    -e CARGO_HOME=/work/cargo -e CARGO_TARGET_DIR=/work/target \
    -v "$deps:/deps" -v "$work:/work" -w /deps/snx-rs "$image" \
    cargo build --release -p snx-rs >"$work/build.log" 2>&1 \
    || { tail -30 "$work/build.log"; rm -rf "$work"; exit 1; }
  cp "$work/target/release/snx-rs" "$out"
  rm -rf "$work"
}

windows() {
  local image="uniray-cp-mingw"
  docker build --quiet -t "$image" - >/dev/null <<'EOF'
FROM rust:1-trixie
RUN apt-get update && apt-get install -y --no-install-recommends g++-mingw-w64-x86-64-posix gcc-mingw-w64-x86-64-posix perl && rm -rf /var/lib/apt/lists/*
RUN rustup target add x86_64-pc-windows-gnu
EOF
  local work
  work="$(mktemp -d)"
  docker run --rm --user "$(id -u):$(id -g)" \
    -e CARGO_HOME=/work/cargo -e CARGO_TARGET_DIR=/work/target \
    -e CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc \
    -e CXX_x86_64_pc_windows_gnu=x86_64-w64-mingw32-g++ \
    -e AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar \
    -e CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
    -v "$deps:/deps" -v "$work:/work" -w /deps/snx-rs "$image" \
    cargo build --release --target x86_64-pc-windows-gnu -p snx-rs >"$work/build.log" 2>&1 \
    || { tail -30 "$work/build.log"; rm -rf "$work"; exit 1; }
  cp "$work/target/x86_64-pc-windows-gnu/release/snx-rs.exe" "$out"
  rm -rf "$work"
}

prepare
case "$target" in
  *-linux-gnu | *-linux-musl) linux ;;
  x86_64-pc-windows-*) windows ;;
  *) echo "build-cp.sh: unsupported target $target" >&2; exit 1 ;;
esac
echo "build-cp.sh: built $out"

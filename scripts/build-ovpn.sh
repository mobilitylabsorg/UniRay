#!/usr/bin/env bash
# Builds uniray-ovpn (ovpn/, the OpenVPN 3 core without a TUN device) for a
# Rust target triple:
#
#   scripts/build-ovpn.sh <target triple> <output file>
#
# The engine runs it for the corporate network (engine/ovpn.go). The core is
# fetched at a pinned commit into build/cache/ovpn-deps/openvpn3.
#
# Linux: a static binary (musl, OpenSSL, LZ4), built in Docker on Alpine, so it
# runs on any distribution and in the gateway container. Other architectures
# than the host's need binfmt (qemu-user-static) for Docker's --platform.
# macOS: on a Mac, with Homebrew's asio, lz4 and openssl@3 linked statically.
set -euo pipefail

OPENVPN3_REPO="https://github.com/OpenVPN/openvpn3.git"
OPENVPN3_TAG="release/3.9"
OPENVPN3_COMMIT="10c6a56cb17fd015a2d24953c3c42145bbcdea58"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target="$1" out="$2"
mkdir -p "$(dirname "$out")"
out="$(cd "$(dirname "$out")" && pwd)/$(basename "$out")"

core="$root/build/cache/ovpn-deps/openvpn3"
if [[ "$(git -C "$core" rev-parse HEAD 2>/dev/null)" != "$OPENVPN3_COMMIT" ]]; then
  rm -rf "$core"
  git clone --quiet --depth 1 --branch "$OPENVPN3_TAG" "$OPENVPN3_REPO" "$core"
  got="$(git -C "$core" rev-parse HEAD)"
  if [[ "$got" != "$OPENVPN3_COMMIT" ]]; then
    echo "build-ovpn.sh: $OPENVPN3_TAG is $got, not the pinned $OPENVPN3_COMMIT" >&2
    exit 1
  fi
fi

linux() {
  local platform="$1"
  local image="uniray-ovpn-alpine-${platform//\//-}"
  docker build --quiet --platform "$platform" -t "$image" - >/dev/null <<'EOF'
FROM alpine:3.22
RUN apk add --no-cache g++ make cmake asio-dev lz4-dev lz4-static openssl-dev openssl-libs-static linux-headers
EOF
  local work
  work="$(mktemp -d)"
  # The core's code runs only here, with no network.
  docker run --rm --network none --platform "$platform" --user "$(id -u):$(id -g)" \
    -v "$root/ovpn:/src:ro" -v "$core:/core:ro" -v "$work:/out" "$image" sh -ec '
      cmake -S /src -B /out/build -DOPENVPN3_DIR=/core -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_EXE_LINKER_FLAGS="-static -s" -DOPENSSL_USE_STATIC_LIBS=TRUE \
        -DLZ4_LIBRARY=/usr/lib/liblz4.a >/out/cmake.log 2>&1 || { tail -20 /out/cmake.log; exit 1; }
      cmake --build /out/build -j"$(nproc)" >/out/build.log 2>&1 || { grep -B2 -A8 "error" /out/build.log | tail -40; exit 1; }
      cp /out/build/uniray-ovpn /out/uniray-ovpn'
  install -m 755 "$work/uniray-ovpn" "$out"
  rm -rf "$work"
}

case "$target" in
  x86_64-unknown-linux-*) linux linux/amd64 ;;
  aarch64-unknown-linux-*) linux linux/arm64 ;;
  armv7-unknown-linux-*) linux linux/arm/v7 ;;
  *-apple-darwin)
    brew="$(brew --prefix)"
    arch="${target%%-*}"
    [[ "$arch" == aarch64 ]] && arch=arm64
    work="$(mktemp -d)"
    cmake -S "$root/ovpn" -B "$work" -DOPENVPN3_DIR="$core" -DCMAKE_BUILD_TYPE=Release \
      -DCMAKE_OSX_ARCHITECTURES="$arch" -DCMAKE_PREFIX_PATH="$brew/opt/asio;$brew/opt/lz4;$brew/opt/openssl@3" \
      -DOPENSSL_USE_STATIC_LIBS=TRUE -DLZ4_LIBRARY="$brew/opt/lz4/lib/liblz4.a" >/dev/null
    cmake --build "$work" -j"$(sysctl -n hw.ncpu)" >/dev/null
    install -m 755 "$work/uniray-ovpn" "$out"
    rm -rf "$work"
    ;;
  *) echo "build-ovpn.sh: no uniray-ovpn build for $target yet" >&2; exit 1 ;;
esac

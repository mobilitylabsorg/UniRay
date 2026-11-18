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
# Windows (x86_64): cross-built with MinGW-w64 in Docker on Debian, OpenSSL
# and LZ4 from their release tarballs (pinned below, kept in
# build/cache/ovpn-deps/dl), all linked statically: one .exe, no DLLs.
# macOS: on a Mac, with Homebrew's asio, lz4 and openssl@3 linked statically.
set -euo pipefail

OPENVPN3_REPO="https://github.com/OpenVPN/openvpn3.git"
OPENVPN3_TAG="release/3.9"
OPENVPN3_COMMIT="10c6a56cb17fd015a2d24953c3c42145bbcdea58"

# Windows' dependencies, built from source.
OPENSSL_VERSION="3.5.9"
OPENSSL_SHA256="603f5602e2eef00d77fbd429d34dcd5822bb301757a1bc9cdb24c670f1eb859a"
LZ4_VERSION="1.10.0"
LZ4_SHA256="537512904744b35e232912055ccf8ec66d768639ff3abe5788d90d792ec5f48b"

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

# fetch <url> <sha256>: into build/cache/ovpn-deps/dl, checked.
fetch() {
  local dl="$root/build/cache/ovpn-deps/dl" file
  file="$dl/$(basename "$1")"
  mkdir -p "$dl"
  if [[ ! -f "$file" ]] || ! echo "$2  $file" | sha256sum --check --status; then
    curl -sSfL -o "$file.part" "$1"
    if ! echo "$2  $file.part" | sha256sum --check --status; then
      echo "build-ovpn.sh: $1 does not match its checksum" >&2
      exit 1
    fi
    mv "$file.part" "$file"
  fi
}

windows() {
  fetch "https://github.com/openssl/openssl/releases/download/openssl-$OPENSSL_VERSION/openssl-$OPENSSL_VERSION.tar.gz" "$OPENSSL_SHA256"
  fetch "https://github.com/lz4/lz4/releases/download/v$LZ4_VERSION/lz4-$LZ4_VERSION.tar.gz" "$LZ4_SHA256"
  local image="uniray-ovpn-mingw"
  # posix threads: the core uses std::thread and std::mutex.
  docker build --quiet -t "$image" - >/dev/null <<'EOF'
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends g++-mingw-w64-x86-64-posix \
      gcc-mingw-w64-x86-64-posix cmake make perl libasio-dev && rm -rf /var/lib/apt/lists/*
EOF
  # OpenSSL and LZ4 once per version; uniray-ovpn each time.
  local deps="$root/build/cache/ovpn-deps/mingw64-openssl-$OPENSSL_VERSION-lz4-$LZ4_VERSION"
  mkdir -p "$deps"
  local work
  work="$(mktemp -d)"
  docker run --rm --network none --user "$(id -u):$(id -g)" \
    -v "$root/ovpn:/src:ro" -v "$core:/core:ro" -v "$root/build/cache/ovpn-deps/dl:/dl:ro" \
    -v "$deps:/deps" -v "$work:/out" \
    -e OPENSSL_VERSION="$OPENSSL_VERSION" -e LZ4_VERSION="$LZ4_VERSION" "$image" sh -ec '
      export HOME=/out
      cross=x86_64-w64-mingw32
      if [ ! -f /deps/lib/libssl.a ]; then
        tar -xzf /dl/openssl-$OPENSSL_VERSION.tar.gz -C /out
        cd /out/openssl-$OPENSSL_VERSION
        ./Configure mingw64 no-shared no-tests no-docs no-apps no-module --cross-compile-prefix=$cross- \
          --prefix=/deps --libdir=lib >/out/openssl.log 2>&1 || { tail -20 /out/openssl.log; exit 1; }
        make -j"$(nproc)" build_libs >>/out/openssl.log 2>&1 || { tail -30 /out/openssl.log; exit 1; }
        make install_dev >>/out/openssl.log 2>&1 || { tail -20 /out/openssl.log; exit 1; }
      fi
      if [ ! -f /deps/lib/liblz4.a ]; then
        tar -xzf /dl/lz4-$LZ4_VERSION.tar.gz -C /out
        lz4=/out/lz4-$LZ4_VERSION/lib
        make -C $lz4 liblz4.a CC=$cross-gcc-posix AR=$cross-ar >/out/lz4.log 2>&1 || { tail -20 /out/lz4.log; exit 1; }
        mkdir -p /deps/lib /deps/include
        cp $lz4/liblz4.a /deps/lib/
        cp $lz4/lz4.h $lz4/lz4hc.h $lz4/lz4frame.h /deps/include/
      fi
      # asio is headers only; the copy keeps the build off the host'"'"'s /usr/include.
      mkdir -p /out/asio && cp -r /usr/include/asio /usr/include/asio.hpp /out/asio/
      cmake -S /src -B /out/build -DOPENVPN3_DIR=/core -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_SYSTEM_NAME=Windows -DCMAKE_C_COMPILER=$cross-gcc-posix -DCMAKE_CXX_COMPILER=$cross-g++-posix \
        -DCMAKE_RC_COMPILER=$cross-windres -DCMAKE_FIND_ROOT_PATH=/deps \
        -DCMAKE_FIND_ROOT_PATH_MODE_LIBRARY=ONLY -DCMAKE_FIND_ROOT_PATH_MODE_INCLUDE=ONLY \
        -DCMAKE_FIND_ROOT_PATH_MODE_PACKAGE=ONLY -DCMAKE_FIND_ROOT_PATH_MODE_PROGRAM=NEVER \
        -DCMAKE_EXE_LINKER_FLAGS="-static -s" -DOPENSSL_ROOT_DIR=/deps -DOPENSSL_USE_STATIC_LIBS=TRUE \
        -DASIO_INCLUDE_DIR=/out/asio -DLZ4_INCLUDE_DIR=/deps/include -DLZ4_LIBRARY=/deps/lib/liblz4.a \
        >/out/cmake.log 2>&1 || { tail -30 /out/cmake.log; exit 1; }
      cmake --build /out/build -j"$(nproc)" >/out/build.log 2>&1 || { grep -B2 -A8 "error" /out/build.log | head -60; exit 1; }
      cp /out/build/uniray-ovpn.exe /out/uniray-ovpn.exe'
  install -m 755 "$work/uniray-ovpn.exe" "$out"
  rm -rf "$work"
}

case "$target" in
  x86_64-pc-windows-*) windows ;;
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

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
# Android (aarch64, armv7): cross-built with the NDK (NDK_HOME, mounted into
# Docker on Debian), OpenSSL and LZ4 as for Windows, libc++ linked statically.
# The result is an executable that the APK carries as a native library
# (libuniray_ovpn.so), the only place an app may run programs from.
# macOS (arm64, x86_64): on a Mac, OpenSSL and LZ4 as for Windows, built for
# the target's architecture; asio's headers from Homebrew.
set -euo pipefail

OPENVPN3_REPO="https://github.com/OpenVPN/openvpn3.git"
OPENVPN3_TAG="release/3.9"
OPENVPN3_COMMIT="10c6a56cb17fd015a2d24953c3c42145bbcdea58"

# The dependencies built from source (Windows, Android, macOS).
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

# sha256_is <sha256> <file>: GNU sha256sum, or macOS's shasum.
sha256_is() {
  [[ "$( (sha256sum "$2" 2>/dev/null || shasum -a 256 "$2") | cut -d' ' -f1)" == "$1" ]]
}

# fetch <url> <sha256>: into build/cache/ovpn-deps/dl, checked.
fetch() {
  local dl="$root/build/cache/ovpn-deps/dl" file
  file="$dl/$(basename "$1")"
  mkdir -p "$dl"
  if [[ ! -f "$file" ]] || ! sha256_is "$2" "$file"; then
    curl -sSfL -o "$file.part" "$1"
    if ! sha256_is "$2" "$file.part"; then
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

# android <abi> <OpenSSL target> <clang prefix>: with the NDK in $NDK_HOME.
android() {
  local abi="$1" openssl_target="$2" clang="$3" api=24
  local ndk="${NDK_HOME:-${ANDROID_NDK_HOME:-}}"
  if [[ -z "$ndk" || ! -d "$ndk/toolchains/llvm" ]]; then
    echo "build-ovpn.sh: set NDK_HOME to the Android NDK" >&2
    exit 1
  fi
  fetch "https://github.com/openssl/openssl/releases/download/openssl-$OPENSSL_VERSION/openssl-$OPENSSL_VERSION.tar.gz" "$OPENSSL_SHA256"
  fetch "https://github.com/lz4/lz4/releases/download/v$LZ4_VERSION/lz4-$LZ4_VERSION.tar.gz" "$LZ4_SHA256"
  local image="uniray-ovpn-android"
  docker build --quiet -t "$image" - >/dev/null <<'DOCKERFILE'
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends cmake make perl libasio-dev && rm -rf /var/lib/apt/lists/*
DOCKERFILE
  # OpenSSL and LZ4 once per ABI and version; uniray-ovpn each time.
  local deps="$root/build/cache/ovpn-deps/android-$abi-$api-openssl-$OPENSSL_VERSION-lz4-$LZ4_VERSION"
  mkdir -p "$deps"
  local work
  work="$(mktemp -d)"
  docker run --rm --network none --user "$(id -u):$(id -g)" \
    -v "$root/ovpn:/src:ro" -v "$core:/core:ro" -v "$root/build/cache/ovpn-deps/dl:/dl:ro" -v "$ndk:/ndk:ro" \
    -v "$deps:/deps" -v "$work:/out" \
    -e OPENSSL_VERSION="$OPENSSL_VERSION" -e LZ4_VERSION="$LZ4_VERSION" \
    -e ABI="$abi" -e OPENSSL_TARGET="$openssl_target" -e CLANG="$clang$api-clang" -e API="$api" "$image" sh -ec '
      export HOME=/out ANDROID_NDK_ROOT=/ndk PATH=/ndk/toolchains/llvm/prebuilt/linux-x86_64/bin:$PATH
      if [ ! -f /deps/lib/libssl.a ]; then
        tar -xzf /dl/openssl-$OPENSSL_VERSION.tar.gz -C /out
        cd /out/openssl-$OPENSSL_VERSION
        ./Configure $OPENSSL_TARGET -D__ANDROID_API__=$API no-shared no-tests no-docs no-apps no-module \
          --prefix=/deps --libdir=lib >/out/openssl.log 2>&1 || { tail -20 /out/openssl.log; exit 1; }
        make -j"$(nproc)" build_libs >>/out/openssl.log 2>&1 || { tail -30 /out/openssl.log; exit 1; }
        make install_dev >>/out/openssl.log 2>&1 || { tail -20 /out/openssl.log; exit 1; }
      fi
      if [ ! -f /deps/lib/liblz4.a ]; then
        tar -xzf /dl/lz4-$LZ4_VERSION.tar.gz -C /out
        lz4=/out/lz4-$LZ4_VERSION/lib
        make -C $lz4 liblz4.a CC=$CLANG AR=llvm-ar >/out/lz4.log 2>&1 || { tail -20 /out/lz4.log; exit 1; }
        mkdir -p /deps/lib /deps/include
        cp $lz4/liblz4.a /deps/lib/
        cp $lz4/lz4.h $lz4/lz4hc.h $lz4/lz4frame.h /deps/include/
      fi
      # Bionic has pthreads in its libc; the core still links -lpthread.
      [ -f /deps/lib/libpthread.a ] || llvm-ar rc /deps/lib/libpthread.a
      mkdir -p /out/asio && cp -r /usr/include/asio /usr/include/asio.hpp /out/asio/
      cmake -S /src -B /out/build -DOPENVPN3_DIR=/core -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_TOOLCHAIN_FILE=/ndk/build/cmake/android.toolchain.cmake -DANDROID_ABI=$ABI \
        -DANDROID_PLATFORM=$API -DANDROID_STL=c++_static -DCMAKE_FIND_ROOT_PATH=/deps \
        -DCMAKE_EXE_LINKER_FLAGS="-s -L/deps/lib" -DOPENSSL_ROOT_DIR=/deps -DOPENSSL_USE_STATIC_LIBS=TRUE \
        -DASIO_INCLUDE_DIR=/out/asio -DLZ4_INCLUDE_DIR=/deps/include -DLZ4_LIBRARY=/deps/lib/liblz4.a \
        >/out/cmake.log 2>&1 || { tail -30 /out/cmake.log; exit 1; }
      cmake --build /out/build -j"$(nproc)" >/out/build.log 2>&1 || { grep -B2 -A8 "error" /out/build.log | head -60; exit 1; }
      cp /out/build/uniray-ovpn /out/uniray-ovpn'
  install -m 755 "$work/uniray-ovpn" "$out"
  rm -rf "$work"
}

# darwin <arch> <OpenSSL target> <deployment target>: on a Mac. OpenSSL and LZ4
# from the tarballs for the target's architecture (the runner's Homebrew has
# its own only), asio's headers from Homebrew (brew install asio).
darwin() {
  local arch="$1" openssl_target="$2"
  export MACOSX_DEPLOYMENT_TARGET="$3"
  fetch "https://github.com/openssl/openssl/releases/download/openssl-$OPENSSL_VERSION/openssl-$OPENSSL_VERSION.tar.gz" "$OPENSSL_SHA256"
  fetch "https://github.com/lz4/lz4/releases/download/v$LZ4_VERSION/lz4-$LZ4_VERSION.tar.gz" "$LZ4_SHA256"
  local dl="$root/build/cache/ovpn-deps/dl"
  local deps="$root/build/cache/ovpn-deps/darwin-$arch-openssl-$OPENSSL_VERSION-lz4-$LZ4_VERSION"
  local work
  work="$(mktemp -d)"
  mkdir -p "$deps/lib" "$deps/include"
  if [[ ! -f "$deps/lib/libssl.a" ]]; then
    tar -xzf "$dl/openssl-$OPENSSL_VERSION.tar.gz" -C "$work"
    (cd "$work/openssl-$OPENSSL_VERSION" &&
      ./Configure "$openssl_target" no-shared no-tests no-docs no-apps no-module \
        --prefix="$deps" --libdir=lib >"$work/openssl.log" 2>&1 &&
      make -j"$(sysctl -n hw.ncpu)" build_libs >>"$work/openssl.log" 2>&1 &&
      make install_dev >>"$work/openssl.log" 2>&1) || { tail -30 "$work/openssl.log"; exit 1; }
  fi
  if [[ ! -f "$deps/lib/liblz4.a" ]]; then
    tar -xzf "$dl/lz4-$LZ4_VERSION.tar.gz" -C "$work"
    local lz4="$work/lz4-$LZ4_VERSION/lib"
    make -C "$lz4" liblz4.a CFLAGS="-O2 -arch $arch" >"$work/lz4.log" 2>&1 || { tail -20 "$work/lz4.log"; exit 1; }
    cp "$lz4/liblz4.a" "$deps/lib/"
    cp "$lz4/lz4.h" "$lz4/lz4hc.h" "$lz4/lz4frame.h" "$deps/include/"
  fi
  local asio
  asio="$(brew --prefix asio)/include"
  cmake -S "$root/ovpn" -B "$work/build" -DOPENVPN3_DIR="$core" -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_OSX_ARCHITECTURES="$arch" -DOPENSSL_ROOT_DIR="$deps" -DOPENSSL_USE_STATIC_LIBS=TRUE \
    -DASIO_INCLUDE_DIR="$asio" -DLZ4_INCLUDE_DIR="$deps/include" -DLZ4_LIBRARY="$deps/lib/liblz4.a" \
    >"$work/cmake.log" 2>&1 || { tail -30 "$work/cmake.log"; exit 1; }
  cmake --build "$work/build" -j"$(sysctl -n hw.ncpu)" >"$work/build.log" 2>&1 \
    || { grep -B2 -A8 "error" "$work/build.log" | head -60; exit 1; }
  strip -x "$work/build/uniray-ovpn"
  install -m 755 "$work/build/uniray-ovpn" "$out"
  rm -rf "$work"
}

case "$target" in
  aarch64-linux-android) android arm64-v8a android-arm64 aarch64-linux-android ;;
  armv7-linux-androideabi) android armeabi-v7a android-arm armv7a-linux-androideabi ;;
  x86_64-pc-windows-*) windows ;;
  x86_64-unknown-linux-*) linux linux/amd64 ;;
  aarch64-unknown-linux-*) linux linux/arm64 ;;
  armv7-unknown-linux-*) linux linux/arm/v7 ;;
  aarch64-apple-darwin) darwin arm64 darwin64-arm64-cc 11.0 ;;
  x86_64-apple-darwin) darwin x86_64 darwin64-x86_64-cc 10.15 ;;
  *) echo "build-ovpn.sh: no uniray-ovpn build for $target yet" >&2; exit 1 ;;
esac

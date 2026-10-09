#!/usr/bin/env bash
# Builds the uniray-gateway container image for MikroTik RouterOS.
#
# Usage: scripts/build-gateway.sh [platform...]       (default: linux/arm/v7)
#   platforms: linux/arm/v7, linux/arm64, linux/amd64
#
# Output: build/uniray-gateway-<arch>.tar and build/uniray-gateway-<arch>-lowmem.tar
# (see below), docker archives that RouterOS imports with
# `/container add file=uniray-gateway-<arch>.tar ...`.
#
# The image holds the gateway, Xray, and for the servers Xray cannot run
# uniray-engine (WireGuard, AmneziaWG; Go, static) and NaiveProxy's naive.
# naive is built against glibc, so the image gets the few glibc libraries it
# needs, taken from Debian's image of the same platform. NAIVE=0 leaves naive
# and glibc out (a smaller image without Naive servers).
#
# Needs docker (with buildx), Go 1.25+ (the engine), cargo-zigbuild
# (`cargo install cargo-zigbuild`), Zig (`pip install ziglang` or a system
# package), readelf (binutils) and the Rust targets
# (`rustup target add armv7-unknown-linux-musleabihf aarch64-unknown-linux-musl`).
# Zig cross-compiles the C in the dependencies (ring) without a container.
set -euo pipefail

XRAY_VERSION="v26.9.30"
# Where naive's glibc libraries come from.
GLIBC_IMAGE="debian:bookworm-slim"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

# naive and the glibc libraries it needs (and nothing more) into the rootfs.
add_naive() {
  local platform=$1 target=$2 arch=$3 libdir=$4 interp=$5 rootfs=$6
  local naive="$root/build/cache/naive-$arch"
  # fetch-naive.sh checks the archive against its pinned digest.
  [[ -f "$naive" ]] || "$root/scripts/fetch-naive.sh" "$target" "$naive"
  install -m 755 "$naive" "$rootfs/usr/bin/naive"

  local want=(libc.so.6 libm.so.6 libdl.so.2 libpthread.so.0 libgcc_s.so.1 "$(basename "$interp")")
  local needed
  needed="$(readelf -d "$naive" | sed -n 's/.*Shared library: \[\(.*\)\]/\1/p')"
  for lib in $needed; do
    if [[ " ${want[*]} " != *" $lib "* ]]; then
      echo "build-gateway: naive needs $lib, which the image does not get; add it to add_naive" >&2
      exit 1
    fi
  done
  if [[ "$(readelf -l "$naive" | sed -n 's/.*interpreter: \(.*\)\]/\1/p')" != "$interp" ]]; then
    echo "build-gateway: naive's program interpreter is not $interp" >&2
    exit 1
  fi

  local container
  container="$(docker create --platform "$platform" "$GLIBC_IMAGE")"
  mkdir -p "$rootfs/lib" "$rootfs$(dirname "$interp")"
  for lib in "${want[@]}"; do
    docker cp -q -L "$container:/usr/lib/$libdir/$lib" "$rootfs/lib/$lib"
  done
  [[ "$interp" == "/lib/"* ]] || mv "$rootfs/lib/$(basename "$interp")" "$rootfs$interp"
  docker rm "$container" >/dev/null
}

platforms=("$@")
[[ ${#platforms[@]} -eq 0 ]] && platforms=(linux/arm/v7)

for platform in "${platforms[@]}"; do
  case "$platform" in
    linux/arm/v7) arch=armv7; target=armv7-unknown-linux-musleabihf; asset=linux-arm32-v7a; sha256=0b9719471c7c69752857714e9711d4da57cf38a6beb75dcddfb21425f7919908
                  libdir=arm-linux-gnueabihf; interp=/lib/ld-linux-armhf.so.3 ;;
    linux/arm64)  arch=arm64; target=aarch64-unknown-linux-musl;     asset=linux-arm64-v8a; sha256=9886f077f9fd8e6713b84c377c1c7db4e53b9bfa8c276a5bd12561139522b473
                  libdir=aarch64-linux-gnu; interp=/lib/ld-linux-aarch64.so.1 ;;
    linux/amd64)  arch=amd64; target=x86_64-unknown-linux-musl;      asset=linux-64;        sha256=f851110beaff16e78d643f0ccfd9524b4a44dfd59bae3e34bb52bba378f7690e
                  libdir=x86_64-linux-gnu; interp=/lib64/ld-linux-x86-64.so.2 ;;
    *) echo "build-gateway: unsupported platform $platform" >&2; exit 1 ;;
  esac
  echo "build-gateway: $platform"

  cargo zigbuild --release -p uniray-gateway --target "$target"

  cache="$root/build/cache"
  zip="$cache/Xray-$asset-$XRAY_VERSION.zip"
  mkdir -p "$cache"
  if [[ ! -f "$zip" ]]; then
    curl -fL --retry 3 -o "$zip.part" "https://github.com/XTLS/Xray-core/releases/download/${XRAY_VERSION}/Xray-${asset}.zip"
    mv "$zip.part" "$zip"
  fi
  actual="$(sha256sum "$zip" | cut -d' ' -f1)"
  if [[ "$actual" != "$sha256" ]]; then
    echo "build-gateway: checksum mismatch for Xray-${asset}.zip (expected $sha256, got $actual)" >&2
    rm -f "$zip"
    exit 1
  fi

  rootfs="$root/build/gateway-rootfs/$arch"
  rm -rf "$rootfs"
  mkdir -p "$rootfs/usr/bin" "$rootfs/etc/ssl/certs" "$rootfs/data" "$rootfs/tmp"
  chmod 1777 "$rootfs/tmp"
  install -m 755 "$root/target/$target/release/uniray-gateway" "$rootfs/usr/bin/uniray-gateway"
  # Only the core: without split routing the geo databases are not needed.
  unzip -p "$zip" xray > "$rootfs/usr/bin/xray"
  chmod 755 "$rootfs/usr/bin/xray"
  # Xray verifies TLS (DoH, servers with security "tls") against these roots;
  # it has none built in.
  install -m 644 /etc/ssl/certs/ca-certificates.crt "$rootfs/etc/ssl/certs/ca-certificates.crt"

  if [[ "${NAIVE:-1}" != 0 ]]; then
    add_naive "$platform" "$target" "$arch" "$libdir" "$interp" "$rootfs"
  fi

  # Two images, the same but for the engine: amneziawg-go as it is, and
  # -lowmem, with bounded packet buffers (ENGINE_LOWMEM, see
  # build-engine.sh), which stays within a small memory limit under load.
  for variant in default lowmem; do
    if [[ "$variant" == lowmem ]]; then
      ENGINE_LOWMEM=1 "$root/scripts/build-engine.sh" "$target" "$rootfs/usr/bin/uniray-engine"
      out="$root/build/uniray-gateway-$arch-lowmem.tar" tag=lowmem
    else
      ENGINE_LOWMEM="" "$root/scripts/build-engine.sh" "$target" "$rootfs/usr/bin/uniray-engine"
      out="$root/build/uniray-gateway-$arch.tar" tag=latest
    fi
    docker buildx build \
      --platform "$platform" \
      --build-context rootfs="$rootfs" \
      -f "$root/docker/gateway/Dockerfile" \
      -t "uniray-gateway:$tag" \
      -o "type=docker,dest=$out" \
      "$root/docker/gateway"
    # The file's size: du shows the blocks it takes, which a compressing
    # filesystem (ZFS, as on the CI runner) makes far smaller.
    echo "build-gateway: $out ($(ls -lh "$out" | awk '{print $5}'))"
  done
done

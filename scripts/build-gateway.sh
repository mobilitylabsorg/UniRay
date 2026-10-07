#!/usr/bin/env bash
# Builds the uniray-gateway container image for MikroTik RouterOS.
#
# Usage: scripts/build-gateway.sh [platform...]       (default: linux/arm/v7)
#   platforms: linux/arm/v7, linux/arm64, linux/amd64
#
# Output: build/uniray-gateway-<arch>.tar, a docker archive that RouterOS
# imports with `/container add file=uniray-gateway-<arch>.tar ...`.
#
# Needs docker (with buildx) and cross (https://github.com/cross-rs/cross).
set -euo pipefail

XRAY_VERSION="v26.9.30"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

platforms=("$@")
[[ ${#platforms[@]} -eq 0 ]] && platforms=(linux/arm/v7)

for platform in "${platforms[@]}"; do
  case "$platform" in
    linux/arm/v7) arch=armv7; target=armv7-unknown-linux-musleabihf; asset=linux-arm32-v7a; sha256=0b9719471c7c69752857714e9711d4da57cf38a6beb75dcddfb21425f7919908 ;;
    linux/arm64)  arch=arm64; target=aarch64-unknown-linux-musl;     asset=linux-arm64-v8a; sha256=9886f077f9fd8e6713b84c377c1c7db4e53b9bfa8c276a5bd12561139522b473 ;;
    linux/amd64)  arch=amd64; target=x86_64-unknown-linux-musl;      asset=linux-64;        sha256=f851110beaff16e78d643f0ccfd9524b4a44dfd59bae3e34bb52bba378f7690e ;;
    *) echo "build-gateway: unsupported platform $platform" >&2; exit 1 ;;
  esac
  echo "build-gateway: $platform"

  cross build --release -p uniray-gateway --target "$target"

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

  out="$root/build/uniray-gateway-$arch.tar"
  docker buildx build \
    --platform "$platform" \
    --build-context rootfs="$rootfs" \
    -f "$root/docker/gateway/Dockerfile" \
    -t "uniray-gateway:latest" \
    -o "type=docker,dest=$out" \
    "$root/docker/gateway"
  echo "build-gateway: $out ($(du -h "$out" | cut -f1))"
done

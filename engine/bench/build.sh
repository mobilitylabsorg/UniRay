#!/usr/bin/env bash
# Builds everything the benches run into <bin dir>: uniray-engine, the
# amneziawg-go server (the version go.mod pins), the key generator of the e2e
# test, the delay relay, Caddy with klzgrad's naive forwardproxy, and naive
# (scripts/fetch-naive.sh). Needs Go 1.26+ (or GOTOOLCHAIN=auto).
#
#   build.sh <bin dir>
set -euo pipefail

# Caddy and the forwardproxy fork, built in a module of their own: the engine's
# go.mod need not carry Caddy's dependencies.
CADDY_VERSION=v2.11.7
FORWARDPROXY_VERSION=v0.0.0-20261006113151-c096d6a00cb2

out=$1
engine="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
export GOTOOLCHAIN="${GOTOOLCHAIN:-auto}"

cd "$engine"
go build -o "$out/uniray-engine" .
go build -o "$out/amneziawg-go" github.com/amnezia-vpn/amneziawg-go/v3
go build -o "$out/keygen" ./e2e/keygen
go build -o "$out/delay" ./bench/delay

[ -x "$out/naive" ] || "$engine/../scripts/fetch-naive.sh" "$(go env GOARCH | sed 's/amd64/x86_64/;s/arm64/aarch64/')-unknown-linux-gnu" "$out/naive"

if [ ! -x "$out/caddy" ]; then
  work="$(mktemp -d)"
  trap 'rm -rf "$work"' EXIT
  cat >"$work/main.go" <<'EOF'
package main

import (
	caddycmd "github.com/caddyserver/caddy/v2/cmd"

	_ "github.com/caddyserver/caddy/v2/modules/standard"
	_ "github.com/caddyserver/forwardproxy"
)

func main() { caddycmd.Main() }
EOF
  (
    cd "$work"
    go mod init uniray-bench-caddy >/dev/null 2>&1
    go mod edit -require "github.com/caddyserver/caddy/v2@$CADDY_VERSION" \
      -replace "github.com/caddyserver/forwardproxy=github.com/klzgrad/forwardproxy@$FORWARDPROXY_VERSION"
    go mod tidy >/dev/null 2>&1
    go build -o "$out/caddy" .
  )
fi
echo "build.sh: $(ls "$out" | tr '\n' ' ')in $out"

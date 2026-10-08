#!/usr/bin/env bash
# Builds the engine, the amneziawg-go server (the commit go.mod pins) and the
# key generator, then runs e2e.sh. Needs root (network namespaces) and Go.
set -euo pipefail
cd "$(dirname "$0")/.."
bin="$(mktemp -d)"
trap 'rm -rf "$bin"' EXIT
go build -o "$bin/uniray-engine" .
go build -o "$bin/amneziawg-go" github.com/amnezia-vpn/amneziawg-go
go build -o "$bin/keygen" ./e2e/keygen
bash e2e/e2e.sh "$bin/uniray-engine" "$bin/amneziawg-go" "$bin/keygen"

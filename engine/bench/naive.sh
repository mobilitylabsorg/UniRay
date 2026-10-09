#!/usr/bin/env bash
# Throughput of NaiveProxy through uniray-engine (naive mode, as UniRay runs
# it) against naive alone (as other clients run it) and plain TCP, to a Caddy
# forwardproxy server (klzgrad's naive fork) over a veth pair. Needs root.
#
#   naive.sh <bin dir>
#
# <bin dir> is what build.sh makes (naive and caddy included). Environment:
#   MB=20   MiB per upload and per download
#
# naive (Chromium) trusts the system's certificates only: the bench's own CA
# is added to /etc/ssl/certs/ca-certificates.crt for the run and taken out on
# exit, whatever happens.
set -euo pipefail

bin=$1
here="$(cd "$(dirname "$0")" && pwd)"
bundle=/etc/ssl/certs/ca-certificates.crt
work=""
cleanup() {
  for ns in bcli bsrv; do ip netns pids $ns 2>/dev/null | xargs -r kill 2>/dev/null || true; done
  ip netns del bcli 2>/dev/null || true
  ip netns del bsrv 2>/dev/null || true
  if [ -n "$work" ] && [ -f "$work/bundle.orig" ]; then cp "$work/bundle.orig" "$bundle"; fi
  if [ -n "$work" ]; then rm -rf "$work"; fi
}
trap cleanup EXIT
cleanup
work="$(mktemp -d)"

# A CA and a certificate for naive.test, trusted for the run.
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 1 \
  -keyout "$work/ca.key" -out "$work/ca.crt" -subj "/CN=uniray bench CA" 2>/dev/null
openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
  -keyout "$work/server.key" -out "$work/server.csr" -subj "/CN=naive.test" 2>/dev/null
printf 'subjectAltName=DNS:naive.test\nextendedKeyUsage=serverAuth\n' >"$work/ext"
openssl x509 -req -in "$work/server.csr" -CA "$work/ca.crt" -CAkey "$work/ca.key" -CAcreateserial \
  -days 1 -extfile "$work/ext" -out "$work/server.crt" 2>/dev/null
cp "$bundle" "$work/bundle.orig"
cat "$work/ca.crt" >>"$bundle"

ip netns add bsrv
ip netns add bcli
ip link add b0 netns bcli type veth peer name b1 netns bsrv
ip -n bcli addr add 10.0.0.2/24 dev b0 && ip -n bcli link set b0 up && ip -n bcli link set lo up
ip -n bsrv addr add 10.0.0.1/24 dev b1 && ip -n bsrv link set b1 up && ip -n bsrv link set lo up
ip netns exec bsrv python3 "$here/sink.py" 10.0.0.1 &

mkdir -p "$work/www"
cat >"$work/Caddyfile" <<CADDY
{
	order forward_proxy before file_server
	auto_https off
	admin off
}
:443 {
	bind 10.0.0.1
	tls $work/server.crt $work/server.key
	forward_proxy {
		basic_auth u p
		hide_ip
		hide_via
		probe_resistance
		# The sink is on a private address, which the proxy refuses by default.
		acl {
			allow all
		}
	}
	file_server {
		root $work/www
	}
}
CADDY
ip netns exec bsrv "$bin/caddy" run --config "$work/Caddyfile" --adapter caddyfile >"$work/caddy.log" 2>&1 &
for _ in $(seq 50); do ip netns exec bsrv ss -tln | grep -q "10.0.0.1:443" && break; sleep 0.2; done

proxy=https://u:p@naive.test
rules="MAP naive.test 10.0.0.1"
ip netns exec bcli "$bin/naive" --listen=socks://u:p@127.0.0.1:1090 --proxy="$proxy" \
  --host-resolver-rules="$rules" >"$work/naive.log" 2>&1 &
python3 -c '
import json, sys
binary, proxy, rules = sys.argv[1:]
print(json.dumps({"user": "u", "pass": "p", "ownerUid": 0,
                  "naive": {"binary": binary, "config": {"proxy": proxy, "host-resolver-rules": rules}}}))' \
  "$bin/naive" "$proxy:443" "$rules" >"$work/engine.json"
mkfifo "$work/engine.in"
ip netns exec bcli "$bin/uniray-engine" naive --config "$work/engine.json" \
  <"$work/engine.in" >"$work/engine.out" 2>"$work/engine.err" &
exec 9>"$work/engine.in" # the engine exits when its stdin closes
for _ in $(seq 100); do grep -q '^ready ' "$work/engine.out" 2>/dev/null && break; sleep 0.1; done
port=$(sed -n 's/^ready //p' "$work/engine.out")
[ -n "$port" ] || { cat "$work/engine.err"; exit 1; }
sleep 1

measure() { ip netns exec bcli python3 "$here/client.py" "$@" 10.0.0.1 "${MB:-20}" 2>&1 | tail -1; }
echo "naive via uniray-engine: $(measure "127.0.0.1:$port" u p)"
echo "naive alone:             $(measure 127.0.0.1:1090 u p)"
echo "plain TCP:               $(measure - u p)"

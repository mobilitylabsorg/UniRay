#!/usr/bin/env bash
# End-to-end test of uniray-engine's AmneziaWG mode against the official
# amneziawg-go as the server, in two network namespaces (needs root):
#
#   [cli] client.py --SOCKS5--> uniray-engine ==AmneziaWG/UDP==> amneziawg-go [srv]
#                                                                 10.8.0.1: HTTP, UDP echo
#
# Checks: HTTP and UDP through the tunnel; refusals for a wrong password, for
# a process the engine was not told about, and for mismatched obfuscation
# (H1), which proves the AmneziaWG options are really in effect.
#
# Usage: e2e.sh <uniray-engine> <amneziawg-go> <keygen>
#   keygen prints "<priv b64> <pub b64> <priv hex> <pub hex>".
set -euo pipefail

engine=$1 server=$2 keygen=$3
here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"
cleanup() {
  ip netns pids cli 2>/dev/null | xargs -r kill 2>/dev/null || true
  ip netns pids srv 2>/dev/null | xargs -r kill 2>/dev/null || true
  ip netns del cli 2>/dev/null || true
  ip netns del srv 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT
failures=0
expect() { # expect <ok|fail> <line>
  local want=$1 line=$2
  if [[ "$line" == *" $want "* || "$line" == *" $want" ]]; then
    echo "PASS  $line"
  else
    echo "FAIL  (wanted $want) $line"
    failures=$((failures + 1))
  fi
}

read -r server_priv _ server_priv_hex server_pub_hex < <("$keygen")
read -r client_priv _ client_priv_hex client_pub_hex < <("$keygen")

# AmneziaWG 2.0 options: junk, paddings (S3/S4 are 2.0), header ranges.
awg="jc=4
jmin=10
jmax=50
s1=117
s2=19
s3=8
s4=16
h1=100000-100500
h2=200000-200500
h3=300000-300500
h4=400000-400500"

ip netns add srv
ip netns add cli
ip link add v0 netns cli type veth peer name v1 netns srv
ip -n cli addr add 10.0.0.2/24 dev v0 && ip -n cli link set v0 up && ip -n cli link set lo up
ip -n srv addr add 10.0.0.1/24 dev v1 && ip -n srv link set v1 up && ip -n srv link set lo up

# --- server: amneziawg-go with a kernel TUN, configured over its UAPI socket
ip netns exec srv env WG_PROCESS_FOREGROUND=1 "$server" awg0 >"$work/server.log" 2>&1 &
for _ in $(seq 1 50); do [ -S /var/run/amneziawg/awg0.sock ] && break; sleep 0.1; done
python3 - "$server_priv_hex" "$client_pub_hex" "$awg" <<'EOF'
import socket, sys
priv, peer, awg = sys.argv[1:]
config = f"set=1\nprivate_key={priv}\nlisten_port=51820\n{awg}\npublic_key={peer}\nallowed_ip=10.8.0.2/32\n\n"
s = socket.socket(socket.AF_UNIX)
s.connect("/var/run/amneziawg/awg0.sock")
s.sendall(config.encode())
reply = s.recv(4096).decode()
assert "errno=0" in reply, reply
EOF
ip -n srv addr add 10.8.0.1/24 dev awg0 && ip -n srv link set awg0 up
ip netns exec srv python3 -m http.server 8080 --bind 10.8.0.1 --directory "$work" >/dev/null 2>&1 &
ip netns exec srv python3 -c '
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.bind(("10.8.0.1", 9999))
while True:
    data, addr = s.recvfrom(65535); s.sendto(data, addr)' &
sleep 0.5

# --- client: the engine, as the app would start it
run_engine() { # run_engine <name> <awg options>
  local name=$1 options=$2
  cat >"$work/$name.json" <<EOF
{"user": "uniray", "pass": "s3cret-$name", "awg": {
  "addresses": ["10.8.0.2/32"], "mtu": 1420,
  "uapi": "private_key=$client_priv_hex\n${options//$'\n'/\\n}\nreplace_peers=true\npublic_key=$server_pub_hex\nendpoint=10.0.0.1:51820\npersistent_keepalive_interval=25\nreplace_allowed_ips=true\nallowed_ip=0.0.0.0/0\n"}}
EOF
  mkfifo "$work/$name.in"
  ip netns exec cli "$engine" awg --config "$work/$name.json" <"$work/$name.in" >"$work/$name.out" 2>"$work/$name.err" &
  exec {fd}>"$work/$name.in"
  eval "${name}_fd=$fd"
  for _ in $(seq 1 50); do grep -q '^ready ' "$work/$name.out" 2>/dev/null && break; sleep 0.1; done
  grep -q '^ready ' "$work/$name.out" || { cat "$work/$name.err"; exit 1; }
  eval "${name}_port=$(sed -n 's/^ready //p' "$work/$name.out")"
  [ ! -e "$work/$name.json" ] || { echo "FAIL  the config file was left on disk"; failures=$((failures + 1)); }
}

client() { # client <port> <pass> <register-with-fd|none> <checks...>
  local port=$1 pass=$2 fd=$3; shift 3
  rm -f "$work/ready" "$work/go"
  ip netns exec cli python3 "$here/client.py" "$port" uniray "$pass" "$work/ready" "$work/go" "$@" >"$work/client.out" &
  local pid=$!
  for _ in $(seq 1 50); do [ -s "$work/ready" ] && break; sleep 0.1; done
  if [ "$fd" != none ]; then echo "peer $(cat "$work/ready")" >&"$fd"; sleep 0.2; fi
  touch "$work/go"
  wait "$pid" || true
  cat "$work/client.out"
}

run_engine good "$awg"
echo "--- through the tunnel, as Xray"
while read -r line; do expect ok "$line"; done < <(client "$good_port" s3cret-good "$good_fd" http:10.8.0.1:8080 udp:10.8.0.1:9999)
echo "--- wrong password"
while read -r line; do expect fail "$line"; done < <(client "$good_port" wrong "$good_fd" http:10.8.0.1:8080)
echo "--- right password, but not the process the engine was told about"
while read -r line; do expect fail "$line"; done < <(client "$good_port" s3cret-good none http:10.8.0.1:8080)
grep -q "not Xray" "$work/good.err" && echo "PASS  the engine logged the refusal" || { echo "FAIL  no refusal logged"; failures=$((failures + 1)); }

echo "--- obfuscation mismatch (H1 outside the server's range)"
run_engine bad "${awg/h1=100000-100500/h1=999}"
while read -r line; do expect fail "$line"; done < <(client "$bad_port" s3cret-bad "$bad_fd" http:10.8.0.1:8080)

grep -q '^handshake ' "$work/good.out" && echo "PASS  the engine reported a handshake" || echo "note  no handshake line yet (reported every 5 s)"
if [ "$failures" -ne 0 ]; then
  echo "== $failures check(s) failed"; echo "--- engine log"; cat "$work/good.err"; echo "--- server log"; tail -20 "$work/server.log"
  exit 1
fi
echo "== all checks passed"

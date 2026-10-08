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
#
# With CHAIN_XRAY, CHAIN_HELPER, CHAIN_ASSETS (geo files) and CHAIN_CONFIGS
# (the engine_configs example of uniray-core) set, it also runs the whole
# chain as the app does - uniray-helper starting the engine and Xray with the
# configs the app generates - in proxy and in TUN mode:
#
#   [cli] curl --> Xray (mixed or TUN) --SOCKS5--> uniray-engine ==AWG==> [srv]
#
# In TUN mode the engine's own packets to the server are captured too, and
# must leave through Xray's bypass rule rather than loop back into the tunnel.
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
http_get() { # http_get <curl args...>: "ok 200", or "fail <code> <error>"
  local code
  code=$(ip netns exec cli curl --noproxy '' -sS -m 10 -o /dev/null -w '%{http_code}' "$@" 2>"$work/curl.err") || true
  if [ "$code" = 200 ]; then echo "ok 200"; else echo "fail $code $(head -c 200 "$work/curl.err")"; fi
}
expect() { # expect <ok|fail> <line>
  local want=$1 line=$2
  if [[ "$line" == *" $want "* || "$line" == *" $want" ]]; then
    echo "PASS  $line"
  else
    echo "FAIL  (wanted $want) $line"
    failures=$((failures + 1))
  fi
}

read -r server_priv server_pub server_priv_hex server_pub_hex < <("$keygen")
read -r client_priv client_pub client_priv_hex client_pub_hex < <("$keygen")

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
run_engine() { # run_engine <name> <awg options> [extra config, as JSON members]
  local name=$1 options=$2 extra=${3:-}
  cat >"$work/$name.json" <<EOF
{"user": "uniray", "pass": "s3cret-$name", ${extra:+$extra,} "awg": {
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

echo "--- Android: the owner's uid, from /proc/net (stealth, Android 9)"
run_engine uid0 "$awg" '"ownerUid": 0, "detached": true'
while read -r line; do expect ok "uid 0: $line"; done < <(client "$uid0_port" s3cret-uid0 none http:10.8.0.1:8080 udp:10.8.0.1:9999)
run_engine uid1 "$awg" '"ownerUid": 4242'
while read -r line; do expect fail "uid 4242: $line"; done < <(client "$uid1_port" s3cret-uid1 none http:10.8.0.1:8080)
exec {uid0_fd}>&-
sleep 0.5
if ip netns pids cli | xargs -r -n1 sh -c 'tr "\0" " " < /proc/$0/cmdline' 2>/dev/null | grep -q "$work/uid0.json"; then
  echo "PASS  detached: still running with stdin closed"
else
  echo "FAIL  detached: exited when stdin closed"; failures=$((failures + 1))
fi

echo "--- Android VPN mode: the owner's uid, from the VPN service's socket"
ip netns exec cli python3 "$here/owner.py" uniray-e2e-yes 0 "$work/questions" &
ip netns exec cli python3 "$here/owner.py" uniray-e2e-no 10123 "$work/questions" &
sleep 0.3
run_engine yes "$awg" '"ownerUid": 0, "ownerSocket": "@uniray-e2e-yes"'
while read -r line; do expect ok "owner says 0: $line"; done < <(client "$yes_port" s3cret-yes none http:10.8.0.1:8080 udp:10.8.0.1:9999)
run_engine no "$awg" '"ownerUid": 0, "ownerSocket": "@uniray-e2e-no"'
while read -r line; do expect fail "owner says 10123: $line"; done < <(client "$no_port" s3cret-no none http:10.8.0.1:8080)
grep -qE '^tcp 127\.0\.0\.1:[0-9]+ 127\.0\.0\.1:[0-9]+$' "$work/questions" && grep -qE '^udp 127\.0\.0\.1:[0-9]+ 127\.0\.0\.1:[0-9]+$' "$work/questions" \
  && echo "PASS  the questions name both ends" || { echo "FAIL  unexpected questions: $(head -3 "$work/questions")"; failures=$((failures + 1)); }

echo "--- obfuscation mismatch (H1 outside the server's range)"
run_engine bad "${awg/h1=100000-100500/h1=999}"
while read -r line; do expect fail "$line"; done < <(client "$bad_port" s3cret-bad "$bad_fd" http:10.8.0.1:8080)

chain() { # chain <proxy|tun>
  local mode=$1 dir="$work/chain-$1"
  mkdir -p "$dir"
  printf '%s\n' "[Interface]" "PrivateKey = $client_priv" "Address = 10.8.0.2/32" "DNS = 10.8.0.1" \
    "Jc = 4" "Jmin = 10" "Jmax = 50" "S1 = 117" "S2 = 19" "S3 = 8" "S4 = 16" \
    "H1 = 100000-100500" "H2 = 200000-200500" "H3 = 300000-300500" "H4 = 400000-400500" \
    "[Peer]" "PublicKey = $server_pub" "Endpoint = 198.51.100.1:51820" "AllowedIPs = 0.0.0.0/0" >"$dir/server.conf"
  "$CHAIN_CONFIGS" "$dir/server.conf" "$dir" 18080 18090 $([ "$mode" = tun ] && echo tun)
  if [ ! -e /proc/net/if_inet6 ]; then
    # A kernel without IPv6 (some containers) refuses the TUN's IPv6 side.
    python3 - "$dir/xray.json" <<'PY'
import json, sys
config = json.load(open(sys.argv[1]))
for inbound in config["inbounds"]:
    settings = inbound.get("settings", {})
    for key in ("gateway", "autoSystemRoutingTable"):
        if key in settings:
            settings[key] = [item for item in settings[key] if ":" not in item]
json.dump(config, open(sys.argv[1], "w"))
PY
  fi
  mkfifo "$dir/in"
  ip netns exec cli "$CHAIN_HELPER" "$CHAIN_XRAY" "$dir/xray.json" "$CHAIN_ASSETS" \
    --engine "$engine" awg "$dir/engine.json" <"$dir/in" >"$dir/log" 2>&1 &
  local helper=$!
  exec {chain_fd}>"$dir/in"
  for _ in $(seq 1 100); do
    ip netns exec cli python3 -c 'import socket; [socket.create_connection(("127.0.0.1", p), 1).close() for p in (18080, 18090)]' 2>/dev/null && break
    sleep 0.1
  done
  if [ "$mode" = tun ]; then
    for _ in $(seq 1 50); do ip -n cli link show uniray0 >/dev/null 2>&1 && break; sleep 0.1; done
    sleep 1
    expect ok "tun http $(http_get http://10.8.0.1:8080/)"
    expect ok "tun udp $(ip netns exec cli python3 "$here/udp_echo.py" 10.8.0.1 9999 2>/dev/null || echo fail)"
    if grep -q "accepted udp:198.51.100.1:51820 \[tun -> direct\]" "$dir/log"; then
      echo "PASS  the engine's packets left through the bypass rule"
    else
      echo "FAIL  the engine's packets did not go through the bypass rule"; failures=$((failures + 1))
    fi
  else
    expect ok "proxy http $(http_get -x socks5h://127.0.0.1:18080 http://10.8.0.1:8080/)"
    while read -r line; do expect ok "proxy $line"; done < <(client 18080 - none udp:10.8.0.1:9999)
  fi
  exec {chain_fd}>&-
  for _ in $(seq 1 100); do kill -0 "$helper" 2>/dev/null || break; sleep 0.1; done
  if kill -0 "$helper" 2>/dev/null; then
    echo "FAIL  the helper did not stop"; failures=$((failures + 1))
  else
    echo "PASS  the helper stopped"
  fi
  if pgrep -f "$dir/engine.json" >/dev/null; then
    echo "FAIL  the engine outlived the helper"; failures=$((failures + 1))
  fi
  if [ "$mode" = tun ] && ip -n cli link show uniray0 >/dev/null 2>&1; then
    echo "FAIL  the TUN outlived the helper"; failures=$((failures + 1))
  fi
  [ "$failures" -eq 0 ] || { echo "--- chain log ($mode)"; tail -30 "$dir/log"; }
}

if [ -n "${CHAIN_XRAY:-}" ]; then
  # Xray's TUN routes need a default route to find the way out. The server
  # gets an address only that route leads to, as on the internet, so that in
  # TUN mode the engine's own packets enter the TUN. The tunnel's addresses
  # are reachable only through the tunnel, not over the veth.
  ip -n cli route add default via 10.0.0.1 metric 100 # as DHCP would
  ip -n srv addr add 198.51.100.1/32 dev lo
  ip netns exec srv iptables -A INPUT -i v1 -d 10.8.0.0/24 -j DROP
  expect fail "around the tunnel $(http_get -m 3 http://10.8.0.1:8080/)"
  echo "--- chain, proxy mode"
  chain proxy
  echo "--- chain, TUN mode"
  chain tun
fi

grep -q 'handshake with the server done' "$work/good.err" && echo "PASS  the engine logged the handshake" || { echo "FAIL  no handshake logged"; failures=$((failures + 1)); }
grep -q 'handshake with the server done' "$work/bad.err" && { echo "FAIL  a handshake with mismatched H1"; failures=$((failures + 1)); } || echo "PASS  no handshake with mismatched H1"
if [ "$failures" -ne 0 ]; then
  echo "== $failures check(s) failed"; echo "--- engine log"; cat "$work/good.err"; echo "--- server log"; tail -20 "$work/server.log"
  exit 1
fi
echo "== all checks passed"

#!/usr/bin/env bash
# Throughput of WireGuard/AmneziaWG through uniray-engine (SOCKS in, gVisor's
# userspace TCP out) against amneziawg-go with a kernel TUN (the kernel's TCP,
# as in AmneziaVPN), to the same amneziawg-go server, over a path with delay
# and loss. Needs root (network namespaces).
#
#   awg.sh <bin dir> <one-way delay, ms> ["<awg options as UAPI key=value ...>"]
#
# <bin dir> is what build.sh makes. Environment:
#   LOSS=0.001     share of packets lost each way (default 0)
#   MB=30          MiB per upload and per download
#   KERNEL_CC="cubic bbr"   the kernel's congestion controls to measure (default cubic)
#   ENGINE=<path>  another uniray-engine to measure (default <bin dir>/uniray-engine)
#   PCAP=<file>    record the engine's upload as the server sees it (sniff.py)
#
# The kernel has no netem here, so the delay and loss come from the delay
# relay in front of the server (UDP, which is all WireGuard sends).
set -euo pipefail

bin=$1 delay=$2 options="${3:-}"
here="$(cd "$(dirname "$0")" && pwd)"
work=""
cleanup() {
  for ns in bcli bsrv; do ip netns pids $ns 2>/dev/null | xargs -r kill 2>/dev/null || true; done
  ip netns del bcli 2>/dev/null || true
  ip netns del bsrv 2>/dev/null || true
  if [ -n "$work" ]; then rm -rf "$work"; fi
}
trap cleanup EXIT
cleanup
work="$(mktemp -d)"

ip netns add bsrv
ip netns add bcli
ip link add b0 netns bcli type veth peer name b1 netns bsrv
ip -n bcli addr add 10.0.0.2/24 dev b0 && ip -n bcli link set b0 up && ip -n bcli link set lo up
ip -n bsrv addr add 10.0.0.1/24 dev b1 && ip -n bsrv link set b1 up && ip -n bsrv link set lo up
ip netns exec bsrv "$bin/delay" 10.0.0.1:51821 127.0.0.1:51820 "${delay}ms" "${LOSS:-0}" &

# The server, with two peers: the engine (10.8.0.2) and the kernel TUN client (10.8.0.3).
read -r _ _ server_priv server_pub < <("$bin/keygen")
read -r _ _ engine_priv engine_pub < <("$bin/keygen")
read -r _ _ kernel_priv kernel_pub < <("$bin/keygen")
mkdir -p /var/run/amneziawg
rm -f /var/run/amneziawg/bs0.sock /var/run/amneziawg/bk0.sock
ip netns exec bsrv env WG_PROCESS_FOREGROUND=1 "$bin/amneziawg-go" bs0 >"$work/server.log" 2>&1 &
ip netns exec bcli env WG_PROCESS_FOREGROUND=1 "$bin/amneziawg-go" bk0 >"$work/kernel-client.log" 2>&1 &
for _ in $(seq 50); do
  [ -S /var/run/amneziawg/bs0.sock ] && [ -S /var/run/amneziawg/bk0.sock ] && break
  sleep 0.1
done
uapi() {
  python3 -c '
import socket, sys
s = socket.socket(socket.AF_UNIX)
s.connect(sys.argv[1])
s.sendall(sys.argv[2].encode())
reply = s.recv(4096).decode()
assert "errno=0" in reply, reply' "$@"
}
# One option per line; no line at all without options, as an empty line ends
# a UAPI command.
opts=""
for option in $options; do opts+="$option"$'\n'; done
uapi /var/run/amneziawg/bs0.sock "set=1
private_key=$server_priv
listen_port=51820
${opts}public_key=$engine_pub
allowed_ip=10.8.0.2/32
public_key=$kernel_pub
allowed_ip=10.8.0.3/32

"
uapi /var/run/amneziawg/bk0.sock "set=1
private_key=$kernel_priv
${opts}public_key=$server_pub
endpoint=10.0.0.1:51821
allowed_ip=10.8.0.0/24

"
ip -n bsrv addr add 10.8.0.1/24 dev bs0 && ip -n bsrv link set bs0 mtu 1420 up
ip -n bcli addr add 10.8.0.3/24 dev bk0 && ip -n bcli link set bk0 mtu 1420 up
ip netns exec bsrv python3 "$here/sink.py" 10.8.0.1 &

# The engine, taking any root process (uid 0) as Xray.
python3 -c '
import json, sys
priv, server, options = sys.argv[1:]
uapi = f"private_key={priv}\n" + "".join(o + "\n" for o in options.split())
uapi += f"replace_peers=true\npublic_key={server}\nendpoint=10.0.0.1:51821\nreplace_allowed_ips=true\nallowed_ip=0.0.0.0/0\n"
print(json.dumps({"user": "u", "pass": "p", "ownerUid": 0,
                  "awg": {"addresses": ["10.8.0.2/32"], "mtu": 1420, "uapi": uapi}}))' \
  "$engine_priv" "$server_pub" "$options" >"$work/engine.json"
mkfifo "$work/engine.in"
ip netns exec bcli "${ENGINE:-$bin/uniray-engine}" awg --config "$work/engine.json" \
  <"$work/engine.in" >"$work/engine.out" 2>"$work/engine.err" &
exec 9>"$work/engine.in" # the engine exits when its stdin closes
for _ in $(seq 50); do grep -q '^ready ' "$work/engine.out" 2>/dev/null && break; sleep 0.1; done
port=$(sed -n 's/^ready //p' "$work/engine.out")
[ -n "$port" ] || { cat "$work/engine.err"; exit 1; }
sleep 1

if [ -n "${PCAP:-}" ]; then
  ip netns exec bsrv python3 "$here/sniff.py" bs0 9000 "$PCAP" &
  sniffer=$!
  sleep 1
fi
measure() { ip netns exec bcli python3 "$here/client.py" "$@" 10.8.0.1 "${MB:-30}" 2>&1 | tail -1; }
echo "uniray-engine (gVisor TCP):  $(measure "127.0.0.1:$port" u p)"
if [ -n "${sniffer:-}" ]; then sleep 1; kill "$sniffer"; fi
for cc in ${KERNEL_CC:-cubic}; do
  echo "kernel TUN, $cc:$(printf '%*s' $((15 - ${#cc})) '')$(CC=$cc measure - u p)"
done

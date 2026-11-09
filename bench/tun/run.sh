#!/bin/sh
# Inside `unshare -rn`. client ns (10.1.0.2) --veth, netem both ways-- this ns --tun xt0--> Xray --freedom--> 127.0.0.1
# Usage: run.sh <xray> <mode down|up> <streams> <seconds> <netem args...>
set -e
X="$1"; MODE="$2"; N="$3"; SECS="$4"; shift 4
D=$(dirname "$0")
ip link set lo up
echo 1 > /proc/sys/net/ipv4/ip_forward
python3 -I "$D/source.py" & SRC=$!
unshare -n sleep 300 & CPID=$!
sleep 0.2
ip link add name vx0 type veth peer name vx1
ip link set vx1 netns $CPID
ip addr add 10.1.0.1/24 dev vx0; ip link set vx0 up
nsenter -t $CPID -n sh -c 'ip link set lo up; ip addr add 10.1.0.2/24 dev vx1; ip link set vx1 up; ip route add default via 10.1.0.1'
if [ $# -gt 0 ]; then
  tc qdisc add dev vx0 root netem "$@"
  nsenter -t $CPID -n tc qdisc add dev vx1 root netem "$@"
fi
GOMAXPROCS=2 "$X" run -c "$D/xray.json" > "${LOG:-/dev/null}" 2>&1 & XP=$!
i=0; while ! ip link show xt0 >/dev/null 2>&1; do i=$((i+1)); [ $i -gt 50 ] && { echo "no tun"; cat "${LOG:-/dev/null}"; exit 1; }; sleep 0.1; done
ip link set xt0 up 2>/dev/null || true
ip route add 10.200.0.0/24 dev xt0
sleep 0.3
cpu() { awk '{print $14+$15}' /proc/$XP/stat; }
c0=$(cpu)
PORT=5001; [ "$MODE" = up ] && PORT=5002
R=$(nsenter -t $CPID -n python3 -I "$D/load.py" "$MODE" 10.200.0.1 $PORT "$N" "$SECS")
c1=$(cpu)
HZ=$(getconf CLK_TCK)
echo "$R" | awk -v c=$((c1-c0)) -v hz=$HZ '{gb=$2/1e9; printf "%8.1f MB/s  %6.2f CPU-s/GB\n", $1, (gb>0? c/hz/gb : 0)}'
kill $XP $SRC $CPID 2>/dev/null || true

"""load.py down|up host port streams seconds -> prints total MB/s and bytes."""
import socket, sys, threading, time
mode, host, port, n, dur = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4]), float(sys.argv[5])
tot = [0] * n
def run(i):
    try:
        s = socket.create_connection((host, port), timeout=10)
    except Exception:
        return
    b = bytearray(1 << 18); t0 = time.monotonic()
    try:
        while time.monotonic() - t0 < dur:
            k = s.recv_into(b) if mode == "down" else s.send(b)
            if not k: break
            tot[i] += k
    except Exception: pass
    s.close()
ts = [threading.Thread(target=run, args=(i,)) for i in range(n)]
t0 = time.monotonic(); [t.start() for t in ts]; [t.join() for t in ts]
el = time.monotonic() - t0
print(f"{sum(tot)/el/1e6:.1f} {sum(tot)}")

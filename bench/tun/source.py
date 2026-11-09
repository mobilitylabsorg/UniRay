"""127.0.0.1:5001 streams data to every connection (download); 127.0.0.1:5002 sinks (upload)."""
import socket, threading
def serve(port, fn):
    s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("127.0.0.1", port)); s.listen(64)
    while True:
        c, _ = s.accept(); threading.Thread(target=fn, args=(c,), daemon=True).start()
def send(c):
    b = bytes(1 << 18)
    try:
        while True: c.sendall(b)
    except Exception: pass
def sink(c):
    b = bytearray(1 << 18)
    try:
        while c.recv_into(b): pass
    except Exception: pass
threading.Thread(target=serve, args=(5002, sink), daemon=True).start()
serve(5001, send)

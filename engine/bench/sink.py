# The far end of the bench: on <host>:9000 it takes "<n>\n" and n bytes, then
# answers "k" (an upload is over when its last byte is in, without relying on
# a half-close through the proxies); on :9001 it takes "<n>\n" and sends n
# bytes (a download).
#   sink.py <host>
import socket, sys, threading

host = sys.argv[1]


def sink(c):
    f = c.makefile("rb")
    n, got = int(f.readline()), 0
    while got < n:
        d = f.read1(1 << 16)
        if not d:
            break
        got += len(d)
    c.sendall(b"k")
    c.close()


def source(c):
    n = int(c.makefile("rb").readline())
    chunk = b"\0" * 65536
    while n > 0:
        c.sendall(chunk[: min(n, len(chunk))])
        n -= len(chunk)
    c.close()


def serve(port, handle):
    s = socket.socket()
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind((host, port))
    s.listen(16)
    while True:
        threading.Thread(target=handle, args=(s.accept()[0],), daemon=True).start()


for port, handle in ((9000, sink), (9001, source)):
    threading.Thread(target=serve, args=(port, handle), daemon=True).start()
threading.Event().wait()

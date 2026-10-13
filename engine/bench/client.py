# One upload and one download of <MB> MiB to sink.py, through a SOCKS5 proxy
# with a username and password (the engine's, naive's) or, with "-", directly
# (then CC picks the kernel's congestion control for the socket).
#   client.py <proxy host:port | -> <user> <pass> <sink host> <MB>
# Prints "up X Mbit/s   down Y Mbit/s".
import os, socket, struct, sys, time

proxy, user, password, dst, mb = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4], int(sys.argv[5])


def connect(port):
    if proxy == "-":
        s = socket.socket()
        s.settimeout(120)
        if os.environ.get("CC"):
            s.setsockopt(socket.IPPROTO_TCP, socket.TCP_CONGESTION, os.environ["CC"].encode())
        s.connect((dst, port))
        return s
    host, p = proxy.rsplit(":", 1)
    s = socket.create_connection((host, int(p)), 120)
    s.sendall(b"\x05\x01\x02")
    assert s.recv(2) == b"\x05\x02", "the proxy wants no password"
    s.sendall(bytes([1, len(user)]) + user.encode() + bytes([len(password)]) + password.encode())
    assert s.recv(2)[1] == 0, "credentials refused"
    s.sendall(b"\x05\x01\x00\x01" + socket.inet_aton(dst) + struct.pack(">H", port))
    reply = s.recv(10)
    assert reply[1] == 0, f"CONNECT refused: {reply!r}"
    return s


n, chunk = mb << 20, b"\0" * 65536
s = connect(9000)
s.sendall(f"{n}\n".encode())
start, sent = time.time(), 0
while sent < n:
    s.sendall(chunk)
    sent += len(chunk)
assert s.recv(1) == b"k"
up = time.time() - start
s.close()

s = connect(9001)
s.sendall(f"{n}\n".encode())
start, got = time.time(), 0
while d := s.recv(1 << 16):
    got += len(d)
down = time.time() - start
print(f"up {n * 8 / up / 1e6:.1f} Mbit/s   down {got * 8 / down / 1e6:.1f} Mbit/s")

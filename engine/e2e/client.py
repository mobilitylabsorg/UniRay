"""A SOCKS5 client standing in for Xray in the engine's end-to-end test.

Usage: client.py <port> <user> <pass> <ready-file> <go-file> <check>...

Writes its pid to <ready-file>, waits for <go-file> (the test registers the
pid with the engine in between), then runs the checks and prints one
"<check> ok|fail <detail>" line each:

  http:<host>:<port>   GET / through CONNECT, ok on HTTP 200
  udp:<host>:<port>    a datagram through UDP ASSOCIATE, ok when echoed
"""

import os
import socket
import struct
import sys
import time


def handshake(port, user, password):
    sock = socket.create_connection(("127.0.0.1", port), timeout=10)
    sock.sendall(b"\x05\x01\x02")
    if sock.recv(2) != b"\x05\x02":
        raise RuntimeError("password method refused")
    sock.sendall(b"\x01" + bytes([len(user)]) + user + bytes([len(password)]) + password)
    if sock.recv(2) != b"\x01\x00":
        raise RuntimeError("credentials refused")
    return sock


def request(sock, cmd, host, port):
    ip = socket.inet_aton(host)
    sock.sendall(b"\x05" + bytes([cmd]) + b"\x00\x01" + ip + struct.pack(">H", port))
    reply = sock.recv(10)
    if len(reply) < 10 or reply[1] != 0:
        raise RuntimeError(f"request refused: {reply!r}")
    return socket.inet_ntoa(reply[4:8]), struct.unpack(">H", reply[8:10])[0]


def check_http(port, user, password, host, target_port):
    sock = handshake(port, user, password)
    request(sock, 1, host, target_port)
    sock.sendall(f"GET / HTTP/1.0\r\nHost: {host}\r\n\r\n".encode())
    data = b""
    while chunk := sock.recv(4096):
        data += chunk
    status = data.split(b"\r\n", 1)[0].decode(errors="replace")
    return "200" in status, status


def check_udp(port, user, password, host, target_port):
    sock = handshake(port, user, password)
    relay = request(sock, 3, "0.0.0.0", 0)
    udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    udp.settimeout(10)
    payload = b"ping-through-amneziawg"
    header = b"\x00\x00\x00\x01" + socket.inet_aton(host) + struct.pack(">H", target_port)
    udp.sendto(header + payload, relay)
    data, _ = udp.recvfrom(65535)
    echoed = data[10:]
    return echoed == payload, repr(echoed)


def main():
    port, user, password, ready, go = sys.argv[1:6]
    port = int(port)
    user, password = user.encode(), password.encode()
    with open(ready, "w") as f:
        f.write(str(os.getpid()))
    for _ in range(100):
        if os.path.exists(go):
            break
        time.sleep(0.1)
    for check in sys.argv[6:]:
        kind, host, target = check.split(":")
        try:
            run = check_http if kind == "http" else check_udp
            ok, detail = run(port, user, password, host, int(target))
        except Exception as err:  # noqa: BLE001 - a failed check is the result
            ok, detail = False, f"{type(err).__name__}: {err}"
        print(f"{check} {'ok' if ok else 'fail'} {detail}", flush=True)


if __name__ == "__main__":
    main()

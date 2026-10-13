#!/usr/bin/env python3
"""Stands in for `naive` in the engine's e2e test (the real one needs a
NaiveProxy server): reads naive's config file as naive does, records it next
to itself (config.seen) for the test, and serves the SOCKS5 `listen` it names
- with its credentials - connecting directly instead of through a server.
"""

import json
import os
import socket
import struct
import sys
import threading
from urllib.parse import unquote, urlsplit

config = json.load(open(sys.argv[1]))
with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "config.seen"), "w") as seen:
    json.dump({**config, "file": sys.argv[1]}, seen)
listen = urlsplit(config["listen"])
assert listen.scheme == "socks" and config["proxy"], config
user, password = unquote(listen.username).encode(), unquote(listen.password).encode()


def pipe(src, dst):
    try:
        while data := src.recv(65536):
            dst.sendall(data)
    except OSError:
        pass
    finally:
        for sock in (src, dst):
            try:
                sock.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass


def serve(conn):
    try:
        conn.recv(3)
        conn.sendall(b"\x05\x02")
        version, ulen = conn.recv(2)
        got_user = conn.recv(ulen)
        plen = conn.recv(1)[0]
        got_pass = conn.recv(plen)
        if (got_user, got_pass) != (user, password):
            conn.sendall(b"\x01\x01")
            return conn.close()
        conn.sendall(b"\x01\x00")
        _, cmd, _, atyp = conn.recv(4)
        if atyp == 1:
            host = socket.inet_ntoa(conn.recv(4))
        elif atyp == 3:
            host = conn.recv(conn.recv(1)[0]).decode()
        else:
            host = socket.inet_ntop(socket.AF_INET6, conn.recv(16))
        port = struct.unpack(">H", conn.recv(2))[0]
        upstream = socket.create_connection((host, port), timeout=10)
        conn.sendall(b"\x05\x00\x00\x01\x00\x00\x00\x00\x00\x00")
        threading.Thread(target=pipe, args=(upstream, conn), daemon=True).start()
        pipe(conn, upstream)
    except Exception:
        conn.close()


server = socket.socket()
server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
server.bind((listen.hostname, listen.port))
server.listen(16)
while True:
    client, _ = server.accept()
    threading.Thread(target=serve, args=(client,), daemon=True).start()

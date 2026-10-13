"""Sends a datagram to <host> <port> and prints ok when it comes back."""

import socket
import sys

sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.settimeout(10)
payload = b"ping-through-the-tun"
sock.sendto(payload, (sys.argv[1], int(sys.argv[2])))
print("ok" if sock.recvfrom(1024)[0] == payload else "fail")

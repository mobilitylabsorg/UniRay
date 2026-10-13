"""Stands in for the Android VPN service's owner socket in the e2e test.

Usage: owner.py <abstract name> <uid to answer> <question log>

Answers every question with <uid> and writes the questions to the log.
"""

import socket
import sys

name, uid, log = sys.argv[1], sys.argv[2], sys.argv[3]
server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
server.bind("\0" + name)
server.listen(16)
with open(log, "a", buffering=1) as questions:
    while True:
        conn, _ = server.accept()
        with conn:
            question = conn.makefile().readline().strip()
            questions.write(question + "\n")
            conn.sendall(f"{uid}\n".encode())

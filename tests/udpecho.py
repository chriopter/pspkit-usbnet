#!/usr/bin/env python3
# udpecho.py [port]: sends every datagram back to where it came from (127.0.0.1 only).
import socket, sys
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.bind(("127.0.0.1", int(sys.argv[1]) if len(sys.argv) > 1 else 8977))
while True:
    data, peer = s.recvfrom(65535)
    s.sendto(data, peer)

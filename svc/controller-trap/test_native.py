#!/usr/bin/env python3

import socket

payload = """
u sensor:env/temp 1 25.57
a unit:tests/door t
a unit:tests/u2 1
"""

sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.sendto(payload.encode(), ('127.0.0.1', 1162))

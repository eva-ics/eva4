#!/usr/bin/env python3

import socket
import bz2
from Cryptodome import Random
from Cryptodome.Cipher import AES
from hashlib import sha256

sender = ''
key_id = 'default'
key_val = 'defaultXXX'

payload = """
u sensor:env/temp 1 25.57
a unit:tests/door t
a unit:tests/u2 1
"""

flags = 2 + (1 << 4)  # 2 = AES-256-GCM, 5th bit = 1 - bzip2
nonce = Random.new().read(12)
hasher = sha256()
hasher.update(key_val.encode())
cipher = AES.new(hasher.digest(), AES.MODE_GCM, nonce)
frame, digest = cipher.encrypt_and_digest(bz2.compress(payload.encode()))
binary_payload = b'\x00\x01' + flags.to_bytes(
    1, 'little') + b'\x00\x00' + sender.encode() + b'\x00' + key_id.encode(
    ) + b'\x00' + frame + digest + nonce
sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.sendto(binary_payload, ('127.0.0.1', 1162))

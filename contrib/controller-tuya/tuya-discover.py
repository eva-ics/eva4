__version__ = '0.0.1'

import evaics.sdk as sdk

from evaics.sdk import pack, unpack

import threading
import busrt
import asyncio
import json
import traceback

from Cryptodome.Cipher import AES
from hashlib import md5
from types import SimpleNamespace

pad = lambda s: s + (16 - len(s) % 16) * chr(16 - len(s) % 16)
unpad = lambda s: s[:-ord(s[len(s) - 1:])]
encrypt = lambda msg, key: AES.new(key, AES.MODE_ECB).encrypt(pad(msg).encode())
decrypt = lambda msg, key: unpad(AES.new(key, AES.MODE_ECB).decrypt(msg)
                                ).decode()
DEFAULT_DISCOVERY_TIMEOUT = 4

d = SimpleNamespace(service=None, local_addr=None)

# discovery is based on
# https://raw.githubusercontent.com/ct-Open-Source/tuya-convert/refs/heads/master/scripts/tuya-discovery.py

udpkey = md5(b"yGAdlopoPVldABfn").digest()
decrypt_udp = lambda msg: decrypt(msg, udpkey)

devices_seen = set()
devices = []

discovery_active_lock = threading.Lock()


class TuyaDiscovery(asyncio.DatagramProtocol):

    def datagram_received(self, data, addr):
        # ignore devices we've already seen
        if data in devices_seen:
            return
        devices_seen.add(data)
        # remove message frame
        data = data[20:-8]
        # decrypt if encrypted
        try:
            data = decrypt_udp(data)
        except:
            try:
                data = data.decode()
            except:
                return
        # parse json
        try:
            data = json.loads(data)
            devices.append(data)
        except:
            pass


def discover(local_addr, discovery_timeout):
    if discovery_active_lock.locked():
        raise Exception("Discovery already active")
    with discovery_active_lock:
        devices.clear()
        devices_seen.clear()
        try:
            loop = asyncio.get_event_loop()
        except RuntimeError as e:
            if str(e).startswith('There is no current event loop in thread'):
                loop = asyncio.new_event_loop()
                asyncio.set_event_loop(loop)
            else:
                raise
        listener = loop.create_datagram_endpoint(TuyaDiscovery,
                                                 local_addr=(local_addr, 6666),
                                                 reuse_port=True)
        encrypted_listener = loop.create_datagram_endpoint(
            TuyaDiscovery, local_addr=(local_addr, 6667), reuse_port=True)
        loop.run_until_complete(listener)
        loop.run_until_complete(encrypted_listener)
        loop.run_until_complete(asyncio.sleep(discovery_timeout))
    return devices


def handle_rpc(event):
    d.service.need_ready()
    if event.method == b'discover':
        try:
            payload = event.get_payload()
            if payload:
                data = unpack(payload)
                t = data.get('discovery_timeout', DEFAULT_DISCOVERY_TIMEOUT)
            else:
                t = DEFAULT_DISCOVERY_TIMEOUT
            devices = discover(d.local_addr, t)
            return pack(devices)
        except Exception as e:
            d.service.logger.error(traceback.format_exc())
            raise busrt.rpc.RpcException(str(e), sdk.ERR_CODE_FUNC_FAILED)
    else:
        sdk.no_rpc_method()


def run():
    info = sdk.ServiceInfo(author='Bohemia Automation',
                           description='Tuya discovery service',
                           version=__version__)
    info.add_method('discover', optional=['discovery_timeout'])
    service = sdk.Service()
    d.service = service
    config = service.get_config()
    try:
        d.local_addr = config['local_addr']
    except:
        d.local_addr = '0.0.0.0'

    service.init(info, on_rpc_call=handle_rpc)
    service.block()


run()

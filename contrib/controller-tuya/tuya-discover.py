__version__ = '0.0.2'

import evaics.sdk as sdk

from evaics.sdk import pack, unpack
from tinytuya import deviceScan

import threading
import busrt
import traceback

from types import SimpleNamespace

DEFAULT_DISCOVERY_TIMEOUT = 4

d = SimpleNamespace(service=None)

discovery_active_lock = threading.Lock()

def discover(discovery_timeout):
    if discovery_active_lock.locked():
        raise Exception("Discovery already active")
    with discovery_active_lock:
        return deviceScan(maxretry=int(discovery_timeout))


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
            devices = discover(t)
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
    service.init(info, on_rpc_call=handle_rpc)
    service.block()

run()

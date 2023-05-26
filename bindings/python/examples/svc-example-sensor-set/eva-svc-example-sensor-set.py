#!/opt/eva4/venv/bin/python

__version__ = '0.0.1'

import evaics.sdk as sdk
import busrt

from types import SimpleNamespace
from evaics.sdk import pack, unpack, OID, RAW_STATE_TOPIC, XCall

# define a global namespace
_d = SimpleNamespace(service=None)


# RPC calls handler
def handle_rpc(event):
    # handle X calls from HMI
    if event.method == b'x':
        try:
            xp = XCall(unpack(event.get_payload()))
            if xp.method == 'set':
                oid = OID(xp.params['i'])
                status = xp.params['status']
                # check if the item is a sensor
                if oid.kind != 'sensor':
                    raise busrt.rpc.RpcException(
                        'can set state for sensors only',
                        sdk.ERR_CODE_ACCESS_DENIED)
                # check if the caller's session is not a read-only one
                xp.require_writable()
                # check if the caller's ACL has write access to the provided OID
                xp.require_item_write(oid)
                # set the sensor state
                # in this example the service does not check does the sensor
                # really exist in the core or not
                event = dict(status=status)
                if 'value' in xp.params:
                    event['value'] = xp.params['value']
                topic = f'{RAW_STATE_TOPIC}{oid.to_path()}'
                _d.service.bus.send(
                    topic,
                    busrt.client.Frame(pack(event), tp=busrt.client.OP_PUBLISH))
                return
            else:
                sdk.no_rpc_method()
        except busrt.rpc.RpcException as e:
            raise e
        except Exception as e:
            raise busrt.rpc.RpcException(str(e), sdk.ERR_CODE_FUNC_FAILED)
    else:
        sdk.no_rpc_method()


def run():
    info = sdk.ServiceInfo(author='Bohemia Automation',
                           description='Sensor state manipulations',
                           version=__version__)
    service = sdk.Service()
    _d.service = service
    service.init(info, on_rpc_call=handle_rpc)
    service.block()


run()

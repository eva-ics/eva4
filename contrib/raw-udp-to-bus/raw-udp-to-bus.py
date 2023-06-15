__version__ = '0.0.1'

import evaics.sdk as sdk

from evaics.sdk import pack, OID, RAW_STATE_TOPIC

from struct import unpack as unpack_number

from types import SimpleNamespace

import busrt
import socket
import threading

d = SimpleNamespace(service=None, ports=[])


def handle_rpc(event):
    d.service.need_ready()
    if event.method == b'port.list':
        try:
            return pack(d.ports)
        except Exception as e:
            raise busrt.rpc.RpcException(str(e), sdk.ERR_CODE_FUNC_FAILED)
    else:
        sdk.no_rpc_method()


def udp_server(addr, port, oid, decode):
    service = d.service
    topic = RAW_STATE_TOPIC + oid.to_path()
    try:
        server = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        server.bind((addr, port))
    except Exception as e:
        service.logger.error(f'Unable to bind to {addr}:{port} for {oid}: {e}')
        raise
    while service.is_active():
        try:
            data, _ = server.recvfrom(1024)
            n = unpack_number(decode, data)[0]
            payload = {'status': 1, 'value': n}
            service.bus.send(
                topic,
                busrt.client.Frame(pack(payload),
                                   tp=busrt.client.OP_PUBLISH,
                                   qos=0))
        except Exception as e:
            service.logger.error(f'{port} {oid} processing error: {e}')


def run():
    info = sdk.ServiceInfo(author='Bohemia Automation',
                           description='Raw UDP sink',
                           version=__version__)
    info.add_method('port.list')
    service = sdk.Service()
    config = service.get_config()

    listen = config['listen']
    port_map = config.get('map', {})
    service.init(info, on_rpc_call=handle_rpc)

    d.service = service

    for (port, params) in port_map.items():
        port = int(port)
        decode = params.get('decode', '<d')
        oid = params['oid']
        server = threading.Thread(target=udp_server,
                                  daemon=True,
                                  args=(listen, port, OID(oid),
                                        decode)).start()
        d.ports.append(dict(port=port, oid=oid, decode=decode))

    d.ports.sort(key=lambda k: k['port'])

    service.block()


run()

__version__ = '0.0.1'

import evaics.sdk as sdk

from evaics.sdk import pack, unpack, OID, LOCAL_STATE_TOPIC

from struct import pack as pack_number

from types import SimpleNamespace

import busrt
import socket
import threading
import time
import traceback

d = SimpleNamespace(service=None, ports=[], target=None, process_map={})


def handle_rpc(event):
    d.service.need_ready()
    if event.method == b'port.list':
        try:
            return pack(d.ports)
        except Exception as e:
            raise busrt.rpc.RpcException(str(e), sdk.ERR_CODE_FUNC_FAILED)
    else:
        sdk.no_rpc_method()


def on_frame(frame):
    if frame.topic and frame.topic.startswith(LOCAL_STATE_TOPIC):
        data = unpack(frame.payload)
        val = data.get('value')
        if val is not None:
            p = frame.topic[len(LOCAL_STATE_TOPIC):]
            process_state(p, val)


def process_state(p, val):
    params = d.process_map.get(p)
    if params is not None:
        (port, encode) = params
        if encode[-1] in ['f', 'd']:
            try:
                val = float(val)
            except:
                return
        else:
            try:
                val = int(val)
            except:
                return
        d.sock.sendto(pack_number(encode, val), (d.target, port))


def collect(interval, oids):
    payload = pack({'i': oids})
    d.service.wait_core()
    while d.service.is_active():
        start = time.perf_counter()
        try:
            states = unpack(
                d.service.rpc.call('eva.core',
                                   busrt.rpc.Request(
                                       'item.state',
                                       payload)).wait_completed().get_payload())
            for state in states:
                oid = state['oid']
                val = state['value']
                if val is not None:
                    process_state(oid, val)
        except Exception as e:
            d.service.logger.error(
                f'interval loop error: {traceback.format_exc(e)}')
        elapsed = time.perf_counter() - start
        to_sleep = interval - elapsed
        if to_sleep <= 0:
            d.service.logger.warning('interval loop timeout')
        else:
            time.sleep(to_sleep)


def run():
    info = sdk.ServiceInfo(author='Bohemia Automation',
                           description='Raw UDP bridge',
                           version=__version__)
    info.add_method('port.list')
    service = sdk.Service()
    config = service.get_config()

    d.target = config['target']

    source_port = config.get('source_port')

    interval = config.get('interval')

    if interval:
        interval = float(interval)

    d.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)

    if source_port:
        d.sock.bind(('0.0.0.0', source_port))

    port_map = config.get('map', {})
    service.init(info,
                 on_frame=None if interval else on_frame,
                 on_rpc_call=handle_rpc)

    d.service = service

    oids = []

    for (oid, params) in port_map.items():
        port = params['port']
        port = int(port)
        encode = params.get('encode', '<d')
        o = OID(oid)
        d.ports.append(dict(port=port, oid=oid, encode=encode))
        d.process_map[oid if interval else o.to_path()] = (port, encode)
        oids.append(oid)

    d.ports.sort(key=lambda k: k['port'])

    if interval:
        threading.Thread(target=collect, daemon=True,
                         args=(interval, oids)).start()
    else:
        service.subscribe_oids(oids, event_kind='local')

    service.block()


run()

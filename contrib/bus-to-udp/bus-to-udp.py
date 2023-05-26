__version__ = '0.0.1'

import evaics.sdk as sdk

from evaics.sdk import pack, unpack, OID, LOCAL_STATE_TOPIC

from types import SimpleNamespace

import socket
import json

d = SimpleNamespace(sock=None, target_host=None, target_port=None, packer=None)


def pack_json(payload):
    return bytes(json.dumps(payload), 'utf-8')


def on_frame(frame):
    if frame.topic and frame.topic.startswith(LOCAL_STATE_TOPIC):
        data = unpack(frame.payload)
        data['oid'] = str(
            OID(frame.topic[len(LOCAL_STATE_TOPIC):], from_path=True))
        d.sock.sendto(d.packer(data), (d.target_host, d.target_port))


def run():
    info = sdk.ServiceInfo(author='Bohemia Automation',
                           description='EAPI to UDP',
                           version=__version__)
    service = sdk.Service()
    config = service.get_config()
    fmt = config.get('format', 'msgpack')
    if fmt == 'json':
        d.packer = pack_json
    elif fmt == 'msgpack':
        d.packer = pack
    else:
        raise RuntimeError(f'unsupported payload format: {fmt}')
    d.target_host, d.target_port = config['target'].rsplit(':', maxsplit=1)
    d.target_port = int(d.target_port)
    d.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)

    service.init(info, on_frame=on_frame)
    service.subscribe_oids(config.get('oids', []), event_kind='local')
    service.logger.info(f'target: {d.target_host}:{d.target_port}')
    service.block()


run()

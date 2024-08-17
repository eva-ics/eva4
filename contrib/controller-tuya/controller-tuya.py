__version__ = '0.0.1'

import sys

sys.path.insert(0, '/opt/eva4/bindings/python/eva-ics-sdk')

import evaics.sdk as sdk

from evaics.sdk import pack, OID, RAW_STATE_TOPIC

import tinytuya
import threading
import busrt
import time
import traceback

from types import SimpleNamespace

d = SimpleNamespace(device=None, map=dict())


def handle_rpc(event):
    d.service.need_ready()
    sdk.no_rpc_method()


def pull(interval, map):
    d.service.wait_core()
    while d.service.is_active():
        start = time.perf_counter()
        try:
            for (dps, value) in d.device.status()['dps'].items():
                oid = map.get(dps)
                if oid is None:
                    continue
                if value is True:
                    value = 1
                elif value is False:
                    value = 0
                payload = pack({'status': 1, 'value': value})
                topic = RAW_STATE_TOPIC + oid.to_path()
                d.service.bus.send(
                    topic,
                    busrt.client.Frame(payload,
                                       tp=busrt.client.OP_PUBLISH,
                                       qos=0))
        except Exception:
            mark_failed(map)
            d.service.logger.error(
                f'interval loop error: {traceback.format_exc()}')
        elapsed = time.perf_counter() - start
        to_sleep = interval - elapsed
        if to_sleep <= 0:
            d.service.logger.warning('interval loop timeout')
        else:
            time.sleep(to_sleep)


def mark_failed(map):
    payload = pack({'status': sdk.ITEM_STATUS_ERROR})
    for oid in map.values():
        topic = RAW_STATE_TOPIC + oid.to_path()
        d.service.bus.send(
            topic, busrt.client.Frame(payload,
                                      tp=busrt.client.OP_PUBLISH,
                                      qos=0))


def run():
    info = sdk.ServiceInfo(author='Bohemia Automation',
                           description='Tuya controller',
                           version=__version__)
    service = sdk.Service()
    d.service = service
    config = service.get_config()

    map = {k: OID(v) for k, v in config.get('map', {}).items()}

    service.init(info, on_rpc_call=handle_rpc)

    if service.is_mode_rtf():
        mark_failed(map)
        return

    dev_id = config['device']['id']
    address = config['device']['addr']
    local_key = config['device']['local_key']
    api_version = config['device']['version']

    d.device = tinytuya.Device(dev_id=dev_id,
                               address=address,
                               local_key=local_key,
                               version=api_version,
                               connection_timeout=service.timeout['default'])

    threading.Thread(target=pull,
                     daemon=True,
                     args=(config['pull_interval'], map)).start()
    service.block()


run()

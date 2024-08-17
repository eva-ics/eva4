__version__ = '0.0.1'

import evaics.sdk as sdk

from evaics.sdk import pack, OID, RAW_STATE_TOPIC, Controller, Action

import tinytuya
import threading
import busrt
import time
import traceback

from types import SimpleNamespace

d = SimpleNamespace(device=None,
                    service=None,
                    controller=None,
                    action_lock=threading.Lock(),
                    map=dict(),
                    action_map=dict())


def run_action(action):
    c = d.controller
    c.event_running(action)
    m = d.action_map.get(action.i)
    if m is None:
        c.event_failed(action, out='unit not mapped')
        return
    try:
        dps = str(m['dps'])
        kind = m['kind']
        value = action.params['value']
        if m.get('convert_bool'):
            value = bool(value)
        with d.action_lock:
            if kind == 'control':
                p = d.device.generate_payload(tinytuya.CONTROL, {dps: value})
                result = d.device._send_receive(p)
            elif kind == 'set_value':
                result = d.device.set_value(dps, value)
            else:
                raise ValueError(f'unsupported action kind: {kind}')
        if result and 'data' in result:
            new_value = result['data']['dps'][dps]
            if new_value != value:
                raise RuntimeError('device value not modified')
            update(result['data']['dps'])
        else:
            raise RuntimeError('action failed')
        c.event_completed(action)
    except Exception as e:
        c.event_failed(action, out=str(e))
        d.service.logger.error(f'interval loop error: {traceback.format_exc()}')


def handle_rpc(event):
    d.service.need_ready()
    if event.method == b'action':
        action = Action(event)
        d.controller.event_pending(action)
        threading.Thread(target=run_action, daemon=True, args=(action,)).start()
    else:
        sdk.no_rpc_method()


def update(dps):
    for (dps, value) in dps.items():
        oid = d.map.get(dps)
        if oid is None:
            continue
        if value is True:
            value = 1
        elif value is False:
            value = 0
        payload = pack({'status': 1, 'value': value})
        topic = RAW_STATE_TOPIC + oid.to_path()
        d.service.bus.send(
            topic, busrt.client.Frame(payload,
                                      tp=busrt.client.OP_PUBLISH,
                                      qos=0))


def pull(interval):
    d.service.wait_core()
    while d.service.is_active():
        start = time.perf_counter()
        try:
            with d.action_lock:
                dps = d.device.status()['dps']
            update(dps)
        except Exception:
            mark_all_items_failed()
            d.service.logger.error(
                f'interval loop error: {traceback.format_exc()}')
        elapsed = time.perf_counter() - start
        to_sleep = interval - elapsed
        if to_sleep <= 0:
            d.service.logger.warning('interval loop timeout')
        else:
            time.sleep(to_sleep)


def mark_all_items_failed():
    payload = pack({'status': sdk.ITEM_STATUS_ERROR})
    for oid in d.map.values():
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

    d.map = {k: OID(v) for k, v in config.get('map', {}).items()}

    service.init(info, on_rpc_call=handle_rpc)

    if service.is_mode_rtf():
        mark_all_items_failed()
        return

    dev_id = config['device']['id']
    address = config['device']['addr']
    local_key = config['device']['local_key']
    api_version = config['device']['version']

    d.action_map = {OID(k): v for k, v in config.get('action_map', {}).items()}

    d.device = tinytuya.Device(dev_id=dev_id,
                               address=address,
                               local_key=local_key,
                               version=api_version,
                               connection_timeout=service.timeout['default'])

    d.controller = Controller(service.bus)

    threading.Thread(target=pull, daemon=True,
                     args=(config['pull_interval'],)).start()
    service.block()


run()

__version__ = '0.0.25'

import evaics.sdk as sdk
from types import SimpleNamespace
import psrt
import threading
import time
import os
import uuid
import zlib
import hashlib
import pyaltt2.crypto
import busrt
try:
    import rapidjson as json
except:
    import json

from functools import partial
from cachetools import TTLCache

from evaics.sdk import pack, unpack

encrypt = partial(pyaltt2.crypto.encrypt, hmac_key=True, b64=False)
decrypt = partial(pyaltt2.crypto.decrypt, hmac_key=True, b64=False)

key_cache = TTLCache(ttl=5, maxsize=4096)

api_callback = {}

_d = SimpleNamespace(test_passed=threading.Event(), lc={})


def get_key_val(key_id):
    try:
        return key_cache[key_id]
    except KeyError:
        key = unpack(
            _d.service.rpc.call(
                _d.key_svc, busrt.rpc.Request('key.get', pack(
                    {'i': key_id}))).wait_completed().get_payload())['key']
        key_cache[key_id] = key
        return key


def mark_node(node, status, info=None):
    payload = {'status': status}
    if info is not None:
        payload['info'] = info
    _d.service.bus.send(
        f'RPL/NODE/{node}',
        busrt.client.Frame(pack(payload), tp=busrt.client.OP_PUBLISH))


def jreq(id, method, params=None):
    d = {'jsonrpc': '2.0', 'method': method, 'id': id}
    if params is not None:
        d['params'] = params
    return d


class Callback:

    def __init__(self):
        self.completed = threading.Event()
        self.body = ''
        self.code = None
        self.proto = None

    def data_handler(self, data):
        self.completed.set()
        if data is not None:
            try:
                if data[0] != 0 and data[0] != 1:
                    raise ValueError
                self.code = data[1]
                self.body = data[2:]
            except:
                self.code = 500


def send_api_request(request_id, controller_id, data, callback):
    try:
        if request_id in api_callback:
            logger.error('duplicate API request ID')
            return False
        t = f'controller/{controller_id}/api/response/{request_id}'
        api_callback[request_id] = (t, callback)
        _d.pubsub.subscribe(t)
        return _d.pubsub.publish(f'controller/{controller_id}/api/request',
                                 data)
    except:
        sdk.log_traceback()


def finish_api_request(request_id):
    try:
        if request_id not in api_callback:
            logger.warning('API request ID not found')
            return False
        t = api_callback[request_id][0]
        del api_callback[request_id]
        _d.pubsub.unsubscribe(t)
    except:
        sdk.log_traceback()


class LegacyNode:

    def __init__(self, id, key_id, key_lid, reload_interval, ping_interval,
                 timeout, controllers):
        self.id = id
        for c in controllers:
            if c not in ['uc', 'lm']:
                raise ValueError(f'invalid controller type: {c}')
        self.controllers = controllers
        self.online = False
        self.reload_interval = reload_interval
        self.ping_interval = ping_interval
        self._last_reload = 0
        self.key_id = key_id
        self.key_lid = key_lid
        self.timeout = timeout
        self.need_reload = threading.Event()
        self.need_ping = threading.Event()
        self.lock = threading.RLock()
        self.build = None
        self.version = None

    def call(self, ctype, method, params=None):
        key_val = get_key_val(self.key_id)
        if params is None:
            params = {'k': key_val}
        else:
            params['k'] = key_val
        rid = uuid.uuid4().bytes
        request_id = rid.hex()
        payload = jreq(request_id, method, params)
        controller_id = f'{ctype}/{self.id}'
        private_key = hashlib.sha512(str(key_val).encode()).digest()
        rid = uuid.uuid4().bytes
        request_id = rid.hex()
        header = b'\x00\x03'
        data = rid + zlib.compress(pack(payload))
        cb = Callback()
        send_api_request(
            request_id, controller_id, header + self.key_lid.encode() +
            b'\x00' + encrypt(data, private_key, key_is_hash=True),
            cb.data_handler)
        if not cb.completed.wait(self.timeout):
            finish_api_request(request_id)
            raise TimeoutError
        if cb.code:
            if cb.code != 200:
                raise RuntimeError(
                    f'{controller_id} response error code: {cb.code}')
            content = unpack(
                zlib.decompress(decrypt(cb.body, private_key,
                                        key_is_hash=True)))
            try:
                error = content['error']
                code = error.get('code', None)
                msg = error.get('message', '')
                raise RuntimeError(
                    f'{controller_id} API call error: {msg} ({code})')
            except KeyError:
                pass
            return content.get('result')
        else:
            raise RuntimeError(f'{controller_id} no response')

    def serialize(self):
        d = {}
        d['id'] = self.id
        d['controllers'] = self.controllers
        d['online'] = self.online
        d['key_id'] = self.key_id
        d['key_legacy_id'] = self.key_lid
        d['reload_interval'] = self.reload_interval
        d['timeout'] = self.timeout
        d['build'] = self.build
        d['version'] = self.version
        return d

    def force_reload(self):
        self._last_reload = 0
        self.need_reload.set()

    def start(self):
        logger.info(f'legacy v3 replication for the node {self.id} active')
        threading.Thread(target=self._t_reloader, daemon=True).start()
        threading.Thread(target=self._t_reload_scheduler, daemon=True).start()
        if self.ping_interval:
            threading.Thread(target=self._t_pinger, daemon=True).start()
            threading.Thread(target=self._t_ping_scheduler, daemon=True).start()

    def _t_reloader(self):
        while True:
            self.need_reload.wait()
            self.need_reload.clear()
            if (time.perf_counter() -
                    self._last_reload) > self.reload_interval / 2:
                self._last_reload = time.perf_counter()
                try:
                    with self.lock:
                        self.reload()
                except:
                    sdk.log_traceback()
            else:
                logger.warning(
                    f'{self.id} reload triggered too often, skipping')

    def _t_pinger(self):
        while True:
            self.need_ping.wait()
            self.need_ping.clear()
            try:
                with self.lock:
                    self.ping()
            except:
                sdk.log_traceback()

    def _t_reload_scheduler(self):
        next_reload = time.perf_counter() + self.reload_interval
        while True:
            self.need_reload.set()
            to_sleep = next_reload - time.perf_counter()
            next_reload += self.reload_interval
            if to_sleep > 0:
                time.sleep(to_sleep)

    def _t_ping_scheduler(self):
        next_ping = time.perf_counter() + self.ping_interval
        while True:
            self.need_ping.set()
            to_sleep = next_ping - time.perf_counter()
            next_ping += self.ping_interval
            if to_sleep > 0:
                time.sleep(to_sleep)

    def ping(self):
        try:
            if not self.controllers:
                return
            for i, controller in enumerate(self.controllers):
                result = self.call(controller, 'test')
                if i == 0:
                    self.build = result['product_build']
                    self.version = result['version']
            logger.warning(f'{self.id} ping passed')
            mark_node(self.id,
                      'online',
                      info={
                          'build': self.build,
                          'version': self.version
                      })
        except:
            sdk.log_traceback()
            self.online = False
            mark_node(self.id, 'offline')

    def reload(self):
        try:
            if not self.controllers:
                return
            result = self.call(self.controllers[0], 'test')
            self.build = result['product_build']
            self.version = result['version']
            inventory = []
            if 'uc' in self.controllers:
                units = self.call('uc', 'state', {'p': 'U', 'full': True})
                for unit in units:
                    meta = {}
                    v4unit = {
                        'oid':
                            unit['oid'],
                        'status':
                            unit['status'],
                        'value':
                            unit['value'],
                        'ieid':
                            unit['ieid'],
                        't':
                            unit['set_time'],
                        'enabled':
                            unit['action_enabled'],
                        'meta':
                            meta,
                        'act':
                            0 if unit['status'] == unit['nstatus'] and
                            unit['value'] == unit['nvalue'] else 1
                    }
                    for f in [
                            'description', 'location', 'loc_x', 'loc_y', 'loc_z'
                    ]:
                        meta[f] = unit.get(f)
                    inventory.append(v4unit)
                sensors = self.call('uc', 'state', {'p': 'S', 'full': True})
                for sensor in sensors:
                    meta = {}
                    v4sensor = {
                        'oid': sensor['oid'],
                        'status': sensor['status'],
                        'value': sensor['value'],
                        'ieid': sensor['ieid'],
                        't': sensor['set_time'],
                        'enabled': sensor['status'] != 0,
                        'meta': meta
                    }
                    for f in [
                            'description', 'location', 'loc_x', 'loc_y', 'loc_z'
                    ]:
                        meta[f] = sensor.get(f)
                    inventory.append(v4sensor)
            if 'lm' in self.controllers:
                lvars = self.call('lm', 'state', {'p': 'LV', 'full': True})
                for lvar in lvars:
                    meta = {}
                    v4lvar = {
                        'oid': lvar['oid'],
                        'status': lvar['status'],
                        'value': lvar['value'],
                        'ieid': lvar['ieid'],
                        't': lvar['set_time'],
                        'enabled': True,
                        'meta': meta
                    }
                    for f in ['description', 'expires']:
                        meta[f] = lvar.get(f)
                    inventory.append(v4lvar)
                lmacros = self.call('lm', 'list_macros')
                for lmacro in lmacros:
                    v4lmacro = {
                        'oid': lmacro['oid'],
                        'enabled': lmacro['action_enabled'],
                        'meta': {
                            'description': lmacro.get('description')
                        }
                    }
                    inventory.append(v4lmacro)
            _d.service.bus.send(
                f'RPL/INVENTORY/{self.id}',
                busrt.client.Frame(pack(inventory), tp=busrt.client.OP_PUBLISH))
            self.online = True
            mark_node(self.id,
                      'online',
                      info={
                          'build': self.build,
                          'version': self.version
                      })
        except:
            sdk.log_traceback()
            self.online = False
            mark_node(self.id, 'offline')


def process_message(client, userdata, message):
    topic = message.topic
    if topic == _d.test_topic and message.payload == b'passed':
        logger.debug('PING test passed')
        _d.test_passed.set()
    elif topic in _d.bulks:
        if message.payload.startswith(b'\x00'):
            payload = unpack(zlib.decompress(message.payload[2:]))
        else:
            payload = json.loads(message.payload.decode())
        node_id = payload['c'].split('/')[1]
        for state in payload['d']:
            v4state = {
                'status': state['status'],
                'value': state['value'],
                'ieid': state['ieid'],
                't': state['set_time'],
                'node': node_id
            }
            if 'nstatus' in state:
                v4state['act'] = 0 if state['status'] == state[
                    'nstatus'] and state['value'] == state['nvalue'] else 1
            _d.service.bus.send(
                f'RPL/ST/{state["oid"].replace(":", "/",1)}',
                busrt.client.Frame(pack(v4state), tp=busrt.client.OP_PUBLISH))
    elif topic == 'controller/discovery':
        node_id = message.payload.decode().split('/')[1]
        try:
            node = _d.lc[node_id]
            if not node.online:
                logger.info(f'{node_id} is back online, triggering reload')
                node.need_reload.set()
        except KeyError:
            pass
    elif topic.startswith('controller/'):
        response_id = topic.rsplit('/', maxsplit=1)[-1]
        if response_id in api_callback:
            api_callback[response_id][1](message.payload)
            finish_api_request(response_id)


ACTION_STATUS_V4 = {
    'created': 'created',
    'pending': 'pending',
    'queued': 'pending',
    'refused': 'canceled',
    'dead': 'failed',
    'canceled': 'canceled',
    'ignored': 'canceled',
    'running': 'running',
    'failed': 'failed',
    'terminated': 'terminated',
    'completed': 'completed'
}


def handle_rpc(event):
    method = event.method.decode()
    if method == 'node.list':
        if event.get_payload():
            raise busrt.rpc.RpcException(code=sdk.ERR_CODE_INVALID_PARAMS)
        return pack(
            sorted([c.serialize() for _, c in _d.lc.items()],
                   key=lambda k: k['id']))
    elif method == 'node.reload':
        try:
            params = unpack(event.get_payload())
            node_id = params['i']
        except Exception as e:
            raise busrt.rpc.RpcException(str(e), sdk.ERR_CODE_INVALID_PARAMS)
        try:
            node = _d.lc[node_id]
            node.force_reload()
        except KeyError:
            raise busrt.rpc.RpcException(f'node {node_id} not found',
                                         sdk.ERR_CODE_NOT_FOUND)
    # end of user method, the next methods are proxied to v3 nodes
    elif method.startswith('lvar.'):
        try:
            params = unpack(event.get_payload())
            oid = params['i']
            node_id = params['node']
            p = {'i': oid}
            if 'status' in params:
                p['s'] = params['status']
            if 'value' in params:
                p['v'] = params['value']
                if p['v'] is None:
                    p['v'] = ''
            rm = method[5:]
            if rm == 'incr':
                rm = 'increment'
            elif rm == 'decr':
                rm = 'decrement'
        except Exception as e:
            raise busrt.rpc.RpcException(str(e), sdk.ERR_CODE_INVALID_PARAMS)
        node = _d.lc[node_id]
        node.call('lm', rm, p)
        return pack(None)
    elif method == 'action':
        try:
            params = unpack(event.get_payload())
            node_id = params['node']
            i = params['i']
            priority = params['priority']
            ap = params['params']
            status = ap['status']
            p = {'i': i, 'p': priority, 's': status}
            if 'value' in ap:
                p['v'] = ap['value']
                if p['v'] is None:
                    p['v'] = ''
            if 'wait' in params:
                p['w'] = params['wait']
        except Exception as e:
            raise busrt.rpc.RpcException(str(e), sdk.ERR_CODE_INVALID_PARAMS)
        node = _d.lc[node_id]
        act_info = node.call('uc', 'action', p)
        return pack(to_v4action(act_info, node_id))
    elif method == 'action.result':
        try:
            params = unpack(event.get_payload())
            node_id = params['node']
            i = params['i']
            u = uuid.UUID(bytes=params['u'])
            ctp = 'lm' if i.startswith('lmacro:') else 'uc'
        except Exception as e:
            raise busrt.rpc.RpcException(str(e), sdk.ERR_CODE_INVALID_PARAMS)
        node = _d.lc[node_id]
        act_info = node.call(ctp, 'result', {'u': str(u)})
        return pack(to_v4action(act_info, node_id))
    elif method == 'run':
        try:
            params = unpack(event.get_payload())
            node_id = params['node']
            i = params['i']
            priority = params['priority']
            ap = params['params']
            p = {'i': i, 'p': priority}
            if 'args' in ap:
                p['a'] = ap['args']
            if 'kwargs' in ap:
                p['kw'] = ap['kwargs']
            if 'wait' in params:
                p['w'] = params['wait']
        except Exception as e:
            raise busrt.rpc.RpcException(str(e), sdk.ERR_CODE_INVALID_PARAMS)
        node = _d.lc[node_id]
        act_info = node.call('lm', 'run', p)
        return pack(to_v4action(act_info, node_id))
    else:
        sdk.no_rpc_method()


def to_v4action(res, node_id):
    ap = {}
    st = res.get('nstatus')
    if st is not None:
        ap['status'] = st
    val = res.get('nvalue')
    if val is not None:
        ap['value'] = val
    args = res.get('args')
    if args is not None:
        ap['args'] = args
    kwargs = res.get('kwargs')
    if kwargs is not None:
        ap['kwargs'] = kwargs
    times = res['time']
    v4times = {}
    for t, v in times.items():
        v4times[ACTION_STATUS_V4.get(t, 'pending')] = v
    v4act = {
        'uuid': uuid.UUID(res['uuid']).bytes,
        'oid': res['item_oid'],
        'priority': res['priority'],
        'status': ACTION_STATUS_V4.get(res['status'], 'pending'),
        'time': v4times,
        'params': ap,
        'out': res['out'],
        'err': res['err'],
        'exitcode': res.get('exitcode'),
        'node': node_id,
        'finished': res.get('finished')
    }
    return v4act


def watch_pubsub():
    while True:
        if not _d.pubsub.is_connected():
            _d.service.mark_terminating()
            break
        time.sleep(0.2)


def ping_pubsub(topic):
    while True:
        _d.test_passed.clear()
        _d.pubsub.publish(topic, 'passed')
        if not _d.test_passed.wait(_d.pubsub.timeout):
            logger.error('PING test timeout')
            _d.service.mark_terminating()
            break
        time.sleep(_d.pubsub.timeout / 2)


def run():
    global logger
    info = sdk.ServiceInfo(author='Bohemia Automation',
                           description='legacy v3 replication service',
                           version=__version__)
    info.add_method('node.list')
    info.add_method('node.reload', required=['i'])
    service = sdk.Service()
    _d.service = service
    config = service.get_config()
    pubsub_cfg = config['pubsub']
    service.init_bus()
    service.drop_privileges()
    logger = service.init_logs()
    if pubsub_cfg['proto'] != 'psrt':
        raise RuntimeError('only PSRT proto is supported')
    del pubsub_cfg['proto']
    if 'user' in pubsub_cfg and pubsub_cfg['user'] is None:
        del pubsub_cfg['user']
    if 'password' in pubsub_cfg and pubsub_cfg['password'] is None:
        del pubsub_cfg['password']
    for id, c in config.get('nodes', {}).items():
        key_id = c['key_id']
        key_lid = c['key_legacy_id']
        reload_interval = c['reload_interval']
        ping_interval = c.get('ping_interval')
        timeout = c.get('timeout', service.timeout['default'])
        controllers = c.get('controllers', [])
        c = LegacyNode(id, key_id, key_lid, reload_interval, ping_interval,
                       timeout, controllers)
        _d.lc[id] = c
        mark_node(c.id, 'offline')
    pubsub = psrt.Client(**pubsub_cfg)
    pubsub.on_message = process_message
    pubsub.connect()
    test_topic = f'v4/tests/{service.system_name}/{service.id}/test'
    pubsub.subscribe(test_topic)
    _d.pubsub = pubsub
    _d.test_topic = test_topic
    _d.key_svc = config['key_svc']
    service.on_rpc_call = handle_rpc
    service.init_rpc(info)
    pubsub.subscribe('controller/discovery')
    _d.bulks = [f'state/{t}' for t in pubsub_cfg.get('bulk_subscribe', [])]
    pubsub.subscribe_bulk(_d.bulks)
    service.register_signals()
    service.mark_ready()
    logger.info('legacy replication service started')
    service.wait_core()
    for _, c in _d.lc.items():
        c.start()
    threading.Thread(target=watch_pubsub, daemon=True).start()
    threading.Thread(target=ping_pubsub, args=(test_topic,),
                     daemon=True).start()
    service.block()
    service.mark_terminating()
    for _, c in _d.lc.items():
        mark_node(c.id, 'removed')
    pubsub.bye()

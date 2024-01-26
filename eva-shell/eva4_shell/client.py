import logging

from .sharedobj import common, current_command
from .tools import debug
from neotermcolor import colored

import busrt
from evaics.sdk import pack, unpack

logger = logging.getLogger('busrt.client')
logger.setLevel(logging.CRITICAL)

DEFAULT_DB_SERVICE = 'eva.db.default'
DEFAULT_REPL_SERVICE = 'eva.repl.default'
DEFAULT_ACL_SERVICE = 'eva.aaa.acl'
DEFAULT_AUTH_SERVICE = 'eva.aaa.localauth'
DEFAULT_KIOSK_SERVICE = 'eva.kioskman.default'
DEFAULT_GENERATOR_SERVICE = 'eva.generator.default'
DEFAULT_ACCOUNTING_SERVICE = 'eva.aaa.accounting'


def print_trace_msg(msg):
    level = msg['l']
    print('[', end='')
    if level == 0:
        print(colored('TRACE', color='grey'), end='')
    elif level <= 10:
        print(colored('DEBUG', color='grey', attrs='bold'), end='')
    elif level <= 20:
        print('INFO', end='')
    elif level <= 30:
        print(colored('WARN', color='yellow'), end='')
    else:
        print(colored('ERROR', color='red'), end='')
    print('] ', end='')
    print(msg.get('msg', ''))


def on_frame(frame):
    if frame.topic and frame.topic.startswith('LOG/TR/'):
        msg = unpack(frame.payload)
        print_trace_msg(msg)


def connect():
    name = f'{common.bus_name}.{common.bus_conn_no}'
    if current_command.debug:
        debug(f'calling bus cli {common.bus_path} as {name}')
    if common.bus is None or not common.bus.is_connected():
        common.bus = busrt.client.Client(common.bus_path, name)
        common.bus.connect()
        common.rpc = busrt.rpc.Rpc(common.bus)
        common.rpc.on_frame = on_frame


def call_rpc(method, params=None, target='eva.core', trace=False):
    common.bus_conn_no += 1
    connect()
    if current_command.debug:
        debug(f'target: {target}')
        debug(f'method: {method}')
        debug(f'params: {params}')
    payload = b''
    if trace:
        import uuid
        trace_id = uuid.uuid4()
        xp = pack({'call_trace_id': trace_id.bytes})
        payload += b'\xc1\xc1' + len(xp).to_bytes(2, 'little') + xp
    else:
        trace_id = None
    if params is not None:
        payload += pack(params)
    if trace_id:
        common.rpc.client.subscribe(f'LOG/TR/{trace_id}').wait_completed(
            timeout=current_command.timeout)
    try:
        result = common.rpc.call(
            target, busrt.rpc.Request(
                method,
                params=payload)).wait_completed(timeout=current_command.timeout)
    finally:
        if trace_id:
            common.rpc.client.unsubscribe(f'LOG/TR/{trace_id}').wait_completed(
                timeout=current_command.timeout)
            print()
    if result.is_empty():
        return None
    else:
        return unpack(result.get_payload())

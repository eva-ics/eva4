import logging

from .sharedobj import common, current_command
from .tools import debug

import busrt
from evaics.sdk import pack, unpack

logger = logging.getLogger('busrt.client')
logger.setLevel(logging.CRITICAL)

DEFAULT_DB_SERVICE = 'eva.db.default'
DEFAULT_REPL_SERVICE = 'eva.repl.default'
DEFAULT_ACL_SERVICE = 'eva.aaa.acl'
DEFAULT_AUTH_SERVICE = 'eva.aaa.localauth'
DEFAULT_KIOSK_SERVICE = 'eva.kioskman.default'


def connect():
    name = f'{common.bus_name}.{common.bus_conn_no}'
    if current_command.debug:
        debug(f'calling bus cli {common.bus_path} as {name}')
    if common.bus is None or not common.bus.is_connected():
        common.bus = busrt.client.Client(common.bus_path, name)
        common.bus.connect()
        common.rpc = busrt.rpc.Rpc(common.bus)


def call_rpc(method, params=None, target='eva.core'):
    common.bus_conn_no += 1
    connect()
    if current_command.debug:
        debug(f'target: {target}')
        debug(f'method: {method}')
        debug(f'params: {params}')
    result = common.rpc.call(
        target,
        busrt.rpc.Request(
            method,
            params=b'' if params is None else pack(params))).wait_completed(
                timeout=current_command.timeout)
    if result.is_empty():
        return None
    else:
        return unpack(result.get_payload())

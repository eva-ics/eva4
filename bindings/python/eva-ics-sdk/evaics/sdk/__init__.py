__version__ = '0.0.36'

import busrt
import sys
import msgpack
import logging
import time
import signal
import threading
import uuid
import pwd
import grp
import os

from ..exceptions import InvalidParameter
from ..exceptions import FunctionFailed
from ..exceptions import ResourceNotFound
from ..exceptions import ResourceBusy
from ..exceptions import ResourceAlreadyExists
from ..exceptions import AccessDenied
from ..exceptions import MethodNotImplemented
from ..exceptions import TimeoutException

from functools import partial

SERVICE_PAYLOAD_PING = 0
SERVICE_PAYLOAD_INITIAL = 1

ACTION_CREATED = 0b0000_0000
ACTION_ACCEPTED = 0b0000_0001
ACTION_PENDING = 0b0000_0010
ACTION_RUNNING = 0b0000_1000
ACTION_COMPLETED = 0b0000_1111
ACTION_FAILED = 0b1000_0000
ACTION_CANCELED = 0b1000_0001
ACTION_TERMINATED = 0b1000_0010

ITEM_STATUS_ERROR = -1

ERR_CODE_NOT_FOUND = -32001
ERR_CODE_ACCESS_DENIED = -32002
ERR_CODE_SYSTEM_ERROR = -32003
ERR_CODE_OTHER = -32004
ERR_CODE_NOT_READY = -32005
ERR_CODE_UNSUPPORTED = -32006
ERR_CODE_CORE_ERROR = -32007
ERR_CODE_TIMEOUT = -32008
ERR_CODE_INVALID_DATA = -32009
ERR_CODE_FUNC_FAILED = -32010
ERR_CODE_ABORTED = -32011
ERR_CODE_ALREADY_EXISTS = -32012
ERR_CODE_BUSY = -32013
ERR_CODE_METHOD_NOT_IMPLEMENTED = -32014
ERR_CODE_TOKEN_RESTRICTED = -32015
ERR_CODE_IO = -32016
ERR_CODE_REGISTRY = -32017
ERR_CODE_EVAHI_AUTH_REQUIRED = -32018

ERR_CODE_PARSE = -32700
ERR_CODE_INVALID_REQUEST = -32600
ERR_CODE_METHOD_NOT_FOUND = -32601
ERR_CODE_INVALID_PARAMS = -32602
ERR_CODE_INTERNAL_RPC = -32603

ERR_CODE_BUS_CLIENT_NOT_REGISTERED = -32113
ERR_CODE_BUS_DATA = -32114
ERR_CODE_BUS_IO = -32115
ERR_CODE_BUS_OTHER = -32116
ERR_CODE_BUS_NOT_SUPPORTED = -32117
ERR_CODE_BUS_BUSY = -32118
ERR_CODE_BUS_NOT_DELIVERED = -32119
ERR_CODE_BUS_TIMEOUT = -32120

pack = msgpack.dumps
unpack = partial(msgpack.loads, raw=False)


def rpc_e2e(e):
    code = e.rpc_error_code
    msg = e.rpc_error_payload
    if isinstance(msg, bytes):
        msg = msg.decode()
    if code == ERR_CODE_INVALID_PARAMS:
        return InvalidParameter(msg)
    elif code == ERR_CODE_NOT_FOUND:
        return ResourceNotFound(msg)
    elif code == ERR_CODE_BUSY:
        return ResourceBusy(msg)
    elif code == ERR_CODE_ALREADY_EXISTS:
        return ResourceAlreadyExists(msg)
    elif code == ERR_CODE_ACCESS_DENIED:
        return AccessDenied(msg)
    elif code == ERR_CODE_METHOD_NOT_IMPLEMENTED:
        return MethodNotImplemented(msg)
    elif code == ERR_CODE_TIMEOUT:
        return TimeoutException(msg)
    else:
        return FunctionFailed(msg)


class LocalProxy(threading.local):
    """
    Simple proxy for threading.local namespace
    """

    def get(self, attr, default=None):
        """
        Get thread-local attribute

        Args:
            attr: attribute name
            default: default value if attribute is not set

        Returns:
            attribute value or default value
        """
        return getattr(self, attr, default)

    def has(self, attr):
        """
        Check if thread-local attribute exists

        Args:
            attr: attribute name

        Returns:
            True if attribute exists, False if not
        """
        return hasattr(self, attr)

    def set(self, attr, value):
        """
        Set thread-local attribute

        Args:
            attr: attribute name
            value: attribute value to set
        """
        return setattr(self, attr, value)

    def clear(self, attr):
        """
        Clear (delete) thread-local attribute

        Args:
            attr: attribute name
        """
        return delattr(self, attr) if hasattr(self, attr) else True


def _action_event_payload(u, status, out=None, err=None, exitcode=None):
    payload = {
        'uuid': u.bytes,
        'status': status,
    }
    if out is not None:
        payload['out'] = out
    if err is not None:
        payload['err'] = err
    if exitcode is not None:
        payload['exitcode'] = exitcode
    return pack(payload)


class OID:

    def __init__(self, s):
        self.kind, self.full_id = s.split(':', maxsplit=1)
        self.oid = s
        self.id = s.rsplit('/', 1)[-1] if '/' in s else self.full_id

    def __str__(self):
        return self.oid

    def to_path(self):
        return f'{self.kind}/{self.full_id}'


class ServiceInfo:

    def __init__(self, author='', description='', version=''):
        self.author = author
        self.description = description
        self.version = version
        self.methods = {}

    def add_method(self, method, description='', required=[], optional=[]):
        params = {}
        for p in required:
            params[p] = {'required': True}
        for p in optional:
            params[p] = {'required': False}
        m = {'description': description, 'params': params}
        self.methods[method] = m

    def serialize(self):
        d = {
            'author': self.author,
            'description': self.description,
            'version': self.version
        }
        if self.methods:
            d['methods'] = self.methods
        return d


class Action:

    def __init__(self, event):
        a = unpack(event.get_payload())
        self.uuid = uuid.UUID(bytes=a['uuid'])
        self.i = OID(a['i'])
        self.timeout = a['timeout']
        self.priority = a['priority']
        self.params = a['params']


class Controller:

    def __init__(self, bus):
        self.bus = bus

    def event_pending(self, action):
        self.bus.send(
            f'ACT/{action.i.to_path()}',
            busrt.client.Frame(payload=_action_event_payload(
                action.uuid, ACTION_PENDING),
                               tp=busrt.client.OP_PUBLISH,
                               qos=0))

    def event_running(self, action):
        self.bus.send(
            f'ACT/{action.i.to_path()}',
            busrt.client.Frame(payload=_action_event_payload(
                action.uuid, ACTION_RUNNING),
                               tp=busrt.client.OP_PUBLISH,
                               qos=0))

    def event_completed(self, action, out=None):
        self.bus.send(
            f'ACT/{action.i.to_path()}',
            busrt.client.Frame(payload=_action_event_payload(action.uuid,
                                                             ACTION_COMPLETED,
                                                             out=out,
                                                             exitcode=0),
                               tp=busrt.client.OP_PUBLISH,
                               qos=0))

    def event_failed(self, action, out=None, err=None, exitcode=None):
        self.bus.send(
            f'ACT/{action.i.to_path()}',
            busrt.client.Frame(payload=_action_event_payload(action.uuid,
                                                             ACTION_FAILED,
                                                             out=out,
                                                             err=err,
                                                             exitcode=exitcode),
                               tp=busrt.client.OP_PUBLISH,
                               qos=0))

    def event_canceled(self, action):
        self.bus.send(
            f'ACT/{action.i.to_path()}',
            busrt.client.Frame(payload=_action_event_payload(
                action.uuid, ACTION_CANCELED),
                               tp=busrt.client.OP_PUBLISH,
                               qos=0))

    def event_terminated(self, action):
        self.bus.send(
            f'ACT/{action.i.to_path()}',
            busrt.client.Frame(payload=_action_event_payload(
                action.uuid, ACTION_TERMINATED),
                               tp=busrt.client.OP_PUBLISH,
                               qos=0))


class EvaLogHandler(logging.Handler):

    def __init__(self, bus):
        self.bus = bus
        super().__init__()

    def emit(self, record):
        msg = self.format(record)
        if record.levelno == 1:
            topic = 'LOG/IN/trace'
        elif record.levelno <= 10:
            topic = 'LOG/IN/debug'
        elif record.levelno <= 20:
            topic = 'LOG/IN/info'
        elif record.levelno <= 30:
            topic = 'LOG/IN/warn'
        elif record.levelno <= 40:
            topic = 'LOG/IN/error'
        else:
            topic = 'LOG/IN/error'
            msg = f'CRITICAL: {msg}'
        self.bus.send(
            topic,
            busrt.client.Frame(msg.encode(), tp=busrt.client.OP_PUBLISH, qos=0))


class Service:

    def __init__(self):
        self.active = True
        stdin = sys.stdin.buffer
        payload_code = stdin.read(1)
        if payload_code[0] != SERVICE_PAYLOAD_INITIAL:
            raise RuntimeError('invalid payload')
        payload_len = int.from_bytes(stdin.read(4), 'little', signed=False)
        self.initial = unpack(stdin.read(payload_len))
        self.id = self.initial['id']
        self.system_name = self.initial['system_name']
        self.sleep_step = 0.1
        self.timeout = self.initial['timeout']
        self.log_format = ('mod:%(module)s fn:%(funcName)s '
                           'th:%(threadName)s :: %(message)s')
        self.log_formatter = logging.Formatter(self.log_format)
        self.on_rpc_call = busrt.rpc.on_call_default
        self.shutdown_requested = False
        self.eva_dir = self.initial['core']['path']
        user = self.initial.get('user')
        if user != 'nobody':
            self.data_path = self.initial['data_path']
        else:
            self.data_path = None
        os.environ['EVA_SYSTEM_NAME'] = self.system_name
        os.environ['EVA_DIR'] = self.eva_dir
        os.environ['EVA_SVC_ID'] = self.id
        os.environ[
            'EVA_SVC_DATA_PATH'] = '' if self.data_path is None \
                    else self.data_path
        os.environ['EVA_TIMEOUT'] = str(self.timeout['default'])
        if self.initial.get('fail_mode'):
            if not self.initial.get('react_to_fail'):
                raise RuntimeError('the service is started in react-to-fail'
                                   ' mode, but rtf disabled for the service')
        else:
            prepare_command = self.initial.get('prepare_command')
            if prepare_command is not None:
                code = os.system(prepare_command)
                if code:
                    raise RuntimeError(
                        f'prepare command exited with code {code}')
        threading.Thread(target=self._t_stdin_watcher, daemon=True).start()

    def _t_stdin_watcher(self):
        f = open(sys.stdin.fileno(), 'rb')
        while self.active:
            try:
                buf = f.read(1)
            except:
                self.mark_terminating()
                break
            if buf[0] != SERVICE_PAYLOAD_PING:
                self.mark_terminating()
                break
            time.sleep(0.1)

    def is_mode_rtf(self):
        return self.initial.get('fail_mode', False)

    def is_mode_normal(self):
        return not self.initial.get('fail_mode', False)

    def need_ready(self):
        if not self.active:
            raise busrt.rpc.RpcException('service not ready',
                                         ERR_CODE_INTERNAL_RPC)

    def get_config(self):
        config = self.initial.get('config')
        if config is None:
            return {}
        else:
            return config

    def init_bus(self):
        bus_config = self.initial['bus']
        if bus_config['type'] != 'native':
            raise ValueError(f'bus {bus_config["type"]} is not supported')
        self.bus = busrt.client.Client(bus_config['path'], self.id)
        self.bus.buf_size = bus_config['buf_size']
        self.bus.timeout = bus_config['timeout']
        self.bus.ping_interval = bus_config['ping_interval']
        self.bus.connect()

    def init_rpc(self, svc_info):
        self._svc_info = pack(svc_info.serialize())
        self.rpc = busrt.rpc.Rpc(self.bus)
        self.rpc.on_call = self._handle_rpc_call

    def wait_core(self, timeout=None, wait_forever=True):
        if timeout is None:
            timeout = self.timeout.get('startup', self.timeout['default'])
        wait_until = time.perf_counter() + timeout
        while True:
            try:
                result = self.rpc.call(
                    'eva.core',
                    busrt.rpc.Request('test')).wait_completed(timeout)
                if unpack(result.get_payload())['active'] is True:
                    return
            except:
                pass
            if not wait_forever and wait_until >= time.perf_counter():
                raise TimeoutError
            time.sleep(self.sleep_step)

    def _handle_rpc_call(self, event):
        method = event.method
        if method == b'test':
            return
        elif method == b'info':
            return self._svc_info
        elif method == b'stop':
            self.active = False
            return
        else:
            return self.on_rpc_call(event)

    def init_logs(self):
        level = self.initial['core']['log_level']
        logging.basicConfig(level=level)
        logger = logging.getLogger()
        logger.setLevel(level)
        while logger.hasHandlers():
            logger.removeHandler(logger.handlers[0])
        self.log_handler = EvaLogHandler(self.bus)
        self.log_handler.setLevel(level=level)
        self.log_handler.setFormatter(self.log_formatter)
        logger.addHandler(self.log_handler)

        def log_trace(msg, *args, **kwargs):
            logger.log(1, msg, *args, **kwargs)

        logger.trace = log_trace
        return logger

    def block(self):
        sleep_step = self.sleep_step
        while self.active and self.bus.is_connected():
            time.sleep(sleep_step)

    def mark_ready(self):
        self._mark('ready')

    def mark_terminating(self):
        self.active = False
        self.shutdown_requested = True
        self._mark('terminating')

    def drop_privileges(self):
        user = self.initial.get('user')
        if user is not None:
            u = pwd.getpwnam(user)
            groups = os.getgrouplist(user, u.pw_gid)
            os.setgroups(groups)
            os.setgid(u.pw_gid)
            os.setuid(u.pw_uid)

    def _mark(self, status):
        self.bus.send(
            'SVC/ST',
            busrt.client.Frame(pack({'status': status}),
                               tp=busrt.client.OP_PUBLISH,
                               qos=0))

    def register_signals(self):
        signal.signal(signal.SIGINT, self._term_handler)
        signal.signal(signal.SIGTERM, self._term_handler)

    def _term_handler(self, sig, frame):
        self.active = False

    def is_active(self):
        return self.active

    def is_shutdown_requested(self):
        return self.shutdown_requested


def no_rpc_method():
    raise busrt.rpc.RpcException('no such method', ERR_CODE_METHOD_NOT_FOUND)


def log_traceback():
    import traceback
    print(traceback.format_exc(), flush=True, file=sys.stderr)

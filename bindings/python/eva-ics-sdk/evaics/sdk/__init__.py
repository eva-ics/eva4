__version__ = '0.2.25'

import busrt
import sys
import msgpack
import logging
import time
import signal
import threading
import uuid
try:
    import pwd
except:
    pwd = None
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
ERR_CODE_ACCESS_DENIED_MORE_DATA_REQUIRED = -32022

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

RAW_STATE_TOPIC = 'RAW/'
LOCAL_STATE_TOPIC = 'ST/LOC/'
REMOTE_STATE_TOPIC = 'ST/REM/'
REMOTE_ARCHIVE_STATE_TOPIC = 'ST/RAR/'
ANY_STATE_TOPIC = 'ST/+/'
REPLICATION_STATE_TOPIC = 'RPL/ST/'
REPLICATION_INVENTORY_TOPIC = 'RPL/INVENTORY/'
REPLICATION_NODE_STATE_TOPIC = 'RPL/NODE/'
LOG_INPUT_TOPIC = 'LOG/IN/'
LOG_EVENT_TOPIC = 'LOG/EV/'
LOG_CALL_TRACE_TOPIC = 'LOG/TR/'
SERVICE_STATUS_TOPIC = 'SVC/ST'
AAA_ACL_TOPIC = 'AAA/ACL/'
AAA_KEY_TOPIC = 'AAA/KEY/'
AAA_USER_TOPIC = 'AAA/USER/'
AAA_ACCOUNTING_TOPIC = 'AAA/REPORT'

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
    """
    Base item OID class
    """
    def __init__(self, s, from_path=False):
        """
        Constructs a new OID from string

        Args:
            from_path: construct OID from a path (kind/group(s)/id)
        """
        self.kind, self.full_id = s.split('/' if from_path else ':',
                                          maxsplit=1)
        self.oid = f'{self.kind}:{self.full_id}'
        self.id = s.rsplit('/', 1)[-1] if '/' in s else self.full_id

    def __str__(self):
        return self.oid

    def __hash__(self):
        return hash(self.oid)

    def __eq__(self, other):
        if isinstance(other, OID):
            return self.oid == other.oid
        else:
            raise NotImplemented

    def to_path(self):
        """
        Converts OID to path
        """
        return f'{self.kind}/{self.full_id}'


class ServiceInfo:
    """
    Service info helper class
    """
    def __init__(self, author='', description='', version=''):
        """
        Args:
            author: service author
            description: service description
            version: service version
        """
        self.author = author
        self.description = description
        self.version = version
        self.methods = {}

    def add_method(self, method, description='', required=[], optional=[]):
        """
        Add a method to service info help

        Args:
            method: method name
            description: method description
            required: list of required param names (strings)
            optional: list of optional param names
        """
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
    """
    Item action from bus event
    """
    def __init__(self, event):
        a = unpack(event.get_payload())
        self.uuid = uuid.UUID(bytes=a['uuid'])
        self.i = OID(a['i'])
        self.timeout = a['timeout']
        self.priority = a['priority']
        self.params = a['params']


class Controller:
    """
    Action handler helper class for controllers
    """
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
            busrt.client.Frame(payload=_action_event_payload(
                action.uuid,
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
            busrt.client.Frame(msg.encode(), tp=busrt.client.OP_PUBLISH,
                               qos=0))


class Service:
    """
    The primary service class
    """
    def __init__(self):
        self.svc_info = None
        self.logger = None
        self.signals_registered = False
        self.marked_ready = False
        self.marked_terminating = False
        self.privileges_dropped = False
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
                raise RuntimeError(
                    'the service is started in react-to-fail'
                    ' mode, but rtf is not supported by the service')
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
        """
        Is service started in react-to-fail mode
        """
        return self.initial.get('fail_mode', False)

    def is_mode_normal(self):
        """
        Is service started in normal mode
        """
        return not self.initial.get('fail_mode', False)

    def need_ready(self):
        """
        Raises an exception if not ready

        RPC helper method which raises an exception if the service is not ready
        """
        if not self.active:
            raise busrt.rpc.RpcException('service not ready',
                                         ERR_CODE_INTERNAL_RPC)

    def get_config(self):
        """
        Get service configuration
        """
        config = self.initial.get('config')
        if config is None:
            return {}
        else:
            return config

    def init(self, info=None, on_frame=None, on_rpc_call=None):
        """
        Init the service

        Automatically calls init_bus, drop_privileges, init_logs and init_rpc
        (if info specified)

        Optional:

            info: RPC info

            on_frame: bus frame handler

        """
        self.init_bus()
        self.drop_privileges()
        self.init_logs()
        if info:
            self.init_rpc(info)
            if on_frame:
                self.rpc.on_frame = on_frame
            if on_rpc_call:
                self.on_rpc_call = on_rpc_call

    def init_bus(self):
        """
        Init the local bus
        """
        bus_config = self.initial['bus']
        if bus_config['type'] != 'native':
            raise ValueError(f'bus {bus_config["type"]} is not supported')
        self.bus = busrt.client.Client(bus_config['path'], self.id)
        self.bus.buf_size = bus_config['buf_size']
        self.bus.timeout = bus_config['timeout']
        self.bus.connect()

    def init_rpc(self, svc_info):
        """
        Init bus RPC layer
        """
        self.svc_info = svc_info
        self._svc_info_packed = pack(svc_info.serialize())
        self.rpc = busrt.rpc.Rpc(self.bus)
        self.rpc.on_call = self._handle_rpc_call

    def wait_core(self, timeout=None, wait_forever=True):
        """
        Wait until the EVA ICS core is started
        """
        if timeout is None:
            timeout = self.timeout.get('startup', self.timeout['default'])
        wait_until = time.perf_counter() + timeout
        while self.is_active():
            try:
                result = self.rpc.call(
                    'eva.core',
                    busrt.rpc.Request('test')).wait_completed(timeout)
                if unpack(result.get_payload())['active'] is True:
                    return
            except:
                pass
            if not wait_forever and wait_until <= time.perf_counter():
                raise TimeoutError
            time.sleep(self.sleep_step)

    def _handle_rpc_call(self, event):
        method = event.method
        if method == b'test':
            return
        elif method == b'info':
            return self._svc_info_packed
        elif method == b'stop':
            self.active = False
            return
        else:
            return self.on_rpc_call(event)

    def init_logs(self):
        """
        Initialize service logs
        """
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
        self.logger = logger
        return self.logger

    def block(self, prepare=True):
        """
        Block the service until terminated

        Automatically calls register_signals, mark_ready, mark_terminating
        (after receiving a termination signal/event)

        Optional:

            prepare: default: True, if False, register_signals, mark_ready and
                     mark_terminating must be called manually

        """
        if prepare:
            self.register_signals()
            self.mark_ready()
        sleep_step = self.sleep_step
        while self.active and self.bus.is_connected():
            time.sleep(sleep_step)
        if prepare:
            self.mark_terminating()

    def mark_ready(self):
        """
        Mark the service ready

        Automatically logs the service is started if logs are initialized
        """
        if not self.marked_ready:
            self._mark('ready')
            self.marked_ready = True
            if self.logger:
                desc = self.svc_info.description if self.svc_info else None
                self.logger.info(f'{desc} started' if desc else 'started')

    def mark_terminating(self):
        """
        Mark the service terminating
        """
        self.active = False
        self.shutdown_requested = True
        if not self.marked_terminating:
            self._mark('terminating')
            self.marked_terminating = True

    def drop_privileges(self):
        """
        Drop service process privileges
        """
        if not self.privileges_dropped:
            user = self.initial.get('user')
            if user is not None:
                if pwd is None:
                    raise RuntimeError(
                        'pwd module not found, can not drop privileges')
                u = pwd.getpwnam(user)
                groups = os.getgrouplist(user, u.pw_gid)
                os.setgroups(groups)
                os.setgid(u.pw_gid)
                os.setuid(u.pw_uid)
            self.privileges_dropped = True

    def _mark(self, status):
        self.bus.send(
            'SVC/ST',
            busrt.client.Frame(pack({'status': status}),
                               tp=busrt.client.OP_PUBLISH,
                               qos=0))

    def register_signals(self):
        """
        Register service process system signals
        """
        signal.signal(signal.SIGINT, self._term_handler)
        signal.signal(signal.SIGTERM, self._term_handler)

    def _term_handler(self, sig, frame):
        self.active = False

    def is_active(self):
        """
        Check is the service active
        """
        return self.active

    def is_shutdown_requested(self):
        """
        Check is the service shutdown requested
        """
        return self.shutdown_requested

    def report_accounting_event(self,
                                u=None,
                                src=None,
                                svc=None,
                                subj=None,
                                oid=None,
                                data=None,
                                note=None,
                                code=None,
                                err=None):
        """
        Reports an event into accounting system
        """
        payload = {}
        if u is not None:
            payload['u'] = u
        if src is not None:
            payload['src'] = src
        if svc is not None:
            payload['svc'] = svc
        if subj is not None:
            payload['subj'] = subj
        if oid is not None:
            payload['oid'] = oid
        if data is not None:
            payload['data'] = data
        if note is not None:
            payload['note'] = note
        if code is not None:
            payload['code'] = code
        if err is not None:
            payload['err'] = err
        self.bus.send(
            AAA_ACCOUNTING_TOPIC,
            busrt.client.Frame(pack(payload),
                               tp=busrt.client.OP_PUBLISH,
                               qos=1))

    def subscribe_oids(self, oids, event_kind='any'):
        """
        subscribe bus to OID events

        Args:
            oids: list of OIDs or strings
            event_kind: any, remote, remote_archive or local
        """
        if event_kind == 'any':
            topic_pfx = ANY_STATE_TOPIC
        elif event_kind == 'remote':
            topic_pfx = REMOTE_STATE_TOPIC
        elif event_kind == 'remote_archive':
            topic_pfx = REMOTE_ARCHIVE_STATE_TOPIC
        elif event_kind == 'local':
            topic_pfx = LOCAL_STATE_TOPIC
        else:
            raise ValueError('Invalid event kind (accepted:'
                             ' any, remote, remote_arcive or local)')
        topics = []
        if oids:
            for oid in oids:
                if oid == '#':
                    path = oid
                else:
                    if isinstance(oid, str):
                        oid = OID(oid)
                    path = oid.to_path()
                topics.append(f'{topic_pfx}{path}')
            self.bus.subscribe(topics).wait_completed()

    def create_items(self, oids):
        """
        Create items one-by one

        Must be called after the node core is ready

        Ignores errors if an item already exists

        Args:
            oids: list of item OIDs to create
        """
        try:
            for oid in [oids] if isinstance(oids, str) or isinstance(
                    oids, OID) else oids:
                try:
                    self.rpc.call(
                        'eva.core',
                        busrt.rpc.Request('item.create', pack({
                            'i': str(oid),
                        }))).wait_completed()
                except busrt.rpc.RpcException as e:
                    if e.rpc_error_code != ERR_CODE_ALREADY_EXISTS:
                        raise
        except Exception as e:
            raise rpc_e2e(e)

    def deploy_items(self, items):
        """
        Create items using deployment payloads in bulk

        Must be called after the node core is ready

        Payloads are equal to item.deploy eva.core EAPI call

        See also https://info.bma.ai/en/actual/eva4/iac.html#items

        Args:
            oids: list of items to create
        """
        try:
            self.rpc.call(
                'eva.core',
                busrt.rpc.Request(
                    'item.deploy',
                    pack({
                        'items':
                        items if isinstance(items, list) else [items],
                    }))).wait_completed()
        except Exception as e:
            raise rpc_e2e(e)


def no_rpc_method():
    """
    Raise an exception on invalid RPC method
    """
    raise busrt.rpc.RpcException('no such method', ERR_CODE_METHOD_NOT_FOUND)


def log_traceback():
    """
    Log an exception traceback
    """
    import traceback
    print(traceback.format_exc(), flush=True, file=sys.stderr)


class ACI:
    """
    ACI (API Call Info) helper class
    """
    def __init__(self, aci_payload):
        self.auth = aci_payload.get('auth')
        self.token_mode = aci_payload.get('token_mode')
        self.user = aci_payload.get('u')
        self.acl = aci_payload.get('acl')
        self.src = aci_payload.get('src')

    def is_writable(self):
        """
        Check is the current session writable or read-only
        """
        return self.auth != 'token' or self.token_mode != 'readonly'


class XCall:
    """
    HMI X calls helper class
    """
    def __init__(self, payload):
        self.method = payload.get('method')
        self.params = payload.get('params', {})
        self.aci = ACI(payload.get('aci', {}))
        self.acl = payload.get('acl', {})

    def get_items_allow_deny_reading(self):
        """
        Get allow and deny item list from ACL
        """
        if self.is_admin():
            return (['#'], [])
        else:
            allow = set()
            deny = set()

            for oid in self.acl.get('read', {}).get('items', []):
                allow.add(OID(oid))
            for oid in self.acl.get('write', {}).get('items', []):
                allow.add(OID(oid))
            for oid in self.acl.get('deny_read', {}).get('items', []):
                deny.add(OID(oid))
            return (list(allow), list(deny))

    def is_writable(self):
        """
        Check is the current session writable or read-only
        """
        return self.aci.is_writable()

    def is_admin(self):
        """
        Check if the session ACL has admin rights
        """
        return self.acl.get('admin')

    def check_op(self, op):
        """
        Check if the session ACL has rights for the operation

        Args:
            op: operation code (e.g. "supervisor")
        """
        return self.is_admin() or op in self.acl.get('ops', [])

    def is_item_readable(self, oid):
        """
        Check if the session ACL has rights to read an item
        """
        return self.is_admin() or (
            (oid_match(oid,
                       self.acl.get('read', {}).get('items', []))
             or oid_match(oid,
                          self.acl.get('write', {}).get('items', [])))
            and not oid_match(oid,
                              self.acl.get('deny_read', {}).get('items', [])))

    def is_item_writable(self, oid):
        """
        Check if the session ACL has rights to write an item
        """
        return self.is_admin() or (
            oid_match(oid,
                      self.acl.get('write', {}).get('items', []))
            and not oid_match(oid,
                              self.acl.get('deny_read', {}).get('items', []))
            and not oid_match(oid,
                              self.acl.get('deny_write', {}).get('items', []))
            and not oid_match(oid,
                              self.acl.get('deny', {}).get('items', [])))

    def is_pvt_readable(self, path):
        """
        Check if the session ACL has rights to read a pvt path
        """
        return self.is_admin() or (
            path_match(path,
                       self.acl.get('read', {}).get('pvt', []))
            and not path_match(path,
                               self.acl.get('deny_read', {}).get('pvt', [])))

    def require_writable(self):
        if not self.is_writable():
            raise busrt.rpc.RpcException('the session is read-only',
                                         ERR_CODE_ACCESS_DENIED)

    def require_admin(self):
        if not self.is_admin:
            raise busrt.rpc.RpcException('admin access required',
                                         ERR_CODE_ACCESS_DENIED)

    def require_op(self, op):
        if not self.check_op(op):
            raise busrt.rpc.RpcException(f'operation access required: {op}',
                                         ERR_CODE_ACCESS_DENIED)

    def require_item_read(self, oid):
        if not self.is_item_readable(oid):
            raise busrt.rpc.RpcException(f'read access required for: {oid}',
                                         ERR_CODE_ACCESS_DENIED)

    def require_item_write(self, oid):
        if not self.is_item_writable(oid):
            raise busrt.rpc.RpcException(f'write access required for: {oid}',
                                         ERR_CODE_ACCESS_DENIED)

    def require_pvt_read(self, path):
        if not self.is_pvt_readable(path):
            raise busrt.rpc.RpcException(f'read access required for: {path}',
                                         ERR_CODE_ACCESS_DENIED)


class XCallDefault:
    """
    HMI X calls mocker for no ACI/ACL
    """
    def __init__(self):
        self.aci = {}
        self.acl = {}
        self.method = None
        self.params = {}

    def get_items_allow_deny_reading(self):
        return (['#'], [])

    def is_writable(self):
        return True

    def is_admin(self):
        return True

    def check_op(self, op):
        return True

    def is_item_readable(self, oid):
        return True

    def is_item_writable(self, oid):
        return True

    def is_pvt_readable(self, path):
        return True

    def require_writable(self):
        pass

    def require_admin(self):
        pass

    def require_op(self, op):
        pass

    def require_item_read(self, oid):
        pass

    def require_item_write(self, oid):
        pass

    def require_pvt_read(self, path):
        pass


def oid_match(oid, oid_masks):
    return path_match(oid.to_path(), [OID(o).to_path() for o in oid_masks])


def path_match(path, masks):
    if '#' in masks or path in masks:
        return True
    for mask in masks:
        g1 = mask.split('/')
        g2 = path.split('/')
        match = True
        for i in range(0, len(g1)):
            try:
                if i >= len(g2):
                    match = False
                    break
                if g1[i] == "#":
                    return True
                if g1[i] != "+" and g1[i] != g2[i]:
                    match = False
                    break
                if i == len(g1) - 1 and len(g2) > len(g1):
                    match = False
            except IndexError:
                match = False
                break
        if match:
            return True
    return False


def self_test():
    assert oid_match(OID("sensor:content/data"), ["sensor:content/#"])
    assert not oid_match(OID("sensor:content/data"), ["sensor:+"])
    assert oid_match(OID("sensor:content/data"), ["sensor:content/+"])
    assert oid_match(OID("sensor:content/data"), ["sensor:+/data"])
    assert oid_match(OID("sensor:content/data"), ["sensor:+/#"])
    assert not oid_match(OID("sensor:content/data"), ["sensor:+/data2"])
    assert oid_match(OID("sensor:content/data"), ["sensor:#"])

    assert path_match('content/data', ['#', 'content'])
    assert not path_match('content/data', ['content'])
    assert path_match('content/data', ['content/+'])
    assert path_match('content/data', ['+/data'])
    assert not path_match('content/data', ['content/+/data'])
    assert path_match('content/data', ['content/data', 'content/+/data'])

    payload = {
        'method': 'list',
        'params': {
            'i': 'test'
        },
        'aci': {
            'auth': 'token',
            'token_mode': 'normal',
            'u': 'admin',
            'acl': 'admin',
            'src': '127.0.0.1',
        },
        'acl': {
            'id': 'admin',
            'read': {
                'items': ['unit:#'],
                'pvt': ['data/#'],
                'rpvt': []
            },
            'write': {
                'items': []
            },
            'deny_read': {
                'items': [],
                'pvt': ['data/secret'],
                'rpvt': []
            },
            'ops': ['supervisor'],
            'meta': {
                'admin': ['any']
            },
            'from': ['admin']
        }
    }

    xcall = XCall(payload)
    assert xcall.method == 'list'
    assert xcall.params.get('i') == 'test'
    assert xcall.aci.auth == 'token'
    assert xcall.aci.user == 'admin'
    assert xcall.aci.src == '127.0.0.1'
    assert xcall.aci.is_writable()
    assert xcall.is_item_readable(OID('unit:tests/t1'))
    assert not xcall.is_item_readable(OID('sensor:tests/t1'))
    assert xcall.is_pvt_readable('data/var1')
    assert not xcall.is_pvt_readable('data2/var1')
    assert not xcall.is_pvt_readable('data/secret')
    assert xcall.check_op('supervisor')
    assert not xcall.check_op('devices')


self_test()

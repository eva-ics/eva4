import busrt
import logging
import sys
import os
import glob
import time
try:
    import rapidjson as json
except:
    import json
import threading
import datetime

from uuid import UUID

from evaics.exceptions import FunctionFailed
from evaics.exceptions import ResourceAlreadyExists
from evaics.exceptions import ResourceNotFound
from evaics.exceptions import ResourceBusy
from evaics.exceptions import AccessDenied
from evaics.exceptions import InvalidParameter
from evaics.exceptions import MethodNotImplemented

from evaics.sdk import LocalProxy, rpc_e2e, pack, unpack, OID
from evaics.sdk import RAW_STATE_TOPIC

from evaics.tools import dict_from_str

from functools import partial

_shared = {}
_shared_lock = threading.RLock()

eva_dir = None
svc_data_dir = None
service = None

locker_svc = None
mailer_svc = None
alarm_svc = None

g = LocalProxy()


def parse_action_payload(payload):
    payload['uuid'] = str(UUID(bytes=payload['uuid']))
    return payload


def get_service():
    """
    Get the service object for the direct access

    e.g. service.bus: direct access to BUS/RT,
    service.rpc: direct access to BUS/RT RPC

    Returns:
        the service object
    """
    return service


def get_system_name():
    """
    Get the system name

    Returns:
        system name
    """
    return service.system_name


def rpc_call(method=None, _target='eva.core', _timeout=None, **kwargs):
    """
    Performs a bus RPC call

    The method parameters are specified in kwargs

    Args:
        method: method

    Optional:
        _target: target service (default: eva.core)
        _timeout: call timeout

    Returns:
        the bus call result
    """
    try:
        result = service.rpc.call(
            _target, busrt.rpc.Request(
                method,
                pack(kwargs) if kwargs else None)).wait_completed(
                    _timeout if _timeout is not None else g.get('t').get(
                        check=True)).get_payload()
    except busrt.rpc.RpcException as e:
        raise rpc_e2e(e)
    if result:
        return unpack(result)
    else:
        return True


def bus_publish(topic, payload, qos=0):
    """
    Publishes a message to a bus topic

    Args:
        topic: topic name
        payload: message payload
        qos: QoS level (default: 0)
    """
    try:
        service.bus.send(
            topic,
            busrt.client.Frame(pack(payload),
                               tp=busrt.client.OP_PUBLISH,
                               qos=qos)).wait_completed()
    except busrt.rpc.RpcException as e:
        raise rpc_e2e(e)


def update_state(oid, state):
    """
    Updates item state

    Args:
        oid: item OID
        state: new state (may contain status/value/t and other state fields)
    """
    if 'status' not in state:
        state['status'] = 1
    oid = OID(oid) if isinstance(oid, str) else oid
    topic = f'{RAW_STATE_TOPIC}{oid.to_path()}'
    bus_publish(topic, state)


def report_accounting_event(u=None,
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

    Optional:
        u: user account name (string)
        src: source (e.g. IP address)
        svc: service ID (default: sender)
        subj: event subject
        oid: affected item OID
        data: a structure with any additional information
        note: a custom note (string)
        code: error code (0 = success)
        err: error message
    """
    service.report_accounting_event(
        u=u,
        src=src,
        svc=svc,
        subj=subj,
        oid=oid,
        data=data,
        note=note,
        code=code,
        err=err,
    )


def shared(name, default=None):
    """
    Gets value of the shared variable

    Gets value of the variable, shared between controller macros

    Args:
        name: variable name

    Optional:
        default: value if variable doesn't exist

    Returns:
        variable value, None (or default) if variable doesn't exist
    """
    with _shared_lock:
        return _shared.get(name, default)


def set_shared(name, value=None):
    """
    Sets value of the shared variable

    Sets value of the variable, shared between controller macros

    Args:
        name: variable name

    Optional:
        value: value to set. If empty, the variable is deleted
    """
    with _shared_lock:
        if value is None:
            try:
                del _shared[name]
            except KeyError:
                pass
        else:
            _shared[name] = value
        return True


def increment_shared(name):
    """
    Increments value of the shared variable

    Increments value of the variable, shared between controller macros. Initial
    value must be number

    Args:
        name: variable name
    """
    with _shared_lock:
        v = shared(name)
        if v is None:
            v = 1
        else:
            v = int(v) + 1
        set_shared(name, v)
        return v


def decrement_shared(name):
    """
    Decrements value of the shared variable

    Decrements value of the variable, shared between controller macros.
    Initial value must be number

    Args:
        name: variable name
    """
    with _shared_lock:
        v = shared(name)
        if v is None:
            v = -1
        else:
            v = int(v) - 1
        set_shared(name, v)
        return v


def ping(host, timeout=1000, count=1):
    """
    Pings a remote host

    Requires fping tool

    Args:
        host: host name or IP to ping
        timeout: ping timeout in milliseconds (default: 1000)
        count: number of packets to send (default: 1)

    Returns:
        True if host is alive, False if not
    """
    return os.system(
        f'fping -t {timeout} -c {count} {host} > /dev/null 2>&1') == 0


def _exit(code=0):
    """
    Finishes macro execution

    Args:
        code: macro exit code (default: 0, no errors)
    """
    sys.exit(code)


def lock(lock_id, expires, timeout=None):
    """
    Acquires a lock

    Requires locker svc to be set in the controller config

    Args:
        lock_id: lock id
        expires: time after which the lock is automatically unlocked (sec)

    Optional:
        timeout: max timeout to wait

    Raises:
        FunctionFailed: failed to acquire the lock
        TimeoutException: timed out
    """
    if locker_svc is None:
        raise MethodNotImplemented('no locker svc defined')
    else:
        rpc_call('lock',
                 _target=locker_svc,
                 i=lock_id,
                 expires=expires,
                 timeout=timeout,
                 _timeout=timeout)


def unlock(lock_id):
    """
    Releases a lock

    Releases the previously acquired lock

    Args:
        lock_id: lock id

    Raises:
        FunctionFailed: ffailed to release the lock
    """
    if locker_svc is None:
        raise MethodNotImplemented('no locker svc defined')
    else:
        rpc_call('unlock', _target=locker_svc, i=lock_id)


def mail(subject=None, text=None, rcp=None, i=None):
    """
    Sends email message

    Requires mailer svc to be set in the controller config

    Optional:
        subject: email subject
        text: email text
        rcp: email recipient or array of the recipients
        i: user login recipient or array of the recipients

    Raises:
        FunctionFailed: mail is not sent
    """
    if mailer_svc is None:
        raise MethodNotImplemented('no mailer svc defined')
    else:
        rpc_call('send',
                 _target=mailer_svc,
                 subject=subject,
                 text=text,
                 rcp=rcp,
                 i=i)


def set_alarm(oid, op, source='lmacro'):
    """
    Sets alarm state

    Requires alarm svc to be set in the controller config

    Args:
        oid: alarm OID
        op: alarm operation
    Optional:
        source: alarm source (default: lmacro)

    Raises:
        FunctionFailed: failed to set alarm state
    """
    if alarm_svc is None:
        raise MethodNotImplemented('no alarm svc defined')
    else:
        rpc_call('alarm.set',
                 _target=alarm_svc,
                 i=oid,
                 op=op,
                 source=source,
                 sk='P')


def state(oid):
    """
    Gets item state

    Args:
        oid: item OID or mask

    Returns:
        item status/value dict or list for mask

    Raises:
        ResourceNotFound: item not found

    @var_out status
    @var_out value
    """
    i = str(oid)
    result = rpc_call('item.state', i=i)
    if not result:
        raise ResourceNotFound
    if '+' in i or '#' in i:
        return result
    elif result[0]['oid'] != i:
        raise ResourceNotFound
    else:
        return result[0]


def status(oid):
    """
    Gets item status

    Args:
        oid: item OID

    Returns:
        item status (integer)

    Raises:
        ResourceNotFound: item not found
    """
    return state(oid)['status']


def value(oid, default=None):
    """
    Gets item value

    Args:
        i: item OID

    Optional:
        default: value if null (default is empty string)

    Returns:
        item value

    Raises:
        ResourceNotFound: item not found
    """
    value = state(oid)['value']
    if value is None:
        return default
    else:
        return value


def is_expired(oid):
    """
    Checks is lvar (timer) or item state expired/error

    Args:
        oid: item OID

    Returns:
        True if the timer has been expired

    Raises:
        ResourceNotFound: item not found
    """
    return state(oid)['status'] == -1


def is_busy(oid):
    """
    Checks is the unit busy

    Args:
        oid: unit OID

    Returns:
        True if unit is busy (action is executed)

    Raises:
        ResourceNotFound: unit not found
    """
    return state(oid)['act'] > 0


def _set(oid, status=None, value=None):
    """
    Sets lvar value

    Args:
        oid: lvar OID

    Optional:
        value: lvar value (if not specified, lvar is set to null)

    Raises:
        FunctionFailed: lvar value set error
        ResourceNotFound: lvar not found
    """
    params = {'i': str(oid)}
    if status is not None:
        params['status'] = status
    if value is not None:
        params['value'] = None if value == 'null' else value
    return rpc_call('lvar.set', **params)


def reset(oid):
    """
    Resets lvar status

    Set lvar status to 1 or start lvar timer

    Args:
        oid: lvar OID

    Raises:
        FunctionFailed: lvar value set error
        ResourceNotFound: lvar not found
    """
    return rpc_call('lvar.reset', i=str(oid))


def clear(oid):
    """
    Clears lvar status

    Set lvar status to 0 or stop timer lvar (set timer status to 0)

    Args:
        oid: lvar OID

    Raises:
        FunctionFailed: lvar value set error
        ResourceNotFound: lvar not found
    """
    return rpc_call('lvar.clear', i=str(oid))


def toggle(oid):
    """
    Toggles lvar status

    Change lvar status to opposite boolean (0->1, 1->0)

    Args:
        oid: lvar OID

    Raises:
        FunctionFailed: lvar value set error
        ResourceNotFound: lvar not found
    """
    return rpc_call('lvar.toggle', i=str(oid))


def increment(oid):
    """
    Increments lvar value

    Args:
        lvar_id: lvar OID

    Raises:
        FunctionFailed: lvar value increment error
        ResourceNotFound: lvar not found
    """
    return rpc_call('lvar.incr', i=str(oid))


def decrement(oid):
    """
    Decrements lvar value

    Args:
        oid: lvar OID

    Raises:
        FunctionFailed: lvar value decrement error
        ResourceNotFound: lvar not found
    """
    return rpc_call('lvar.decr', i=str(oid))


def action(oid, value, status=None, wait=None, priority=None):
    """
    Executes unit control action
    
    Args:
        oid: unit OID
        value: desired unit value

    Optional:
        wait: wait for the completion for the specified number of seconds
        priority: queue priority (default is 100, lower is better)

    Returns:
        Serialized action object (dict)

    Raises:
        FunctionFailed: action failed to be executed
        ResourceNotFound: unit not found

    @var_out exitcode Exit code
    @var_out status Action status
    """
    params = {'value': value}
    if status is not None:
        logging.warning(
            'status field in actions is ignored and deprecated. remove the field from API call payloads'
        )
    payload = {'i': str(oid), 'params': params}
    if wait is not None:
        payload['wait'] = wait
    if priority is not None:
        payload['priority'] = priority
    return parse_action_payload(rpc_call('action', **payload))


def action_toggle(oid, wait=None, priority=None):
    """
    Executes an action to toggle unit status
    
    Creates a unit control action to toggle its status (1->0, 0->1)

    Args:
        oid: unit OID

    Optional:
        value: desired unit value
        wait: wait for the completion for the specified number of seconds
        uuid: action UUID (will be auto generated if none specified)
        priority: queue priority (default is 100, lower is better)

    Returns:
        Serialized action object (dict)

    Raises:
        ResourceNotFound: unit not found

    @var_out exitcode Exit code
    @var_out status Action status
    """
    return parse_action_payload(
        rpc_call('action.toggle', i=str(oid), wait=wait, priority=priority))


def result(oid=None, uuid=None, sq=None, limit=None):
    """
    Gets action status

    Checks the result of the action by its UUID or returns the actions for
    the specified unit

    Args:
        oid: unit OID or
        uuid: action uuid

    Optional:
        sq: filter by action status: waiting, running, completed, failed or
                finished
        limit: limit action list to N records

    Returns:
        list or single serialized action object

    Raises:
        ResourceNotFound: unit or action not found

    @var_out exitcode Exit code
    @var_out status Action status
    """
    if oid is not None:
        return [
            parse_action_payload(a)
            for a in rpc_call('action.list', i=str(oid), sq=sq, limit=limit)
        ]
    elif uuid is not None:
        return parse_action_payload(
            rpc_call('action.result', u=UUID(str(uuid)).bytes))
    else:
        raise InvalidParameter('either oid or uuid must be specified')


def action_start(oid, wait=None, priority=None):
    """
    Executes an action to a unit
    
    Creates unit control action to set its value to 1

    Args:
        oid: unit OID

    Optional:
        wait: wait for the completion for the specified number of seconds
        priority: queue priority (default is 100, lower is better)

    Returns:
        Serialized action object (dict)

    Raises:
        ResourceNotFound: unit not found

    @var_out exitcode Exit code
    @var_out status Action status
    """
    return action(oid, value=1, wait=wait, priority=priority)


def action_stop(oid, wait=None, priority=None):
    """
    Executes an action to stop a unit
    
    Creates unit control action to set its value to 0

    Args:
        oid: unit OID

    Optional:
        wait: wait for the completion for the specified number of seconds
        priority: queue priority (default is 100, lower is better)

    Returns:
        Serialized action object (dict)

    Raises:
        ResourceNotFound: unit not found

    @var_out exitcode Exit code
    @var_out status Action status
    """
    return action(oid, value=0, wait=wait, priority=priority)


def terminate(uuid):
    """
    Terminates action execution
    
    Terminates or cancel the action if it is still queued
    
    Args:
        uuid: action uuid
        
    Raises:
        ResourceNotFound: if action is not found or action is already finished
    """
    return rpc_call('action.terminate', u=UUID(str(uuid)).bytes)


def kill(oid):
    """
    Kills unit actions

    Terminates the current action (if possible) and cancels all pending

    Args:
        oid: unit OID

    Raises:
        ResourceNotFound: unit not found
    """
    return rpc_call('action.kill', i=str(oid))


def run(_oid, *args, _wait=None, _priority=None, **kwargs):
    """
    Executes another lmacro

    Args and kwargs are passed to the target lmacro as-is, except listed below.

    Args:
        _oid: lmacro OID

    Optional:
        _wait: wait for the completion for the specified number of seconds
        _priority: queue priority (default is 100, lower is better)

    Returns:
        Serialized macro action object (dict)

    Raises:
        ResourceNotFound: macro not found

    @var_out exitcode Exit code
    @var_out status Action status
    @var_out out Macro "out" variable
    """
    payload = {'i': str(_oid)}
    if _wait is not None:
        payload['wait'] = _wait
    if _priority is not None:
        payload['priority'] = _priority
    if args:
        payload.setdefault('params', {})['args'] = args
    if kwargs:
        payload.setdefault('params', {})['kwargs'] = kwargs
    return parse_action_payload(rpc_call('run', **payload))


def date():
    """
    Gets current date/time

    Returns:
        Serialized date/time object (dict)

    @var_out year
    @var_out month
    @var_out day
    @var_out weekday
    @var_out hour
    @var_out minute
    @var_out second
    @var_out timestamp
    """
    t = datetime.datetime.now()
    return {
        'year': t.year,
        'month': t.month,
        'day': t.day,
        'weekday': t.weekday(),
        'hour': t.hour,
        'minute': t.minute,
        'second': t.second,
        'timestamp': t.timestamp()
    }


def ls(mask, recursive=False):
    """
    Lists files in directory

    If recursive is true, the pattern "**" will match any files and zero or
    more directories and subdirectories.

    Args:
        mask: path and mask (e.g. /opt/data/\*.jpg)
        recursive: if True, perform a recursive search

    Returns:
        dict with fields 'name' 'file', 'size' and 'time' { 'c': created,
        'm': modified }
    """
    fls = [x for x in glob.glob(mask, recursive=recursive) if os.path.isfile(x)]
    l = []
    for x in fls:
        l.append({
            'name': os.path.basename(x),
            'file': os.path.abspath(x),
            'size': os.path.getsize(x),
            'time': {
                'c': os.path.getctime(x),
                'm': os.path.getmtime(x)
            }
        })
    return l


def sha256sum(value, hexdigest=True):
    """
    Calculates SHA256 sum

    Args:
        value: value to calculate
        hexdigest: return binary digest or hex (True, default)

    Returns:
        sha256 digest
    """
    if not isinstance(value, bytes):
        value = str(value).encode()
    from hashlib import sha256
    s = sha256()
    s.update(value)
    return s.hexdigest() if hexdigest else s.digest()


def get_directory(tp):
    """
    Gets path to EVA ICS directory

    Args:
        tp: directory type: eva, runtime, svc_data, venv or xc
    Raises:
        LookupError: if the directory type is invalid
    """
    if tp == 'eva':
        return eva_dir
    elif tp == 'svc_data':
        return svc_data_dir
    elif tp not in ['runtime', 'xc', 'venv']:
        raise LookupError
    else:
        return f'{eva_dir}/{tp}'


def _system(*args, **kwargs):
    code = os.system(*args, **kwargs)
    if code != 0:
        raise FunctionFailed(f'command failed with exit code {code}')


def log_message(level, *args, **kwargs):
    try:
        del kwargs['flush']
    except KeyError:
        pass
    logging.log(level, ' '.join([str(s) for s in args]), **kwargs)


api_globals = {
    'FunctionFailed': FunctionFailed,
    'ResourceAlreadyExists': ResourceAlreadyExists,
    'ResourceNotFound': ResourceNotFound,
    'ResourceAlreadyExists': ResourceBusy,
    'AccessDenied': AccessDenied,
    'InvalidParameter': InvalidParameter,
    'json': json,
    'os': os,
    'sys': sys,
    'bus': busrt,
    'on': 1,
    'off': 0,
    'yes': True,
    'no': False,
    'get_directory': get_directory,
    'shared': shared,
    'set_shared': set_shared,
    'increment_shared': increment_shared,
    'decrement_shared': decrement_shared,
    'print': partial(log_message, logging.INFO),
    'trace': partial(log_message, 1),
    'debug': partial(log_message, logging.DEBUG),
    'info': partial(log_message, logging.INFO),
    'warn': partial(log_message, logging.WARNING),
    'warning': partial(log_message, logging.WARNING),
    'error': partial(log_message, logging.ERROR),
    'critical': partial(log_message, logging.CRITICAL),
    'exit': _exit,
    '_sleep': time.sleep,
    'lock': lock,
    'unlock': unlock,
    'is_expired': is_expired,
    'is_busy': is_busy,
    'set': _set,
    'state': state,
    'update_state': update_state,
    'sha256sum': sha256sum,
    'status': status,
    'value': value,
    'reset': reset,
    'clear': clear,
    'toggle': toggle,
    'increment': increment,
    'decrement': decrement,
    'action': action,
    'action_toggle': action_toggle,
    'result': result,
    'start': action_start,
    'stop': action_stop,
    'service': get_service,
    'system_name': get_system_name,
    'terminate': terminate,
    'kill': kill,
    'run': run,
    'system': _system,
    'ping': ping,
    'time': time.time,
    'mail': mail,
    'set_alarm': set_alarm,
    'instant': time.perf_counter,
    'date': date,
    'ls': ls,
    'rpc_call': rpc_call,
    'bus_publish': bus_publish,
    'report_accounting_event': report_accounting_event
}

__version__ = '0.1.6'

from evaics.sdk import Service, Controller, Action, no_rpc_method, ServiceInfo
from evaics.sdk import OID

from timeouter import Timer

from types import SimpleNamespace
from pathlib import Path
from concurrent.futures import ThreadPoolExecutor

from . import macro_api

import sys
import os
import threading
import traceback

dir_me = Path(__file__).absolute().parents[0].as_posix()

_d = SimpleNamespace(dir_xcpy=None,
                     common_py=None,
                     controller=None,
                     timeout=None,
                     service=None,
                     pool=None,
                     builtins=None,
                     env=None)

bfname = f'{dir_me}/macro_builtins.py'
d = {}
with open(bfname) as fh:
    _d.builtins = compile(fh.read(), bfname, 'exec')


class Compiler:

    def __init__(self):
        self.cache = {}
        self.cache_lock = threading.Lock()

    def compile(self, fname):
        mtime = os.path.getmtime(fname)
        with self.cache_lock:
            try:
                entry = self.cache[fname]
                if mtime <= entry['m']:
                    return entry['c']
            except KeyError:
                pass
            with open(fname) as fh:
                code = compile(fh.read(), fname, mode='exec')
                self.cache[fname] = {'m': mtime, 'c': code}
            return code


compiler = Compiler()


def prepare_env(i, args, kwargs):
    env = _d.env.copy()
    env['args'] = args
    env['kwargs'] = kwargs
    env['_0'] = i.id
    env['_00'] = i.full_id
    for n in range(1, 10):
        nn = f'_{n}'
        try:
            env[nn] = args[n - 1]
        except IndexError:
            for x in range(n, 10):
                env[f'_{x}'] = None
            break
    for k, v in kwargs.items():
        env[k] = v
    if 'aci' not in kwargs:
        env['aci'] = None
    if 'acl' not in kwargs:
        env['acl'] = None
    return env


def run_macro(oid, args, kwargs, timeout, env, skip_non_existing=False):
    macro_api.g.set('t', Timer(timeout))
    i = oid.id
    if '../' in i:
        raise ValueError('invalid macro ID')
    fname = f'{_d.dir_xcpy}/{i}.py'
    try:
        code = compiler.compile(fname)
    except FileNotFoundError:
        if not skip_non_existing:
            raise
        else:
            return
    env.update(prepare_env(oid, args, kwargs))
    env['_timeout'] = timeout
    exec(_d.builtins, env)
    try:
        common_c = compiler.compile(_d.common_py)
        exec(common_c, env)
    except FileNotFoundError:
        pass
    exec(code, env)


def safe_run_macro(action):
    _d.controller.event_running(action)
    env = {}
    try:
        i = action.i
        args = action.params.get('args', [])
        kwargs = action.params.get('kwargs', {})
        timeout = action.timeout / 1_000_000
        exitcode = 0
        try:
            run_macro(i, args, kwargs, timeout, env)
        except SystemExit as value:
            exitcode = value.code
        if exitcode == 0:
            _d.controller.event_completed(action, out=env.get('out'))
        else:
            _d.controller.event_failed(action,
                                       out=env.get('out'),
                                       exitcode=exitcode)
    except Exception as e:
        msg = traceback.format_exc()
        _d.logger.error(msg)
        _d.controller.event_failed(action,
                                   out=env.get('out'),
                                   err=msg,
                                   exitcode=-1)


def safe_run_system_macro(i):
    try:
        oid = OID(f'lmacro:system/{i}')
        run_macro(oid, [], {}, _d.timeout, {}, skip_non_existing=True)
    except Exception as e:
        msg = traceback.format_exc()
        _d.logger.error(msg)


def handle_rpc(event):
    _d.service.need_ready()
    if event.method == b'run':
        action = Action(event)
        _d.controller.event_pending(action)
        _d.pool.submit(safe_run_macro, action)
    else:
        no_rpc_method()


def run():
    info = ServiceInfo(author='Bohemia Automation',
                       description='Python macros runner',
                       version=__version__)
    service = Service()
    _d.timeout = service.timeout['default']
    config = service.get_config()
    poll_delay = config.get('poll_delay', 0.1)
    pool_size = config.get('pool_size', 50)
    _d.pool = ThreadPoolExecutor(max_workers=pool_size)
    eva_dir = service.initial['core']['path']
    sys.path.insert(0, f'{eva_dir}/venv/bin')
    macro_api.eva_dir = eva_dir
    macro_api.svc_data_dir = service.data_path
    macro_dir = config.get('macro_dir', 'xc/py')
    dir_xcpy = macro_dir if macro_dir.startswith(
        '/') else f'{eva_dir}/runtime/{macro_dir}'
    try:
        Path(dir_xcpy).mkdir(parents=True, exist_ok=True)
    except:
        pass
    _d.dir_xcpy = dir_xcpy
    _d.common_py = f'{dir_xcpy}/common.py'
    service.init_bus()
    service.drop_privileges()
    _d.service = service
    _d.controller = Controller(service.bus)
    _d.logger = service.init_logs()
    _d.env = {
        '_polldelay': poll_delay,
        'is_shutdown': service.is_shutdown_requested
    }
    _d.env.update(config.get('cvars', {}))
    _d.env.update(macro_api.api_globals)
    macro_api.service = service
    macro_api.locker_svc = config.get('locker_svc')
    macro_api.mailer_svc = config.get('mailer_svc', 'eva.svc.mailer')
    macro_api.alarm_svc = config.get('alarm_svc', 'eva.alarm.default')
    service.on_rpc_call = handle_rpc
    service.init_rpc(info)
    _d.logger.info(f'dir: {_d.dir_xcpy}, pool_size: {pool_size}')
    service.register_signals()
    service.mark_ready()
    service.wait_core()
    _d.pool.submit(safe_run_system_macro, 'autoexec')
    service.block(prepare=False)
    _d.pool.submit(safe_run_system_macro, 'shutdown')
    service.mark_terminating()

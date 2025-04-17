from . import pack, unpack

import platform
import os
import sys
import busrt
import subprocess
from pathlib import Path

TIMEOUT = 5

SVC_TPL = '''#!/opt/eva4/venv/bin/python

__version__ = '0.0.1'

import evaics.sdk as sdk
import busrt

from types import SimpleNamespace
from evaics.sdk import pack, unpack, OID

# define a global namespace
_d = SimpleNamespace(service=None)

# RPC calls handler
def handle_rpc(event):
    sdk.no_rpc_method()


# handle BUS/RT frames
def on_frame(frame):
    if _d.service.is_active():
        pass


def run():
    # define ServiceInfo object
    info = sdk.ServiceInfo(author='Bohemia Automation',
                           description='Python service',
                           version=__version__)
    # create a service object
    service = sdk.Service()
    _d.service = service
    # get the service config
    config = service.get_config()
    # init the service
    service.init(info, on_frame=on_frame, on_rpc_call=handle_rpc)
    service.block()


run()
'''


def run_service(bus_path,
                svc_id,
                svc_file,
                id_override=None,
                data_path=None,
                user=None):
    svc_path = Path(svc_file)
    if not svc_path.exists():
        raise FileNotFoundError(f'{svc_file} not found')
    hostname = platform.node()
    pid = os.getpid()
    me = f'eva-svc-launcher.{hostname}.{pid}'
    bus = busrt.client.Client(bus_path, me)
    bus.timeout = TIMEOUT
    bus.connect()
    rpc = busrt.rpc.Rpc(bus)
    initial = unpack(
        rpc.call('eva.core',
                 busrt.rpc.Request('svc.get_init', pack(
                     dict(i=svc_id)))).wait_completed(TIMEOUT).get_payload())[0]
    if data_path is not None:
        initial['data_path'] = data_path
    svc_id = id_override or f'{svc_id}.{hostname}.{pid}'
    initial['id'] = svc_id
    initial['bus']['path'] = bus_path
    initial['user'] = user
    env = os.environ.copy()
    env['EVA_DIR'] = initial['core']['path']
    env['EVA_SYSTEM_NAME'] = initial['system_name']
    env['EVA_SVC_ID'] = svc_id
    env['EVA_TIMEOUT'] = str(initial['timeout']['default'])
    env['EVA_SVC_DEBUG'] = '1'
    args = []
    if svc_file.endswith('.py'):
        args = [sys.executable, '-u']
    args.append(svc_file)
    initial_packed = pack(initial)
    p = b'\x01' + len(initial_packed).to_bytes(4, 'little') + initial_packed
    process = subprocess.Popen(args, stdin=subprocess.PIPE, env=env)
    process.communicate(input=p)
    exitcode = process.wait()
    sys.exit(exitcode)


def main():
    from argparse import ArgumentParser
    ap = ArgumentParser()
    root_sp = ap.add_subparsers(dest='_command',
                                metavar='COMMAND',
                                help='command')

    sp = root_sp.add_parser('new', help='Create new Python service')
    sp.add_argument('NAME', help='Service file name')

    sp = root_sp.add_parser('run', help='Run service')
    sp.add_argument('-b',
                    '--bus',
                    default='/opt/eva4/var/bus.ipc',
                    help='BUS/RT IPC path')
    sp.add_argument('-i',
                    '--id-override',
                    help='Override service ID (default: SVC_ID.hostname.pid')
    sp.add_argument(
        '-d',
        '--data-path',
        help='Override the service data path (use a local directory)')
    sp.add_argument('-u', '--user', help='Run as a restricted user')
    sp.add_argument('SVC_ID', help='service ID')
    sp.add_argument('FILE', help='service file')

    a = ap.parse_args()

    if a._command == 'new':
        fname = a.NAME
        if not fname.endswith('.py'):
            fname += '.py'
        with open(fname, 'w') as f:
            f.write(SVC_TPL)
            print(f'Service file created: {a.NAME}')
    elif a._command == 'run':
        run_service(a.bus, a.SVC_ID, a.FILE, a.id_override, a.data_path, a.user)


if __name__ == '__main__':
    main()

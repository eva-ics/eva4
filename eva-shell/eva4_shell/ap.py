import os
import icli
import readline
import argcomplete
import neotermcolor

from busrt.rpc import RpcException

from neotermcolor import colored

neotermcolor.readline_always_safe = True

from .sharedobj import common, current_command
from .tools import print_tb, err
from .compl import ComplOID, ComplSvc, ComplNode, ComplYamlFile
from .compl import ComplOIDtp, ComplSvcRpcMethod, ComplSvcRpcParams, ComplEdit
from .client import call_rpc, DEFAULT_DB_SERVICE, DEFAULT_REPL_SERVICE
from .client import DEFAULT_ACL_SERVICE, DEFAULT_AUTH_SERVICE
from .client import DEFAULT_KIOSK_SERVICE

DEFAULT_RPC_ERROR_MESSAGE = {
    -32700: 'parse error',
    -32600: 'invalid request',
    -32601: 'method not found',
    -32602: 'invalid method params',
    -32603: 'internal server error'
}


def dispatcher(_command,
               _subc=None,
               debug=False,
               json=False,
               timeout=5.0,
               **kwargs):
    current_command.debug = debug
    current_command.json = json
    current_command.timeout = timeout
    try:
        method = _command
        if _subc is not None:
            method += '_' + _subc.replace('.', '_')
        getattr(common.cli, method.replace('-', '_'))(**kwargs)
        current_command.exit_code = 0
    except RpcException as e:
        current_command.exit_code = 3
        code = e.rpc_error_code
        msg = e.rpc_error_payload
        if not msg:
            msg = DEFAULT_RPC_ERROR_MESSAGE.get(code)
        elif isinstance(msg, bytes):
            msg = msg.decode()
        err(f'{msg} (code: {code})')
        if current_command.debug:
            print_tb(force=True)
    except Exception as e:
        current_command.exit_code = 1
        err(f'{e.__class__.__name__}: {e}')
        if current_command.debug:
            print_tb(force=True)
    current_command.debug = False


class Parser(icli.ArgumentParser):

    def get_interactive_prompt(self):
        try:
            color = 'yellow'
            name = call_rpc('test')['system_name']
            banner = colored(f'eva.4:{name}', color=color)
        except:
            color = 'grey'
            banner = colored('eva.4', color=color)
        if self.current_section:
            return '[{}/{}]# '.format(
                banner, colored("".join(self.current_section), color=color))
        else:
            return f'[{banner}]# '

    def print_global_help(self):
        print('cls - clear screen')
        print('date - print system date/time')
        print('sh - enter system shell')
        print('top - processes')
        print('uptime - system uptime')
        print('w - who is logged in')
        print()

    def handle_interactive_exception(self):
        import traceback
        err(traceback.format_exc())


def append_registry_cli(root_sp):
    ap = root_sp.add_parser('registry', help='registry commands')
    sp = ap.add_subparsers(dest='_subc', help='sub command')

    p = sp.add_parser('manage', help='manage EVA ICS registry')


def append_server_cli(root_sp):
    ap = root_sp.add_parser('server', help='server commands')
    sp = ap.add_subparsers(dest='_subc', help='sub command')

    p = sp.add_parser('start', help='start the local node')
    p = sp.add_parser('stop', help='start the local node')
    p = sp.add_parser('reload', help='reload the local node')
    p = sp.add_parser('restart', help='restart the local node')
    p = sp.add_parser('launch',
                      help='launch the local node in the verbose mode')
    p = sp.add_parser('status', help='local node status')


def append_action_cli(root_sp):
    ap = root_sp.add_parser('action', help='action commands')
    sp = ap.add_subparsers(dest='_subc', help='sub command')

    p = sp.add_parser('exec', help='exec unit action')
    p.add_argument('i', metavar='OID',
                   help='unit OID').completer = ComplOIDtp('unit')
    p.add_argument('status', metavar='STATUS', type=int)
    p.add_argument('-v', '--value', metavar='VALUE')
    p.add_argument('-p', '--priority', metavar='PRIORITY', type=int)
    p.add_argument('-w',
                   '--wait',
                   metavar='SEC',
                   type=float,
                   help='wait max seconds until the action is completed')

    p = sp.add_parser('run', help='run lmacro')
    p.add_argument('i', metavar='OID',
                   help='lmacro OID').completer = ComplOIDtp('lmacro')
    p.add_argument('-a',
                   '--arg',
                   metavar='ARG',
                   action='append',
                   help='argument, can be multiple')
    p.add_argument('--kwarg',
                   metavar='KWARG',
                   action='append',
                   help='keyword argument name=value, can be multiple')
    p.add_argument('-p', '--priority', metavar='PRIORITY', type=int)
    p.add_argument('-w',
                   '--wait',
                   metavar='SEC',
                   type=float,
                   help='wait max seconds until the action is completed')

    p = sp.add_parser('toggle', help='exec unit toggle action')
    p.add_argument('i', metavar='OID',
                   help='unit OID').completer = ComplOIDtp('unit')
    p.add_argument('-p', '--priority', metavar='PRIORITY', type=int)
    p.add_argument('-w',
                   '--wait',
                   metavar='SEC',
                   type=float,
                   help='wait max seconds until the action is completed')

    p = sp.add_parser('result', help='get action result')
    p.add_argument('u', metavar='UUID', help='action UUID')

    p = sp.add_parser('terminate', help='terminate action (if possible)')
    p.add_argument('u', metavar='UUID', help='action UUID')

    p = sp.add_parser(
        'kill', help='cancel/terminate all actions for the item (if possible)')
    p.add_argument('i', metavar='OID',
                   help='unit OID').completer = ComplOIDtp('unit')

    p = sp.add_parser('list', help='list recent actions')
    p.add_argument('-i', '--oid', metavar='OID',
                   help='filter by OID').completer = ComplOID()
    p.add_argument(
        '-q',
        '--status-query',
        help='filter by status',
        choices=['waiting', 'running', 'completed', 'failed', 'finished'])
    p.add_argument('-s', '--svc', metavar='SVC',
                   help='filter by service').completer = ComplSvc()
    p.add_argument('-t',
                   '--time',
                   metavar='SEC',
                   type=int,
                   help='get actions for the last SEC seconds')
    p.add_argument('-n',
                   '--limit',
                   metavar='LIMIT',
                   type=int,
                   help='limit action list to')


def append_broker_cli(root_sp):
    ap = root_sp.add_parser('broker', help='bus broker commands')
    sp = ap.add_subparsers(dest='_subc', help='sub command')

    sp.add_parser('client.list', help='list registered bus clients')
    sp.add_parser('test', help='test broker')
    sp.add_parser('info', help='broker info')
    sp.add_parser('stats', help='broker stats')


def append_svc_cli(root_sp):
    ap = root_sp.add_parser('svc', help='service commands')
    sp = ap.add_subparsers(dest='_subc', help='sub command')
    sp.add_parser('list', help='list services')

    p = sp.add_parser('restart', help='restart service')
    p.add_argument('i', metavar='SVC').completer = ComplSvc()

    p = sp.add_parser('edit', help='edit service config')
    p.add_argument('i', metavar='SVC').completer = ComplSvc()

    p = sp.add_parser('enable', help='enable service')
    p.add_argument('i', metavar='SVC').completer = ComplSvc()

    p = sp.add_parser('disable', help='disable service')
    p.add_argument('i', metavar='SVC').completer = ComplSvc()

    p = sp.add_parser('test', help='test service')
    p.add_argument('i', metavar='SVC').completer = ComplSvc()

    p = sp.add_parser('info', help='get service info')
    p.add_argument('i', metavar='SVC').completer = ComplSvc()

    p = sp.add_parser('call', help='perform RPC call to the service')
    p.add_argument('i', metavar='SVC').completer = ComplSvc()
    p.add_argument('-f',
                   '--file',
                   metavar='FILE',
                   help='read call params payload form the file'
                  ).completer = ComplYamlFile()
    p.add_argument('method', metavar='METHOD').completer = ComplSvcRpcMethod()
    p.add_argument('params',
                   nargs='*',
                   help='param=value',
                   metavar='PARAM=VALUE').completer = ComplSvcRpcParams()

    p = sp.add_parser('export', help='export service(s) to a deployment file')
    p.add_argument('i', metavar='MASK').completer = ComplSvc()
    p.add_argument('-o', '--output', metavar='FILE',
                   help='output file').completer = ComplYamlFile()

    p = sp.add_parser('deploy', help='deploy service(s) from a deployment file')
    p.add_argument('-f', '--file', metavar='FILE',
                   help='deployment file').completer = ComplYamlFile()

    p = sp.add_parser('undeploy',
                      help='undeploy service(s) using a deployment file')
    p.add_argument('-f', '--file', metavar='FILE',
                   help='deployment file').completer = ComplYamlFile()

    p = sp.add_parser('create', help='create service from the template config')
    p.add_argument('i', metavar='SVC', help='service id').completer = ComplSvc()
    p.add_argument('f', metavar='FILE',
                   help='configuration template').completer = ComplYamlFile()

    p = sp.add_parser('destroy', help='destroy service')
    p.add_argument('i', metavar='SVC').completer = ComplSvc()

    p = sp.add_parser(
        'purge', help='purge service (destroy and delete all service data)')
    p.add_argument('i', metavar='SVC').completer = ComplSvc()


def append_acl_cli(root_sp):
    ap = root_sp.add_parser('acl', help='ACL commands')
    sp = ap.add_subparsers(dest='_subc', help='sub command')
    p = sp.add_parser('list', help='list ACLs')
    p.add_argument('-a',
                   '--acl-svc',
                   help=f'ACL service (default: {DEFAULT_ACL_SERVICE})',
                   default=DEFAULT_ACL_SERVICE).completer = ComplSvc('aaa')

    p = sp.add_parser('edit', help='edit ACL')
    p.add_argument('i', metavar='ACL')
    p.add_argument('-a',
                   '--acl-svc',
                   help=f'ACL service (default: {DEFAULT_ACL_SERVICE})',
                   default=DEFAULT_ACL_SERVICE).completer = ComplSvc('aaa')

    p = sp.add_parser('export', help='export ACLs(s) to a deployment file')
    p.add_argument('i', metavar='MASK')
    p.add_argument('-a',
                   '--acl-svc',
                   help=f'ACL service (default: {DEFAULT_ACL_SERVICE})',
                   default=DEFAULT_ACL_SERVICE).completer = ComplSvc('aaa')
    p.add_argument('-o', '--output', metavar='FILE',
                   help='output file').completer = ComplYamlFile()

    p = sp.add_parser('deploy', help='deploy ACL(s) from a deployment file')
    p.add_argument('-a',
                   '--acl-svc',
                   help=f'ACL service (default: {DEFAULT_ACL_SERVICE})',
                   default=DEFAULT_ACL_SERVICE).completer = ComplSvc('aaa')
    p.add_argument('-f', '--file', metavar='FILE',
                   help='deployment file').completer = ComplYamlFile()

    p = sp.add_parser('undeploy',
                      help='undeploy ACL(s) using a deployment file')
    p.add_argument('-a',
                   '--acl-svc',
                   help=f'ACL service (default: {DEFAULT_ACL_SERVICE})',
                   default=DEFAULT_ACL_SERVICE).completer = ComplSvc('aaa')
    p.add_argument('-f', '--file', metavar='FILE',
                   help='deployment file').completer = ComplYamlFile()

    p = sp.add_parser('create', help='create ACL')
    p.add_argument('i', metavar='ACL', help='ACL id')
    p.add_argument('-a',
                   '--acl-svc',
                   help=f'ACL service (default: {DEFAULT_ACL_SERVICE})',
                   default=DEFAULT_ACL_SERVICE).completer = ComplSvc('aaa')

    p = sp.add_parser('destroy', help='destroy ACL')
    p.add_argument('i', metavar='ACL')
    p.add_argument('-a',
                   '--acl-svc',
                   help=f'ACL service (default: {DEFAULT_ACL_SERVICE})',
                   default=DEFAULT_ACL_SERVICE).completer = ComplSvc('aaa')


def append_key_cli(root_sp):
    ap = root_sp.add_parser('key', help='API key commands')
    sp = ap.add_subparsers(dest='_subc', help='sub command')
    p = sp.add_parser('list', help='list API keys')
    p.add_argument(
        '-a',
        '--auth-svc',
        help=f'Authentication service (default: {DEFAULT_AUTH_SERVICE})',
        default=DEFAULT_AUTH_SERVICE).completer = ComplSvc('aaa')

    p = sp.add_parser('get', help='get API key data (including the key field)')
    p.add_argument('i', metavar='API key')
    p.add_argument(
        '-a',
        '--auth-svc',
        help=f'Authentication service (default: {DEFAULT_AUTH_SERVICE})',
        default=DEFAULT_AUTH_SERVICE).completer = ComplSvc('aaa')

    p = sp.add_parser('edit', help='edit API key')
    p.add_argument('i', metavar='API key')
    p.add_argument(
        '-a',
        '--auth-svc',
        help=f'Authentication service (default: {DEFAULT_AUTH_SERVICE})',
        default=DEFAULT_AUTH_SERVICE).completer = ComplSvc('aaa')

    p = sp.add_parser('export', help='export API keys(s) to a deployment file')
    p.add_argument('i', metavar='MASK')
    p.add_argument(
        '-a',
        '--auth-svc',
        help=f'Authentication service (default: {DEFAULT_AUTH_SERVICE})',
        default=DEFAULT_AUTH_SERVICE).completer = ComplSvc('aaa')
    p.add_argument('-o', '--output', metavar='FILE',
                   help='output file').completer = ComplYamlFile()

    p = sp.add_parser('deploy', help='deploy API key(s) from a deployment file')
    p.add_argument(
        '-a',
        '--auth-svc',
        help=f'Authentication service (default: {DEFAULT_AUTH_SERVICE})',
        default=DEFAULT_AUTH_SERVICE).completer = ComplSvc('aaa')
    p.add_argument('-f', '--file', metavar='FILE',
                   help='deployment file').completer = ComplYamlFile()

    p = sp.add_parser('undeploy',
                      help='undeploy API key(s) using a deployment file')
    p.add_argument(
        '-a',
        '--auth-svc',
        help=f'Authentication service (default: {DEFAULT_AUTH_SERVICE})',
        default=DEFAULT_AUTH_SERVICE).completer = ComplSvc('aaa')
    p.add_argument('-f', '--file', metavar='FILE',
                   help='deployment file').completer = ComplYamlFile()

    p = sp.add_parser('create', help='create API key')
    p.add_argument('i', metavar='API key', help='API key id')
    p.add_argument(
        '-a',
        '--auth-svc',
        help=f'Authentication service (default: {DEFAULT_AUTH_SERVICE})',
        default=DEFAULT_AUTH_SERVICE).completer = ComplSvc('aaa')

    p = sp.add_parser('destroy', help='destroy API key')
    p.add_argument('i', metavar='API key')
    p.add_argument(
        '-a',
        '--auth-svc',
        help=f'Authentication service (default: {DEFAULT_AUTH_SERVICE})',
        default=DEFAULT_AUTH_SERVICE).completer = ComplSvc('aaa')

    p = sp.add_parser('regenerate', help='re-generate API key')
    p.add_argument('i', metavar='API key', help='API key id')
    p.add_argument(
        '-a',
        '--auth-svc',
        help=f'Authentication service (default: {DEFAULT_AUTH_SERVICE})',
        default=DEFAULT_AUTH_SERVICE).completer = ComplSvc('aaa')


def append_user_cli(root_sp):
    ap = root_sp.add_parser('user', help='user commands')
    sp = ap.add_subparsers(dest='_subc', help='sub command')
    p = sp.add_parser('list', help='list users')
    p.add_argument(
        '-a',
        '--auth-svc',
        help=f'Authentication service (default: {DEFAULT_AUTH_SERVICE})',
        default=DEFAULT_AUTH_SERVICE).completer = ComplSvc('aaa')

    p = sp.add_parser('get', help='get user data')
    p.add_argument('i', metavar='user')
    p.add_argument(
        '-a',
        '--auth-svc',
        help=f'Authentication service (default: {DEFAULT_AUTH_SERVICE})',
        default=DEFAULT_AUTH_SERVICE).completer = ComplSvc('aaa')

    p = sp.add_parser('edit', help='edit user')
    p.add_argument('i', metavar='user')
    p.add_argument(
        '-a',
        '--auth-svc',
        help=f'Authentication service (default: {DEFAULT_AUTH_SERVICE})',
        default=DEFAULT_AUTH_SERVICE).completer = ComplSvc('aaa')

    p = sp.add_parser('export', help='export users(s) to a deployment file')
    p.add_argument('i', metavar='MASK')
    p.add_argument(
        '-a',
        '--auth-svc',
        help=f'Authentication service (default: {DEFAULT_AUTH_SERVICE})',
        default=DEFAULT_AUTH_SERVICE).completer = ComplSvc('aaa')
    p.add_argument('-o', '--output', metavar='FILE',
                   help='output file').completer = ComplYamlFile()

    p = sp.add_parser('deploy', help='deploy user(s) from a deployment file')
    p.add_argument(
        '-a',
        '--auth-svc',
        help=f'Authentication service (default: {DEFAULT_AUTH_SERVICE})',
        default=DEFAULT_AUTH_SERVICE).completer = ComplSvc('aaa')
    p.add_argument('-f', '--file', metavar='FILE',
                   help='deployment file').completer = ComplYamlFile()

    p = sp.add_parser('undeploy',
                      help='undeploy user(s) using a deployment file')
    p.add_argument(
        '-a',
        '--auth-svc',
        help=f'Authentication service (default: {DEFAULT_AUTH_SERVICE})',
        default=DEFAULT_AUTH_SERVICE).completer = ComplSvc('aaa')
    p.add_argument('-f', '--file', metavar='FILE',
                   help='deployment file').completer = ComplYamlFile()

    p = sp.add_parser('create', help='create user')
    p.add_argument('i', metavar='user', help='user id')
    p.add_argument(
        '-a',
        '--auth-svc',
        help=f'Authentication service (default: {DEFAULT_AUTH_SERVICE})',
        default=DEFAULT_AUTH_SERVICE).completer = ComplSvc('aaa')

    p = sp.add_parser('destroy', help='destroy user')
    p.add_argument('i', metavar='user')
    p.add_argument(
        '-a',
        '--auth-svc',
        help=f'Authentication service (default: {DEFAULT_AUTH_SERVICE})',
        default=DEFAULT_AUTH_SERVICE).completer = ComplSvc('aaa')

    p = sp.add_parser('password', help='change user\'s password')
    p.add_argument('i', metavar='user', help='user id')
    p.add_argument(
        '-a',
        '--auth-svc',
        help=f'Authentication service (default: {DEFAULT_AUTH_SERVICE})',
        default=DEFAULT_AUTH_SERVICE).completer = ComplSvc('aaa')


def append_item_cli(root_sp):
    ap = root_sp.add_parser('item', help='item commands')
    sp = ap.add_subparsers(dest='_subc', help='sub command')

    p = sp.add_parser(
        'announce',
        help='forcibly announce current item states via the local bus')
    p.add_argument('i', metavar='MASK').completer = ComplOID('state')
    p.add_argument('-n', metavar='NODE').completer = ComplNode()

    p = sp.add_parser('list', help='list items')
    p.add_argument('i', metavar='MASK').completer = ComplOID()
    p.add_argument('-n', metavar='NODE').completer = ComplNode()

    p = sp.add_parser('state', help='item states')
    p.add_argument('i', metavar='MASK').completer = ComplOID('state')
    p.add_argument('-y', '--full', action='store_true')

    p = sp.add_parser('history', help='item state history')
    p.add_argument('i', metavar='OID').completer = ComplOID('state')
    p.add_argument('-a',
                   '--db-svc',
                   help=f'database service (default: {DEFAULT_DB_SERVICE})',
                   metavar='SVC',
                   default=DEFAULT_DB_SERVICE).completer = ComplSvc('db')
    p.add_argument('-s', '--time-start', metavar='TIME', help='start time')
    p.add_argument('-e', '--time-end', metavar='TIME', help='end time')
    p.add_argument('-z',
                   '--time-zone',
                   metavar='ZONE',
                   help='time zone (pytz, e.g. UTC or Europe/Prague)')
    p.add_argument('-n',
                   '--limit',
                   metavar='LIMIT',
                   type=int,
                   help='limit records to')
    p.add_argument('-x',
                   '--prop',
                   metavar='PROP',
                   choices=['status', 'value'],
                   help='item state prop (status/value)')
    p.add_argument(
        '-w',
        '--fill',
        metavar='INTERVAL',
        help='fill (e.g. 1T - 1 min, 2H - 2 hours), requires start time,'
        ' value precision can be specified as e.g. 1T:2 '
        'for 2 digits after comma')

    p = sp.add_parser('slog', help='item state log')
    p.add_argument('i', metavar='OID').completer = ComplOID()
    p.add_argument('-a',
                   '--db-svc',
                   help=f'database service (default: {DEFAULT_DB_SERVICE})',
                   metavar='SVC',
                   default=DEFAULT_DB_SERVICE).completer = ComplSvc('db')
    p.add_argument('-s', '--time-start', metavar='TIME', help='start time')
    p.add_argument('-e', '--time-end', metavar='TIME', help='end time')
    p.add_argument('-z',
                   '--time-zone',
                   metavar='ZONE',
                   help='time zone (pytz, e.g. UTC or Europe/Prague)')
    p.add_argument('-n',
                   '--limit',
                   metavar='LIMIT',
                   type=int,
                   help='limit records to')

    p = sp.add_parser('create', help='create an item')
    p.add_argument('i', metavar='OID').completer = ComplOID()

    p = sp.add_parser('destroy', help='destroy item(s)')
    p.add_argument('i', metavar='MASK').completer = ComplOID()

    p = sp.add_parser('enable', help='enable item(s)')
    p.add_argument('i', metavar='MASK').completer = ComplOID()

    p = sp.add_parser('disable', help='disable item(s)')
    p.add_argument('i', metavar='MASK').completer = ComplOID()

    p = sp.add_parser('edit', help='edit item config')
    p.add_argument('i', metavar='OID').completer = ComplOID()

    p = sp.add_parser('set', help='forcibly set item state')
    p.add_argument('i', metavar='OID').completer = ComplOID()
    p.add_argument('status', metavar='STATUS', type=int)
    p.add_argument('-v', '--value', metavar='VALUE')

    sp.add_parser('summary', help='item summary per source')

    p = sp.add_parser('export', help='export item(s) to a deployment file')
    p.add_argument('i', metavar='MASK').completer = ComplOID()
    p.add_argument('-o', '--output', metavar='FILE',
                   help='output file').completer = ComplYamlFile()

    p = sp.add_parser('deploy', help='deploy item(s) from a deployment file')
    p.add_argument('-f', '--file', metavar='FILE',
                   help='deployment file').completer = ComplYamlFile()

    p = sp.add_parser('undeploy',
                      help='undeploy item(s) using a deployment file')
    p.add_argument('-f', '--file', metavar='FILE',
                   help='deployment file').completer = ComplYamlFile()

    p = sp.add_parser('watch', help='Watch item state')
    p.add_argument('i', metavar='OID').completer = ComplOID()
    p.add_argument('-n',
                   '--interval',
                   help='Watch interval (default: 1s)',
                   metavar='SEC',
                   default=1,
                   type=float)
    p.add_argument('-r', '--rows', help='Rows to plot', metavar='NUM', type=int)
    p.add_argument('-x',
                   '--prop',
                   help='State prop to use (default: value)',
                   choices=['status', 'value'],
                   metavar='PROP',
                   default='value')
    p.add_argument('-p',
                   '--chart-type',
                   help='Chart type',
                   choices=['bar', 'line'],
                   default='bar')


def append_lvar_cli(root_sp):
    ap = root_sp.add_parser('lvar', help='lvar commands')
    sp = ap.add_subparsers(dest='_subc', help='sub command')

    p = sp.add_parser('set', help='set lvar state')
    p.add_argument('i', metavar='OID').completer = ComplOIDtp('lvar')
    p.add_argument('status', metavar='STATUS', nargs='?', type=int)
    p.add_argument('-v', '--value', metavar='VALUE')

    p = sp.add_parser('reset', help='reset lvar state')
    p.add_argument('i', metavar='OID').completer = ComplOIDtp('lvar')

    p = sp.add_parser('clear', help='clear lvar state')
    p.add_argument('i', metavar='OID').completer = ComplOIDtp('lvar')

    p = sp.add_parser('toggle', help='toggle lvar state')
    p.add_argument('i', metavar='OID').completer = ComplOIDtp('lvar')

    p = sp.add_parser('incr', help='increment lvar value')
    p.add_argument('i', metavar='OID').completer = ComplOIDtp('lvar')

    p = sp.add_parser('decr', help='decrement lvar value')
    p.add_argument('i', metavar='OID').completer = ComplOIDtp('lvar')


def append_log_cli(root_sp):
    ap = root_sp.add_parser('log', help='log commands')
    sp = ap.add_subparsers(dest='_subc', help='sub command')

    sp.add_parser('purge', help='purge memory log')

    p = sp.add_parser('get', help='get memory log records')
    p.add_argument('level',
                   help='Log level',
                   nargs='?',
                   choices=[
                       'trace', 't', 'debug', 'd', 'info', 'i', 'warn', 'w',
                       'error', 'e'
                   ])
    p.add_argument('-t',
                   '--time',
                   metavar='SEC',
                   type=int,
                   help='get records for the last SEC seconds')
    p.add_argument('-n',
                   '--limit',
                   metavar='LIMIT',
                   type=int,
                   help='limit records to')
    p.add_argument('-m', '--module', metavar='MOD', help='filter by module')
    p.add_argument('-x', '--regex', metavar='REGEX', help='filter by regex')
    p.add_argument('-y',
                   '--full',
                   action='store_true',
                   help='display full log records')
    # p.add_argument('-f',
    # '--follow',
    # action='store_true',
    # help='follow log until C-c')


def append_node_cli(root_sp):
    ap = root_sp.add_parser('node', help='node commands')
    sp = ap.add_subparsers(dest='_subc', help='sub command')

    p = sp.add_parser('list', help='list nodes')
    p.add_argument('-a',
                   '--repl-svc',
                   help='get list from a replication service',
                   metavar='SVC').completer = ComplSvc('repl')
    p.add_argument(
        '-s',
        dest='default_rpl',
        action='store_true',
        help=
        f'get list from the default replication service {DEFAULT_REPL_SERVICE}')

    p = sp.add_parser('append', help='append node')
    p.add_argument('i', metavar='NAME')
    p.add_argument(
        '-a',
        '--repl-svc',
        help=f'use a replication service (default: {DEFAULT_REPL_SERVICE})',
        default=DEFAULT_REPL_SERVICE,
        metavar='SVC').completer = ComplSvc('repl')

    p = sp.add_parser('reload', help='reload node')
    p.add_argument('i', metavar='NAME').completer = ComplNode()
    p.add_argument(
        '-a',
        '--repl-svc',
        help=f'use a replication service (default: {DEFAULT_REPL_SERVICE})',
        default=DEFAULT_REPL_SERVICE,
        metavar='SVC').completer = ComplSvc('repl')

    p = sp.add_parser('edit', help='edit node config')
    p.add_argument('i', metavar='OID').completer = ComplNode()
    p.add_argument(
        '-a',
        '--repl-svc',
        help=f'use a replication service (default: {DEFAULT_REPL_SERVICE})',
        default=DEFAULT_REPL_SERVICE,
        metavar='SVC').completer = ComplSvc('repl')

    p = sp.add_parser('export', help='export node(s) to a deployment file')
    p.add_argument('i', metavar='NODE',
                   help="* to export all nodes").completer = ComplNode()
    p.add_argument(
        '-a',
        '--repl-svc',
        help=f'use a replication service (default: {DEFAULT_REPL_SERVICE})',
        default=DEFAULT_REPL_SERVICE,
        metavar='SVC').completer = ComplSvc('repl')
    p.add_argument('-o', '--output', metavar='FILE',
                   help='output file').completer = ComplYamlFile()

    p = sp.add_parser('deploy', help='deploy node(s) from a deployment file')
    p.add_argument('-f', '--file', metavar='FILE',
                   help='deployment file').completer = ComplYamlFile()
    p.add_argument(
        '-a',
        '--repl-svc',
        help=f'use a replication service (default: {DEFAULT_REPL_SERVICE})',
        default=DEFAULT_REPL_SERVICE,
        metavar='SVC').completer = ComplSvc('repl')

    p = sp.add_parser('undeploy',
                      help='undeploy node(s) using a deployment file')
    p.add_argument('-f', '--file', metavar='FILE',
                   help='deployment file').completer = ComplYamlFile()
    p.add_argument(
        '-a',
        '--repl-svc',
        help=f'use a replication service (default: {DEFAULT_REPL_SERVICE})',
        default=DEFAULT_REPL_SERVICE,
        metavar='SVC').completer = ComplSvc('repl')

    p = sp.add_parser('test', help='test node')
    p.add_argument('i', metavar='NAME').completer = ComplNode()
    p.add_argument(
        '-a',
        '--repl-svc',
        help=f'use a replication service (default: {DEFAULT_REPL_SERVICE})',
        default=DEFAULT_REPL_SERVICE,
        metavar='SVC').completer = ComplSvc('repl')

    p = sp.add_parser('mtest', help='test management functions for node')
    p.add_argument('i', metavar='NAME').completer = ComplNode()
    p.add_argument(
        '-a',
        '--repl-svc',
        help=f'use a replication service (default: {DEFAULT_REPL_SERVICE})',
        default=DEFAULT_REPL_SERVICE,
        metavar='SVC').completer = ComplSvc('repl')

    p = sp.add_parser('remove', help='remove node')
    p.add_argument('i', metavar='NAME').completer = ComplNode()
    p.add_argument(
        '-a',
        '--repl-svc',
        help=f'use a replication service (default: {DEFAULT_REPL_SERVICE})',
        default=DEFAULT_REPL_SERVICE,
        metavar='SVC').completer = ComplSvc('repl')


def append_spoint_cli(root_sp):
    ap = root_sp.add_parser('spoint', help='spoint commands')
    sp = ap.add_subparsers(dest='_subc', help='sub command')

    p = sp.add_parser('list', help='list nodes')


def append_venv_cli(root_sp):
    ap = root_sp.add_parser('venv', help='manage Python venv')
    sp = ap.add_subparsers(dest='_subc', help='sub command')

    p = sp.add_parser('build')
    p.add_argument('-S', '--from-scratch', action='store_true')

    sp.add_parser('list')

    sp.add_parser('edit')

    sp.add_parser('config')

    p = sp.add_parser('add')
    p.add_argument('modules', metavar='MODULE', nargs='+')
    p.add_argument('-B',
                   '--rebuild',
                   action='store_true',
                   help='automatically rebuild venv')

    p = sp.add_parser('remove')
    p.add_argument('modules', metavar='MODULE', nargs='+')
    p.add_argument('-B',
                   '--rebuild',
                   action='store_true',
                   help='automatically rebuild venv')

    p = sp.add_parser('update')
    p.add_argument('modules', metavar='MODULE', nargs='+')
    p.add_argument('-B',
                   '--rebuild',
                   action='store_true',
                   help='automatically rebuild venv')

    mirror_dir = f'{common.dir_eva}/mirror'
    p = sp.add_parser('mirror-update', help='Create/update PyPi mirror')
    p.add_argument('--dest',
                   metavar='DIR',
                   help=f'Mirror directory (default: {mirror_dir})',
                   default=mirror_dir)

    p = sp.add_parser('mirror-set', help='Set PyPi mirror URL')
    p.add_argument('mirror_url',
                   metavar='URL',
                   help=('EVA ICS v4 mirror url as http://<ip/host>:port'
                         ' or "default" to restore the default settings'))


def append_cloud_cli(root_sp):
    ap = root_sp.add_parser('cloud', help='cloud manager commands')
    sp = ap.add_subparsers(dest='_subc', help='sub command')

    p = sp.add_parser('deploy', help='cloud deploy')
    p.add_argument('file', metavar='FILE',
                   help='deployment file').completer = ComplYamlFile()
    p.add_argument('-c',
                   '--config-var',
                   metavar='VAR',
                   action='append',
                   help='config var name=value')
    p.add_argument('--config',
                   metavar='FILE',
                   help='load configuration variables from YAML file'
                  ).completer = ComplYamlFile()
    p.add_argument('--test',
                   action='store_true',
                   help=f'test deployment config and exit')

    p = sp.add_parser('undeploy', help='cloud undeploy')
    p.add_argument('file', metavar='FILE',
                   help='deployment file').completer = ComplYamlFile()
    p.add_argument('-c',
                   '--config-var',
                   metavar='VAR',
                   action='append',
                   help='config var name=value')
    p.add_argument('--config',
                   metavar='FILE',
                   help='load configuration variables from YAML file'
                  ).completer = ComplYamlFile()
    p.add_argument('--test',
                   action='store_true',
                   help=f'test deployment config and exit')

    p = sp.add_parser('update', help='update cloud nodes')
    p.add_argument('nodes',
                   metavar='NODE',
                   nargs='*',
                   help="Node name or node/spoint").completer = ComplNode()
    p.add_argument('--check-timeout',
                   help='Max node update duration (default: 120 sec)',
                   default=120,
                   metavar='SEC',
                   type=float)
    p.add_argument('--all', help='update all nodes', action='store_true')
    p.add_argument('-i',
                   '--info-only',
                   help='display the update plan and exit',
                   action='store_true')
    p.add_argument('--YES',
                   dest='yes',
                   help='update without any confirmations',
                   action='store_true')


def append_system_cli(root_sp):
    ap = root_sp.add_parser('system', help='system commands')
    sp = ap.add_subparsers(dest='_subc', help='sub command')

    p = sp.add_parser('update', help='update the system')
    p = sp.add_parser('reboot', help='reboot the system')
    p = sp.add_parser('poweroff', help='power off the system')


def append_mirror_cli(root_sp):
    ap = root_sp.add_parser('mirror', help='mirror commands')
    sp = ap.add_subparsers(dest='_subc', help='sub command')

    mirror_dir = f'{common.dir_eva}/mirror'
    p = sp.add_parser('update', help='update the mirror')
    p.add_argument('--dest',
                   metavar='DIR',
                   help=f'Mirror directory (default: {mirror_dir})',
                   default=mirror_dir)
    p.add_argument('-o',
                   '--current-arch-only',
                   help='download files for the current CPU architecture only',
                   action='store_true')
    p.add_argument('--force',
                   help='force download existing files',
                   action='store_true')
    p.add_argument('-u',
                   '--repository-url',
                   metavar='URL',
                   help='repository url')

    p = sp.add_parser('info', help='get mirror info')

    p = sp.add_parser(
        'set', help='Set mirror URL (do not run this on the mirror host node)')
    p.add_argument('mirror_url',
                   metavar='URL',
                   help=('EVA ICS v4 mirror url as http://<ip/host>:port'
                         ' or "default" to restore the default settings'))


def sys_cmd(cmd):
    if cmd == 'top':
        import distutils.spawn
        top_cmd = distutils.spawn.find_executable('htop')
        if not top_cmd:
            top_cmd = cmd
        os.system(top_cmd)
    elif cmd == 'cls':
        os.system('clear')
    elif cmd == 'sh':
        print('Executing system shell')
        sh_cmd = os.getenv('SHELL')
        if sh_cmd is None:
            import distutils.spawn
            sh_cmd = distutils.spawn.find_executable('bash')
            if not sh_cmd:
                sh_cmd = 'sh'
        os.system(sh_cmd)
    else:
        os.system(cmd)


def append_kiosk_cli(root_sp):
    ap = root_sp.add_parser('kiosk', help='kiosk commands')
    sp = ap.add_subparsers(dest='_subc', help='sub command')

    p = sp.add_parser('login', help='login kiosk')
    p.add_argument('i', metavar='kiosk')
    p.add_argument(
        '-a',
        '--kiosk-svc',
        help=f'kiosk service (default: {DEFAULT_KIOSK_SERVICE})',
        default=DEFAULT_KIOSK_SERVICE).completer = ComplSvc('kioskman')

    p = sp.add_parser('logout', help='logout kiosk')
    p.add_argument('i', metavar='kiosk')
    p.add_argument(
        '-a',
        '--kiosk-svc',
        help=f'kiosk service (default: {DEFAULT_KIOSK_SERVICE})',
        default=DEFAULT_KIOSK_SERVICE).completer = ComplSvc('kioskman')

    p = sp.add_parser('test', help='test kiosk')
    p.add_argument('i', metavar='kiosk')
    p.add_argument(
        '-a',
        '--kiosk-svc',
        help=f'kiosk service (default: {DEFAULT_KIOSK_SERVICE})',
        default=DEFAULT_KIOSK_SERVICE).completer = ComplSvc('kioskman')

    p = sp.add_parser('info', help='kiosk info')
    p.add_argument('i', metavar='kiosk')
    p.add_argument(
        '-a',
        '--kiosk-svc',
        help=f'kiosk service (default: {DEFAULT_KIOSK_SERVICE})',
        default=DEFAULT_KIOSK_SERVICE).completer = ComplSvc('kioskman')

    p = sp.add_parser('alert', help='display alert on kiosk')
    p.add_argument('i', metavar='kiosk')
    p.add_argument('text', metavar='TEXT')
    p.add_argument('-l', '--level', choices=['info', 'warning'])
    p.add_argument('--timeout', metavar='SEC', dest='t', type=int)
    p.add_argument(
        '-a',
        '--kiosk-svc',
        help=f'kiosk service (default: {DEFAULT_KIOSK_SERVICE})',
        default=DEFAULT_KIOSK_SERVICE).completer = ComplSvc('kioskman')

    p = sp.add_parser('eval', help='execute JavaScript code on kiosk')
    p.add_argument('i', metavar='kiosk')
    p.add_argument('code', metavar='CODE')
    p.add_argument(
        '-a',
        '--kiosk-svc',
        help=f'kiosk service (default: {DEFAULT_KIOSK_SERVICE})',
        default=DEFAULT_KIOSK_SERVICE).completer = ComplSvc('kioskman')

    p = sp.add_parser('navigate', help='navigate kiosk to a page')
    p.add_argument('i', metavar='kiosk')
    p.add_argument('url', metavar='URL', nargs='?')
    p.add_argument(
        '-a',
        '--kiosk-svc',
        help=f'kiosk service (default: {DEFAULT_KIOSK_SERVICE})',
        default=DEFAULT_KIOSK_SERVICE).completer = ComplSvc('kioskman')

    p = sp.add_parser('display', help='control kiosk display')
    p.add_argument('i', metavar='kiosk')
    p.add_argument('--off', action='store_true')
    p.add_argument('--on', action='store_true')
    p.add_argument('--brightness', metavar='LEVEL', type=int)
    p.add_argument(
        '-a',
        '--kiosk-svc',
        help=f'kiosk service (default: {DEFAULT_KIOSK_SERVICE})',
        default=DEFAULT_KIOSK_SERVICE).completer = ComplSvc('kioskman')

    p = sp.add_parser('zoom', help='change kiosk zoom level')
    p.add_argument('i', metavar='kiosk')
    p.add_argument('level', metavar='LEVEL', type=float)
    p.add_argument(
        '-a',
        '--kiosk-svc',
        help=f'kiosk service (default: {DEFAULT_KIOSK_SERVICE})',
        default=DEFAULT_KIOSK_SERVICE).completer = ComplSvc('kioskman')

    p = sp.add_parser('reload', help='reload kiosk browser')
    p.add_argument('i', metavar='kiosk')
    p.add_argument(
        '-a',
        '--kiosk-svc',
        help=f'kiosk service (default: {DEFAULT_KIOSK_SERVICE})',
        default=DEFAULT_KIOSK_SERVICE).completer = ComplSvc('kioskman')

    p = sp.add_parser('reboot', help='reboot kiosk OS')
    p.add_argument('i', metavar='kiosk')
    p.add_argument(
        '-a',
        '--kiosk-svc',
        help=f'kiosk service (default: {DEFAULT_KIOSK_SERVICE})',
        default=DEFAULT_KIOSK_SERVICE).completer = ComplSvc('kioskman')

    p = sp.add_parser('dev_open', help='open kiosk development console')
    p.add_argument('i', metavar='kiosk')
    p.add_argument(
        '-a',
        '--kiosk-svc',
        help=f'kiosk service (default: {DEFAULT_KIOSK_SERVICE})',
        default=DEFAULT_KIOSK_SERVICE).completer = ComplSvc('kioskman')

    p = sp.add_parser('dev_close', help='close kiosk development console')
    p.add_argument('i', metavar='kiosk')
    p.add_argument(
        '-a',
        '--kiosk-svc',
        help=f'kiosk service (default: {DEFAULT_KIOSK_SERVICE})',
        default=DEFAULT_KIOSK_SERVICE).completer = ComplSvc('kioskman')

    p = sp.add_parser('list', help='list kiosks')
    p.add_argument(
        '-a',
        '--kiosk-svc',
        help=f'kiosk service (default: {DEFAULT_KIOSK_SERVICE})',
        default=DEFAULT_KIOSK_SERVICE).completer = ComplSvc('kioskman')

    p = sp.add_parser('edit', help='edit kiosk')
    p.add_argument('i', metavar='kiosk')
    p.add_argument(
        '-a',
        '--kiosk-svc',
        help=f'kiosk service (default: {DEFAULT_KIOSK_SERVICE})',
        default=DEFAULT_KIOSK_SERVICE).completer = ComplSvc('kioskman')

    p = sp.add_parser('export', help='export kiosks(s) to a deployment file')
    p.add_argument('i', metavar='MASK')
    p.add_argument(
        '-a',
        '--kiosk-svc',
        help=f'kiosk service (default: {DEFAULT_KIOSK_SERVICE})',
        default=DEFAULT_KIOSK_SERVICE).completer = ComplSvc('kioskman')
    p.add_argument('-o', '--output', metavar='FILE',
                   help='output file').completer = ComplYamlFile()

    p = sp.add_parser('deploy', help='deploy kiosk(s) from a deployment file')
    p.add_argument(
        '-a',
        '--kiosk-svc',
        help=f'kiosk service (default: {DEFAULT_KIOSK_SERVICE})',
        default=DEFAULT_KIOSK_SERVICE).completer = ComplSvc('kioskman')
    p.add_argument('-f', '--file', metavar='FILE',
                   help='deployment file').completer = ComplYamlFile()

    p = sp.add_parser('undeploy',
                      help='undeploy kiosk(s) using a deployment file')
    p.add_argument(
        '-a',
        '--kiosk-svc',
        help=f'kiosk service (default: {DEFAULT_KIOSK_SERVICE})',
        default=DEFAULT_KIOSK_SERVICE).completer = ComplSvc('kioskman')
    p.add_argument('-f', '--file', metavar='FILE',
                   help='deployment file').completer = ComplYamlFile()

    p = sp.add_parser('create', help='create kiosk')
    p.add_argument('i', metavar='kiosk', help='kiosk id')
    p.add_argument(
        '-a',
        '--kiosk-svc',
        help=f'kiosk service (default: {DEFAULT_KIOSK_SERVICE})',
        default=DEFAULT_KIOSK_SERVICE).completer = ComplSvc('kioskman')

    p = sp.add_parser('destroy', help='destroy kiosk')
    p.add_argument('i', metavar='kiosk')
    p.add_argument(
        '-a',
        '--kiosk-svc',
        help=f'kiosk service (default: {DEFAULT_KIOSK_SERVICE})',
        default=DEFAULT_KIOSK_SERVICE).completer = ComplSvc('kioskman')


def init_ap():
    ap = Parser()

    completer = argcomplete.CompletionFinder(
        ap, default_completer=argcomplete.completers.SuppressCompleter())
    readline.set_completer_delims('')
    readline.set_completer(completer.rl_complete)
    readline.parse_and_bind('tab: complete')

    ap.sections = {
        'action': [],
        'broker': [],
        'item': [],
        'lvar': [],
        'log': [],
        'svc': [],
        'server': [],
        'registry': [],
        'node': [],
        'cloud': [],
        'venv': [],
        'acl': [],
        'key': [],
        'user': [],
        'system': [],
        'mirror': []
    }

    ap.add_argument('-D',
                    '--debug',
                    help='debug the bus call',
                    action='store_true')
    ap.add_argument('-J', '--json', help='JSON output', action='store_true')
    ap.add_argument('-T',
                    '--timeout',
                    help='RPC timeout',
                    type=float,
                    default=5.0)

    sp = ap.add_subparsers(dest='_command', metavar='COMMAND', help='command')

    append_action_cli(sp)
    append_acl_cli(sp)
    append_broker_cli(sp)
    append_item_cli(sp)
    append_key_cli(sp)
    append_lvar_cli(sp)
    append_log_cli(sp)
    append_node_cli(sp)
    append_spoint_cli(sp)
    append_registry_cli(sp)
    append_server_cli(sp)
    append_user_cli(sp)
    append_venv_cli(sp)
    append_cloud_cli(sp)
    append_system_cli(sp)
    append_mirror_cli(sp)
    append_kiosk_cli(sp)

    sp.add_parser('save', help='save scheduled states (if instant-save is off)')
    append_svc_cli(sp)
    sp.add_parser('test', help='core test/info')

    p = sp.add_parser('dump', help='dump node info')
    p.add_argument('-s',
                   action='store_true',
                   help='create an encrypted service request')

    p = sp.add_parser('edit', help='edit the configuration keys and xc files')
    p.add_argument(
        'fname',
        metavar='CONFIG',
        help='config key to edit',
    ).completer = ComplEdit()
    p.add_argument(
        '--offline',
        action='store_true',
        help=
        'for keys: connect directly to the registry db when the node is offline'
    )

    p = sp.add_parser('update', help='update the node')
    p.add_argument('--download-timeout',
                   type=float,
                   help='update file download timeout (default: timeout*30)')
    p.add_argument('-u',
                   '--repository-url',
                   metavar='URL',
                   help='repository url')
    p.add_argument('--YES',
                   dest='yes',
                   action='store_true',
                   help='update without a confirmation')
    p.add_argument('-i',
                   '--info-only',
                   action='store_true',
                   help='get update info only')
    p.add_argument('--test',
                   action='store_true',
                   help='install a build marked as test')

    sp.add_parser('version', help='core version')

    for c in ('cls', 'date', 'sh', 'top', 'uptime', 'w'):
        ap.interactive_global_commands[c] = sys_cmd

    ap.interactive_history_file = '~/.eva4_history'
    readline.set_history_length(300)

    ap.run = dispatcher

    return ap

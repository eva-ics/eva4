import sys
import socket
import os
import stat
import time
import pytz
import neotermcolor
from neotermcolor import colored, cprint
from datetime import datetime
from rapidtables import (format_table, FORMAT_GENERATOR, FORMAT_GENERATOR_COLS,
                         MULTILINE_ALLOW)
from collections import OrderedDict

from .sharedobj import current_command, common


def check_local_shell():
    if not common.dir_eva:
        raise RuntimeError('not a local node shell')


def is_local_shell():
    return True


def ok():
    if current_command.json:
        print('null')
    else:
        print(colored('OK', color='green'))


def debug(s):
    print(colored(str(s), color='grey'))


def warn(s, delay=False):
    print(colored(str(s), color='yellow', attrs='bold'))


def err(e, delay=False):
    print(colored(str(e), color='red'))
    if delay:
        import getch
        if getch.getch() == '\x03':
            raise KeyboardInterrupt


def prepare_time(t):
    if t is None:
        return None
    import dateutil.parser
    try:
        return float(t)
    except:
        return dateutil.parser.parse(t).timestamp()


def print_tb(force=False, delay=False):
    if force:
        import traceback
        err(traceback.format_exc())
    else:
        err('FAILED')
    if delay:
        import getch
        if getch.getch() == '\x03':
            raise KeyboardInterrupt


def can_colorize():
    return os.getenv('ANSI_COLORS_DISABLED') is None and (
        not neotermcolor.tty_aware or neotermcolor._isatty)


def set_file_lock(name):
    check_local_shell()
    lock_file = f'{common.dir_eva}/var/{name}.lock'
    if os.path.exists(lock_file):
        raise RuntimeError
    else:
        with open(lock_file, 'w'):
            pass


def remove_file_lock(name):
    check_local_shell()
    lock_file = f'{common.dir_eva}/var/{name}.lock'
    try:
        os.unlink(lock_file)
    except FileNotFoundError:
        pass


def get_my_ip():
    try:
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
        s.connect(('255.255.255.255', 0))
        ip = s.getsockname()[0]
        try:
            s.close()
        except:
            pass
    except:
        ip = None
    return ip


TIME_ORD = {
    'created': 0,
    'accepted': 1,
    'pending': 0b10,
    'running': 0b1000,
    'completed': 0b1111,
    'failed': 0b10000000,
    'canceled': 0b10000001,
    'terminated': 0b10000010,
}

ACTION_STATUS_COLOR = {
    'completed': 'green',
    'canceled': 'grey',
    'terminated': 'yellow',
    'failed': 'red'
}


def xc(cmd, ps='working', verbose=False):
    import subprocess

    is_tty = can_colorize()

    CAROUSEL = ['-', '\\', '|', '/']

    def bs():
        print('\033[D', end='')

    def print_st(st, color=None):
        data = st.read()
        if data:
            try:
                cprint(data.decode(), color=color)
            except:
                cprint(data, color=color)

    if verbose:
        print(ps)
        p = subprocess.run(cmd, shell=True)
    else:
        print(ps, end='  ' if is_tty else '... ', flush=True)
        p = subprocess.Popen(cmd,
                             shell=True,
                             stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE)
        c_idx = 0
        while p.poll() is None:
            if is_tty:
                bs()
                print(CAROUSEL[c_idx], end='', flush=True)
                c_idx += 1
                if c_idx == len(CAROUSEL):
                    c_idx = 0
            time.sleep(0.1)
        if is_tty:
            bs()
        if p.returncode == 0:
            cprint('OK', color='green', flush=True)
        else:
            cprint('FAILED!', color='red', attrs=['bold'], flush=True)
            print_st(p.stdout)
            print_st(p.stderr, color='red')
    if p.returncode != 0:
        raise RuntimeError(f'process exited with the code {p.returncode}')


def exec_cmd(cmd, args, search_in='bin', search_system=True, env=None):
    check_local_shell()
    c = f'{common.dir_eva}/{search_in}/{cmd}'
    if not os.path.isfile(c) and search_system:
        import shutil
        c = shutil.which(cmd)
    if not c:
        raise RuntimeError(f'{cmd} not found')
    if env is None:
        ec = ''
    else:
        ec = ''
        for var, value in env.items():
            ec += f'{var}="{value}" '
    code = os.system(f'{ec}{c} {args}')
    if code:
        raise RuntimeError(f'{cmd} failed with code {code}')


def print_action_result(result):
    import uuid
    result['uuid'] = str(uuid.UUID(bytes=result['uuid']))
    status = result['status']
    time = result['time']
    times = [v for _, v in time.items()]
    if status in ['completed', 'failed', 'terminated']:
        result['elapsed'] = '{:.6f}'.format(max(times) - min(times))
    if time:
        time_data = sorted([{
            'n': k,
            'v': datetime.fromtimestamp(v, common.TZ).isoformat()
        } for k, v in time.items()],
                           key=lambda k: TIME_ORD.get(k['n'], 10))
        result['time'] = format_table(time_data, generate_header=False)
    params = result['params']
    if params:
        params_data = [{'n': k, 'v': v} for k, v in params.items()]
        result['params'] = format_table(params_data, generate_header=False)
    out = result.pop('out')
    err = result.pop('err')
    data = sorted([{
        'n': k,
        'v': v
    } for k, v in result.items()],
                  key=lambda k: k['n'])
    rows = format_table(
        data,
        fmt=FORMAT_GENERATOR_COLS,
        generate_header=False,
        multiline=MULTILINE_ALLOW,
    )
    spacer = '  '
    for r in rows:
        print(colored(r[0], color='blue') + spacer, end='')
        if r[0].startswith('status '):
            print(colored(r[1], color=ACTION_STATUS_COLOR.get(status)))
        else:
            print(r[1])
    if out is not None:
        print('--- OUT ---')
        print(out)
    if err is not None:
        print('--- ERR ---')
        print(colored(err, color='red'))
    print()


def convert_bytes(data):
    if isinstance(data, bytes):
        return list(data)
    if isinstance(data, list):
        return [convert_bytes(item) for item in data]
    if isinstance(data, dict):
        return {key: convert_bytes(value) for key, value in data.items()}
    return data


def print_result(data, need_header=True, name_value=False, cols=None):
    if current_command.json:
        from pygments import highlight, lexers, formatters
        import json
        j = json.dumps(convert_bytes(data), indent=4, sort_keys=True)
        if can_colorize():
            j = highlight(j, lexers.JsonLexer(), formatters.TerminalFormatter())
        print(j)
        return
    elif data:
        if name_value:
            if name_value is True:
                nn = 'field'
                vn = 'value'
            else:
                nn = name_value[0]
                vn = name_value[1]
            data = sorted([{
                nn: k,
                vn: v
            } for k, v in data.items()],
                          key=lambda k: k[nn])
            if need_header:
                header, rows = format_table(data, fmt=FORMAT_GENERATOR)
                print(colored(header, color='blue'))
                print(colored('-' * len(header), color='grey'))
                for r in rows:
                    print(r)
            else:
                rows = format_table(data,
                                    fmt=FORMAT_GENERATOR_COLS,
                                    generate_header=False)
                spacer = '  '
                for r in rows:
                    print(colored(r[0], color='blue') + spacer, end='')
                    print(r[1])
        else:
            if cols:
                col_rules = {}
                for i, c in enumerate(cols):
                    if '|' in c:
                        r = c.split('|')
                        rules = {'_': r[0]}
                        col_rules[i] = rules
                        for rule in r[1:]:
                            k, v = rule.split('=', maxsplit=1)
                            rules[k] = v
                    else:
                        col_rules[i] = {}
                formatted_data = []
                for d in data:
                    od = OrderedDict()
                    for i, c in enumerate(cols):
                        rules = col_rules[i]
                        src = rules.get('_', c)
                        val = d.get(src, '')
                        fmt = rules.get('f')
                        if val != '' and val is not None:
                            try:
                                if fmt is not None:
                                    if fmt == 'time':
                                        val = datetime.fromtimestamp(
                                            val, common.TZ).isoformat()
                                    elif fmt.startswith('time:'):
                                        zone = fmt.split(':', 1)[-1]
                                        val = datetime.fromtimestamp(
                                            val,
                                            pytz.timezone(zone)).isoformat()
                                    if fmt == 'time_sec':
                                        val = datetime.fromtimestamp(
                                            round(val), common.TZ).isoformat()
                                    elif fmt.startswith('time_sec:'):
                                        zone = fmt.split(':', 1)[-1]
                                        val = datetime.fromtimestamp(
                                            round(val),
                                            pytz.timezone(zone)).isoformat()
                                    elif fmt.startswith('round:'):
                                        digits = int(fmt.split(':', 1)[-1])
                                        if digits > 0:
                                            val = round(val, digits)
                                        else:
                                            val = int(val)
                                    elif fmt == 'uuid_bytes':
                                        import uuid
                                        val = str(uuid.UUID(bytes=val))
                            except:
                                pass
                        if val is not None:
                            val = str(val)
                            max_len = rules.get('l')
                            if max_len is not None:
                                max_len = int(max_len) - 3
                                if len(val) > max_len:
                                    val = val[:max_len] + '...'
                        od[rules.get('n', c)] = val
                    formatted_data.append(od)
                header, rows = format_table(formatted_data,
                                            fmt=FORMAT_GENERATOR)
            else:
                if data and not isinstance(data[0], dict):
                    data = [{'value': d} for d in data]
                header, rows = format_table(data, fmt=FORMAT_GENERATOR)
            if need_header:
                print(colored(header, color='blue'))
                print(colored('-' * len(header), color='grey'))
            for r in rows:
                print(r)
    print()


def edit_file(fname):
    from pathlib import Path
    check_local_shell()
    suffix = Path(fname).suffix
    fname = f'{common.dir_eva}/{fname}'
    editor = os.getenv('EDITOR', 'vi')
    while True:
        code = os.system(f'{editor} "{fname}"')
        if code:
            err(f'editor exited with code {code}')
            break
        try:
            if suffix in ['.yaml', '.yml']:
                import yaml
                with open(fname) as fh:
                    yaml.safe_load(fh.read())
            elif suffix == '.json':
                import json
                with open(fname) as fh:
                    json.loads(fh.read())
            elif suffix == '.py':
                with open(fname) as fh:
                    compile(fh.read(), fname, 'exec')
            elif suffix == '.sh':
                st = os.stat(fname)
                os.chmod(
                    fname,
                    st.st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH)
            break
        except:
            print_tb(force=True, delay=True)
            continue


def edit_config(value, ss, deploy_fn, initial=False):
    import yaml
    import tempfile
    from hashlib import sha256
    from pathlib import Path
    editor = os.getenv('EDITOR', 'vi')
    fname = sha256(ss.encode()).hexdigest()
    tmpfile = Path(f'{tempfile.gettempdir()}/{fname}.tmp.yml')
    if isinstance(value, str):
        tmpfile.write_text(value)
    else:
        tmpfile.write_text(yaml.dump(value, default_flow_style=False))
    try:
        while True:
            code = os.system(f'{editor} {tmpfile}')
            if code:
                err(f'editor exited with code {code}')
                break
            try:
                data = yaml.safe_load(tmpfile.read_text())
            except:
                print_tb(force=True, delay=True)
                continue
            if data == value and not initial:
                break
            else:
                try:
                    deploy_fn(data)
                    break
                except Exception as e:
                    err(e, delay=True)
                    continue
    finally:
        try:
            tmpfile.unlink()
        except FileNotFoundError:
            pass


def edit_remote_file(value, orig_fname, ss, deploy_fn, initial=False):
    import tempfile
    from hashlib import sha256
    from pathlib import Path
    editor = os.getenv('EDITOR', 'vi')
    fname = sha256(ss.encode()).hexdigest()
    suffix = Path(orig_fname).suffix
    tmpfile = Path(f'{tempfile.gettempdir()}/{fname}.tmp{suffix}')
    tmpfile.write_text(value)
    try:
        while True:
            code = os.system(f'{editor} {tmpfile}')
            if code:
                err(f'editor exited with code {code}')
                break
            try:
                data = tmpfile.read_text()
            except:
                print_tb(force=True, delay=True)
                continue
            if data == value and not initial:
                break
            else:
                try:
                    deploy_fn(data)
                    break
                except Exception as e:
                    err(e, delay=True)
                    continue
    finally:
        try:
            tmpfile.unlink()
        except FileNotFoundError:
            pass


def read_file(fname=None):
    if fname is None or fname == '-':
        if sys.stdin.isatty():
            print(
                'Copy/paste or type data, press Ctrl-D to end, Ctrl-C to abort')
        return sys.stdin.buffer.read()
    else:
        with open(os.path.expanduser(fname), 'rb') as fh:
            return fh.read()


def write_file(fname, content, mode='w'):
    with open(os.path.expanduser(fname), mode) as fh:
        fh.write(content)


def format_value(value, advanced=False, name='', p=None):
    if p == 'json':
        import json
        return json.loads(value)
    if value == '<<':
        import pwinput
        value = pwinput.pwinput(prompt=f'{name}: ')
    if value.startswith('!'):
        return value[1:]
    if value.startswith('@'):
        import yaml
        with open(value[1:]) as fh:
            return yaml.safe_load(fh)
    elif advanced and ',' in value:
        return [format_value(v) for v in value.split(',')]
    else:
        if value == 'null':
            return None
        elif value == 'true':
            return True
        elif value == 'false':
            return False
        try:
            return int(value)
        except:
            try:
                return float(value)
            except:
                return value


def safe_print(val, extra=0):
    width, height = get_term_size()
    print(val[:width + extra])


def get_term_size():
    width, height = os.get_terminal_size(0)
    if width == 0 or height == 0:
        return (80, 25)
    else:
        return (width, height)


def get_node_svc_info():
    if is_local_shell():
        import json
        with os.popen(f'{common.dir_eva}/svc/eva-node --mode info') as p:
            return json.loads(p.read())
    else:
        from .client import call_rpc
        result = call_rpc('test')
        return {
            'name': result['product_name'],
            'code': result['product_code'],
            'build': result['build'],
            'version': result['version'],
            'arch': result.get('system_arch', 'unknown'),
        }


def get_arch():
    import platform
    return platform.machine()

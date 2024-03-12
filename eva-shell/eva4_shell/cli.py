import os
import sys
import time
from datetime import datetime
from functools import partial
from collections import OrderedDict

import busrt
from evaics.sdk import pack
from neotermcolor import colored
from rapidtables import format_table, FORMAT_GENERATOR, FORMAT_GENERATOR_COLS

from .tools import print_result, ok
from .tools import edit_config, edit_remote_file, read_file, write_file
from .tools import print_action_result, format_value, prepare_time, err, warn
from .tools import get_node_svc_info, edit_file, get_term_size, exec_cmd, xc
from .tools import get_my_ip, check_local_shell
from .client import call_rpc, DEFAULT_REPL_SERVICE, connect
from .sharedobj import common, current_command

from . import DEFAULT_REPOSITORY_URL


def eva_control_c(c, pfx=''):
    check_local_shell()
    cmd = f'{pfx}{common.dir_eva}/sbin/eva-control {c}'
    os.system(cmd)


class CLI:

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        for vc in [
                'build', 'list', 'edit', 'config', 'add', 'remove', 'update',
                'mirror-update', 'mirror-set'
        ]:
            setattr(self, f'venv_{vc.replace("-", "_")}',
                    partial(self.venv_cmd, vc))

    def venv_cmd(self, vc, **kwargs):
        args = '--verbose ' if current_command.debug else ''
        args += vc
        args += ' ' + ' '.join(kwargs.get('modules', []))
        if 'mirror_url' in kwargs:
            args += f' "{kwargs["mirror_url"]}"'
        if kwargs.get('rebuild'):
            args += ' --rebuild'
        if kwargs.get('from_scratch'):
            args += ' --from-scratch'
        if 'dest' in kwargs:
            args += f' --dest "{kwargs["dest"]}"'
        exec_cmd('venvmgr', args, search_in='sbin', search_system=False)

    def test(self, wait_active=False):
        op_start = time.perf_counter()
        while True:
            if op_start + current_command.timeout < time.perf_counter():
                raise TimeoutError
            else:
                data = call_rpc('test')
                if not wait_active or data.get('active'):
                    break
        print_result(data, need_header=False, name_value=True)

    def dump(self, s=False):
        args = ''
        args = ''
        if current_command.debug:
            args += ' --verbose '
        args += f'node dump --connection-path "{common.bus_path}"'
        if s:
            args += ' -s'
        exec_cmd('eva-cloud-manager', args)

    def save(self):
        call_rpc('save')
        ok()

    def server_start(self):
        eva_control_c('start')
        time.sleep(1)

    def server_stop(self):
        eva_control_c('stop')
        time.sleep(1)

    def server_restart(self):
        eva_control_c('restart')
        time.sleep(1)

    def server_reload(self):
        until = time.time() + current_command.timeout
        call_rpc('core.shutdown')
        while True:
            time.sleep(1)
            try:
                call_rpc('test')
                break
            except:
                pass
            if time.time() > until:
                raise TimeoutError
        ok()

    def server_launch(self):
        eva_control_c('launch', 'VERBOSE=1 ')

    def server_status(self):
        eva_control_c('status')

    def cloud_deploy(self, file, config_var, config, test):
        args = ''
        if current_command.debug:
            args += ' --verbose '
        args += (f'cloud deploy --timeout {current_command.timeout}'
                 f' --connection-path "{common.bus_path}" "{file}"')
        if config_var:
            for c in config_var:
                args += f' --config-var "{c}"'
        if config:
            args += f' --config "{config}"'
        if test:
            args += ' --test'
        exec_cmd('eva-cloud-manager', args)

    def cloud_undeploy(self, file, config_var, config, test):
        args = ''
        if current_command.debug:
            args += ' --verbose '
        args += (f'cloud undeploy --timeout {current_command.timeout}'
                 f' --connection-path "{common.bus_path}" "{file}"')
        if config_var:
            for c in config_var:
                args += f' --config-var "{c}"'
        if config:
            args += f' --config "{config}"'
        if test:
            args += ' --test'
        exec_cmd('eva-cloud-manager', args)

    def cloud_update(self, nodes, check_timeout, all, info_only, yes):
        if not nodes and not all:
            raise RuntimeError('specify either nodes to update or --all')
        args = ''
        if current_command.debug:
            args += ' --verbose '
        args += (f'cloud update --timeout {current_command.timeout}'
                 f' --connection-path "{common.bus_path}"'
                 f' --check-timeout "{check_timeout}"')
        args += ' ' + ' '.join([f'"{n}"' for n in nodes])
        if all:
            args += ' --all'
        if info_only:
            args += ' --info-only'
        if yes:
            args += ' --YES'
        exec_cmd('eva-cloud-manager', args)

    def update(self, download_timeout, repository_url, yes, info_only, test,
               target_version):
        args = ''
        if current_command.debug:
            args += ' --verbose '
        args += f'node update --timeout {current_command.timeout}'
        if repository_url:
            args = + f' --repository-url "{repository_url}"'
        if download_timeout:
            args += f' --download-timeout {download_timeout}'
        if yes:
            args += ' --YES'
        if info_only:
            args += ' --info-only'
        if test:
            args += ' --test'
        old_ver = get_node_svc_info()
        if target_version:
            env = {'EVA_UPDATE_FORCE_VERSION': target_version}
        else:
            env = None
        exec_cmd('eva-cloud-manager', args, env=env)
        if info_only:
            print()
            info = get_node_svc_info()
            print('update other node to the current version:\n')
            v = f'{info["version"]}:{info["build"]}'
            print(colored(f'    eva update --target-version {v}',
                          color='white'))
            print()
        else:
            if old_ver != get_node_svc_info():
                print('Update completed', end='')
                if common.interactive:
                    print('. Now exit EVA shell and log in back')
                else:
                    print()

    def registry_manage(self, yedb_args=None):
        check_local_shell()
        cmd = f'{common.dir_eva}/venv/bin/yedb'
        args = '' if yedb_args is None else ' ' + yedb_args
        if not os.path.exists(cmd):
            import shutil
            cmd = shutil.which('yedb')
            if not cmd:
                raise RuntimeError('YEDB CLI not found')
        try:
            data = call_rpc('test')
            system_name = data.get('system_name', '')
            yedb_socket = f'rt://{common.bus_path}:eva.registry'
            os.system(f'env "YEDB_PS=registry:{system_name}" "{cmd}" '
                      f'"{yedb_socket}"{args}')
        except:
            import getch
            warn('The node is offline, starting '
                 'the registry manager in offline mode')
            warn('The registry database will be '
                 'LOCKED until the registry shell exit')
            warn('Press any key to continue, Ctrl-C to abort...')
            if getch.getch() != '\x03':
                os.system(f'env YEDB_PS=registry "{cmd}"'
                          f' "{common.dir_eva}/runtime/registry"{args}')

    def edit(self, fname, filemgr_svc, offline=False):
        if fname.startswith('config'):
            self.registry_manage(yedb_args=f'edit eva/"{fname}"')
            if fname != 'config/python-venv':
                print('restart or reload the node server to apply changes')
        else:

            def deploy(content):
                call_rpc('file.put',
                         dict(i=fname, x=0o0755, c=content),
                         target=filemgr_svc)
                print(f'file re-deployed: {fname}')
                print()

            initial = False

            try:
                content = call_rpc('file.get',
                                   dict(i=fname, mode='text'),
                                   target=filemgr_svc)['text']
            except busrt.rpc.RpcException as e:
                if e.rpc_error_code == -32001:
                    initial = True
                    content = ''
                else:
                    raise
            edit_remote_file(content,
                             fname,
                             f'file|{fname}',
                             deploy_fn=partial(deploy),
                             initial=initial)

    def log_purge(self):
        call_rpc('log.purge')
        ok()

    def log_get(self, level, time, limit, module, regex, msg, full):

        def log_record_color(level):
            if level == 'TRACE':
                return dict(color='grey', attrs='dark')
            elif level == 'DEBUG':
                return dict(color='grey')
            elif level == 'WARN':
                return dict(color='yellow', attrs='bold')
            elif level == 'ERROR':
                return dict(color='red')
            return {}

        width, height = get_term_size()

        params = {
            'level': level,
            'time': time,
            'limit': limit if limit else height - 3,
            'module': module,
            'rx': regex
        }
        if msg:
            params['msg'] = msg
        records = call_rpc('log.get', params)
        if current_command.json:
            print_result(records)
        elif records:
            data = []
            for r in records:
                data.append(
                    dict(time=r['dt'],
                         host=r['h'],
                         module=r['mod'],
                         level=r['lvl'].upper(),
                         message=r['msg']))
            header, rows = format_table(data, fmt=FORMAT_GENERATOR)
            if not full:
                header = header[:width]
            print(colored(header, color='blue'))
            print(colored('-' * len(header), color='grey'))
            for row, record in zip(rows, data):
                print(
                    colored(row if full else row[:width],
                            **log_record_color(record['level'])))

    def acl_list(self, acl_svc):
        data = call_rpc('acl.list', target=acl_svc)
        print_result(data, cols=['id', 'admin'])

    def acl_edit(self, i, acl_svc):

        def deploy(cfg, i):
            call_rpc('acl.deploy', dict(acls=[cfg]), target=acl_svc)
            print(f'ACL re-deployed: {i}')
            print()

        config = call_rpc('acl.get_config', dict(i=i), target=acl_svc)
        edit_config(config, f'acl|{i}|config', deploy_fn=partial(deploy, i=i))

    def acl_export(self, i, acl_svc, output=None):
        c = 0
        configs = []
        for acl in call_rpc('acl.list', target=acl_svc):
            name = acl['id']
            if (i.startswith('*') and name.endswith(i[1:])) or \
                (i.endswith('*') and name.startswith(i[:-1])) or \
                (i.startswith('*') and i.endswith('*') and i[1:-1] in name) or \
                i == name:
                c += 1
                configs.append(acl)
        result = dict(acls=configs)
        if current_command.json:
            print_result(result)
        else:
            import yaml
            dump = yaml.dump(result, default_flow_style=False)
            if output is None:
                print(dump)
            else:
                write_file(output, dump)
                print(f'{c} ACL(s) exported')
                print()

    def acl_deploy(self, acl_svc, file=None):
        import yaml
        acls = yaml.safe_load(read_file(file).decode()).pop('acls')
        call_rpc('acl.deploy', dict(acls=acls), target=acl_svc)
        print(f'{len(acls)} ACL(s) deployed')
        print()

    def acl_undeploy(self, acl_svc, file=None):
        import yaml
        acls = yaml.safe_load(read_file(file).decode()).pop('acls')
        call_rpc('acl.undeploy', dict(acls=acls), target=acl_svc)
        print(f'{len(acls)} ACL(s) undeployed')
        print()

    def acl_create(self, i, acl_svc):
        call_rpc('acl.deploy', dict(acls=[dict(id=i)]), target=acl_svc)
        ok()

    def acl_destroy(self, i, acl_svc):
        call_rpc('acl.destroy', dict(i=i), target=acl_svc)
        ok()

    def key_list(self, auth_svc):
        data = call_rpc('key.list', target=auth_svc)
        if not current_command.json:
            for d in data:
                d['acls'] = ', '.join(d['acls'])
        print_result(data, cols=['id', 'acls'])

    def key_edit(self, i, auth_svc):

        def deploy(cfg, i):
            call_rpc('key.deploy', dict(keys=[cfg]), target=auth_svc)
            print(f'API key re-deployed: {i}')
            print()

        config = call_rpc('key.get_config', dict(i=i), target=auth_svc)
        edit_config(config, f'key|{i}|config', deploy_fn=partial(deploy, i=i))

    def key_export(self, i, auth_svc, output=None):
        c = 0
        configs = []
        for key in call_rpc('key.list', target=auth_svc):
            name = key['id']
            if (i.startswith('*') and name.endswith(i[1:])) or \
                (i.endswith('*') and name.startswith(i[:-1])) or \
                (i.startswith('*') and i.endswith('*') and i[1:-1] in name) or \
                i == name:
                c += 1
                configs.append(key)
        result = dict(keys=configs)
        if current_command.json:
            print_result(result)
        else:
            import yaml
            dump = yaml.dump(result, default_flow_style=False)
            if output is None:
                print(dump)
            else:
                write_file(output, dump)
                print(f'{c} API key(s) exported')
                print()

    def key_deploy(self, auth_svc, file=None):
        import yaml
        keys = yaml.safe_load(read_file(file).decode()).pop('keys')
        call_rpc('key.deploy', dict(keys=keys), target=auth_svc)
        print(f'{len(keys)} API key(s) deployed')
        print()

    def key_undeploy(self, auth_svc, file=None):
        import yaml
        keys = yaml.safe_load(read_file(file).decode()).pop('keys')
        call_rpc('key.undeploy', dict(keys=keys), target=auth_svc)
        print(f'{len(keys)} API key(s) undeployed')
        print()

    def key_create(self, i, auth_svc):
        import random
        import string
        symbols = string.ascii_letters + '0123456789'
        k = ''.join(random.choice(symbols) for i in range(64))
        call_rpc('key.deploy', dict(keys=[dict(id=i, key=k)]), target=auth_svc)
        data = call_rpc('key.get', dict(i=i), target=auth_svc)
        print_result([data], cols=['id', 'key'])

    def key_destroy(self, i, auth_svc):
        call_rpc('key.destroy', dict(i=i), target=auth_svc)
        ok()

    def key_regenerate(self, i, auth_svc):
        data = call_rpc('key.regenerate', dict(i=i), target=auth_svc)
        print_result([data], cols=['id', 'key'])

    def key_get(self, i, auth_svc):
        data = call_rpc('key.get_config', dict(i=i), target=auth_svc)
        if not current_command.json:
            data['acls'] = ', '.join(data['acls'])
        print_result([data], cols=['id', 'key', 'acls'])

    def user_list(self, auth_svc):
        data = call_rpc('user.list', target=auth_svc)
        if not current_command.json:
            for d in data:
                d['acls'] = ', '.join(d['acls'])
        print_result(data, cols=['login', 'acls'])

    def user_edit(self, i, auth_svc):

        def deploy(cfg, i):
            call_rpc('user.deploy', dict(users=[cfg]), target=auth_svc)
            print(f'user re-deployed: {i}')
            print()

        config = call_rpc('user.get_config', dict(i=i), target=auth_svc)
        edit_config(config, f'user|{i}|config', deploy_fn=partial(deploy, i=i))

    def user_export(self, i, auth_svc, output=None):
        c = 0
        configs = []
        for user in call_rpc('user.list',
                             dict(full=True, with_password=True),
                             target=auth_svc):
            name = user['login']
            if (i.startswith('*') and name.endswith(i[1:])) or \
                (i.endswith('*') and name.startswith(i[:-1])) or \
                (i.startswith('*') and i.endswith('*') and i[1:-1] in name) or \
                i == name:
                c += 1
                configs.append(user)
        result = dict(users=configs)
        if current_command.json:
            print_result(result)
        else:
            import yaml
            dump = yaml.dump(result, default_flow_style=False)
            if output is None:
                print(dump)
            else:
                write_file(output, dump)
                print(f'{c} user(s) exported')
                print()

    def user_deploy(self, auth_svc, file=None):
        import yaml
        users = yaml.safe_load(read_file(file).decode()).pop('users')
        call_rpc('user.deploy', dict(users=users), target=auth_svc)
        print(f'{len(users)} user(s) deployed')
        print()

    def user_undeploy(self, auth_svc, file=None):
        import yaml
        users = yaml.safe_load(read_file(file).decode()).pop('users')
        call_rpc('user.undeploy', dict(users=users), target=auth_svc)
        print(f'{len(users)} user(s) undeployed')
        print()

    def user_create(self, i, auth_svc):
        import pwinput
        password = pwinput.pwinput()
        try:
            pwhash = call_rpc('password.hash',
                              dict(password=password, algo='pbkdf2'),
                              target=auth_svc)['hash']
        except Exception as e:
            err(e)
            warn('deploying sha256-hashed password')
            from hashlib import sha256
            pwhash = sha256(password.encode()).hexdigest()
        call_rpc('user.deploy',
                 dict(users=[dict(login=i, password=pwhash)]),
                 target=auth_svc)
        ok()

    def user_destroy(self, i, auth_svc):
        call_rpc('user.destroy', dict(i=i), target=auth_svc)
        ok()

    def user_password(self, i, auth_svc, ignore_policy):
        import pwinput
        password = pwinput.pwinput()
        data = call_rpc('user.set_password',
                        dict(i=i,
                             password=password,
                             check_policy=not ignore_policy),
                        target=auth_svc)
        ok()

    def user_get(self, i, auth_svc):
        data = call_rpc('user.get_config', dict(i=i), target=auth_svc)
        if not current_command.json:
            data['acls'] = ', '.join(data['acls'])
        print_result([data], cols=['login', 'acls'])

    def svc_list(self, filter=None):
        if filter is None:
            params = None
        else:
            params = dict(filter='^' + filter.replace('.', '\\.') + '.*$')
        data = call_rpc('svc.list', params)
        print_result(data, cols=['id', 'status', 'pid', 'launcher'])

    def svc_export(self, i, output=None):
        c = 0
        configs = []
        for svc in call_rpc('svc.list'):
            name = svc['id']
            if (i.startswith('*') and name.endswith(i[1:])) or \
                (i.endswith('*') and name.startswith(i[:-1])) or \
                (i.startswith('*') and i.endswith('*') and i[1:-1] in name) or \
                i == name:
                c += 1
                configs.append(
                    dict(id=name,
                         params=call_rpc('svc.get_params', dict(i=name))))
        result = dict(svcs=configs)
        if current_command.json:
            print_result(result)
        else:
            import yaml
            dump = yaml.dump(dict(svcs=configs), default_flow_style=False)
            if output is None:
                print(dump)
            else:
                write_file(output, dump)
                print(f'{c} service(s) exported')
                print()

    def svc_deploy(self, file=None):
        import yaml
        svcs = yaml.safe_load(read_file(file).decode()).pop('svcs')
        call_rpc('svc.deploy', dict(svcs=svcs))
        print(f'{len(svcs)} service(s) deployed')
        print()

    def svc_undeploy(self, file=None):
        import yaml
        svcs = yaml.safe_load(read_file(file).decode()).pop('svcs')
        call_rpc('svc.undeploy', dict(svcs=svcs))
        print(f'{len(svcs)} service(s) undeployed')
        print()

    def svc_restart(self, i):
        call_rpc('svc.restart', dict(i=i))
        ok()

    def svc_destroy(self, i):
        call_rpc('svc.undeploy', dict(svcs=[i]))
        ok()

    def svc_purge(self, i):
        call_rpc('svc.purge', dict(svcs=[i]))
        ok()

    def svc_call(self, i, file, method, params, trace=False):
        if file:
            import yaml
            payload = yaml.safe_load(read_file(file))
        else:
            payload = {}
            for p in params:
                n, v = p.split('=', 1)
                payload[n] = format_value(v, advanced=True, name=n)
        result = call_rpc(method,
                          payload if payload else None,
                          target=i,
                          trace=trace)
        if current_command.json or isinstance(result, list):
            print_result(result)
        elif isinstance(result, dict):
            print_result(result, name_value=True)
        elif result is None:
            ok()
        else:
            print(result)
            print()

    def svc_edit(self, i):

        def deploy(cfg, i):
            call_rpc('svc.deploy', dict(svcs=[dict(id=i, params=cfg)]))
            print(f'service re-deployed: {i}')
            print()

        config = call_rpc('svc.get_params', dict(i=i))
        edit_config(config, f'svc|{i}|config', deploy_fn=partial(deploy, i=i))

    def svc_enable(self, i):
        config = call_rpc('svc.get_params', dict(i=i))
        if not config.get('enabled'):
            config['enabled'] = True
            call_rpc('svc.deploy', dict(svcs=[dict(id=i, params=config)]))
        ok()

    def svc_disable(self, i):
        config = call_rpc('svc.get_params', dict(i=i))
        if config.get('enabled'):
            config['enabled'] = False
            call_rpc('svc.deploy', dict(svcs=[dict(id=i, params=config)]))
        ok()

    def svc_create(self, i, f):

        def deploy(cfg, i):
            call_rpc('svc.deploy', dict(svcs=cfg))
            print(f'service deployed: {i}')
            print()

        lines = read_file(f).decode().split('\n')
        tpl = (' ' * 4) + ('\n' + ' ' * 4).join(lines).rstrip()
        config = f'- id: {i}\n  params:\n{tpl}'
        if sys.stdin.isatty():
            edit_config(config,
                        f'svc|{i}|config',
                        deploy_fn=partial(deploy, i=i))
        else:
            import yaml
            deploy(yaml.safe_load(config), i)

    def svc_test(self, i):
        call_rpc('test', target=i)
        ok()

    def svc_info(self, i):
        data = call_rpc('info', target=i)
        if not current_command.json:
            try:
                del data['methods']
            except KeyError:
                pass
        print_result(data, need_header=False, name_value=True)

    def broker_test(self):
        call_rpc('test', target='.broker')
        ok()

    def broker_info(self):
        data = call_rpc('info', target='.broker')
        print_result(data, need_header=False, name_value=True)

    def broker_stats(self):
        data = call_rpc('stats', target='.broker')
        print_result(data, need_header=False, name_value=True)

    def broker_client_list(self, filter=None):
        if filter is None:
            params = None
        else:
            params = dict(filter='^' + filter.replace('.', '\\.') + '.*$')
        data = call_rpc('client.list', params, target='.broker')
        if current_command.json:
            print_result(data)
        else:
            print_result(
                [d for d in data['clients'] if d['name'] != common.bus_name],
                cols=[
                    'name', 'kind|n=type', 'source', 'port', 'r_frames',
                    'r_bytes', 'w_frames', 'w_bytes', 'queue', 'instances|n=ins'
                ])

    def action_exec(self, i, value, priority, wait):
        params = dict(i=i, wait=wait)
        if priority is not None:
            params['priority'] = int(priority)
        action_params = {'value': format_value(value, advanced=True)}
        params['params'] = action_params
        result = call_rpc('action', params)
        if current_command.json:
            import uuid
            result['uuid'] = str(uuid.UUID(bytes=result['uuid']))
            print_result(result)
        else:
            print_action_result(result)

    def action_run(self, i, arg, kwarg, priority, wait):
        params = dict(i=i, wait=wait)
        if priority is not None:
            params['priority'] = int(priority)
        action_params = {}
        if arg:
            action_params['args'] = [format_value(v) for v in arg]
        if kwarg:
            kw = {}
            for k in kwarg:
                n, v = k.split('=', 1)
                kw[n] = format_value(v)
            action_params['kwargs'] = kw
        params['params'] = action_params
        result = call_rpc('run', params)
        if current_command.json:
            import uuid
            result['uuid'] = str(uuid.UUID(bytes=result['uuid']))
            print_result(result)
        else:
            print_action_result(result)

    def action_toggle(self, i, priority, wait):
        params = dict(i=i, wait=wait)
        if priority is not None:
            params['priority'] = int(priority)
        result = call_rpc('action.toggle', params)
        if current_command.json:
            import uuid
            result['uuid'] = str(uuid.UUID(bytes=result['uuid']))
            print_result(result)
        else:
            print_action_result(result)

    def action_terminate(self, u):
        import uuid
        call_rpc('action.terminate', dict(u=uuid.UUID(u).bytes))
        ok()

    def action_kill(self, i):
        call_rpc('action.kill', dict(i=i))
        ok()

    def action_result(self, u):
        import uuid
        result = call_rpc('action.result', dict(u=uuid.UUID(u).bytes))
        if current_command.json:
            import uuid
            result['uuid'] = str(uuid.UUID(bytes=result['uuid']))
            print_result(result)
        else:
            print_action_result(result)

    def action_list(self, oid, status_query, svc, time, limit):
        import uuid
        from .tools import ACTION_STATUS_COLOR
        params = dict(i=oid, sq=status_query, svc=svc, time=time)
        if limit is not None:
            params['limit'] = limit
        result = call_rpc('action.list', params)
        if current_command.json:
            for r in result:
                r['uuid'] = str(uuid.UUID(bytes=r['uuid']))
            print_result(result)
        elif result:
            data = []
            for r in result:
                if r['status'] in ['completed', 'failed', 'terminated']:
                    times = [v for _, v in r['time'].items()]
                    elapsed = '{:.6f}'.format(max(times) - min(times))
                else:
                    elapsed = ''
                t = r.get('time', {}).get('created')
                if t:
                    t = datetime.fromtimestamp(t, common.TZ).isoformat()
                d = OrderedDict()
                d['time'] = t
                d['uuid'] = str(uuid.UUID(bytes=r['uuid']))
                d['oid'] = r['oid']
                d['status'] = r['status']
                d['elapsed'] = elapsed
                d['node'] = r['node']
                d['svc'] = r['svc']
                data.append(d)
            spacer = '  '
            header, rows = format_table(data, fmt=FORMAT_GENERATOR_COLS)
            for i, c in enumerate(header):
                if i > 0:
                    print(spacer, end='')
                print(colored(c, color='blue'), end='')
            print()
            print(
                colored('-' * (sum(len(s) for s in header) +
                               (len(header) - 1) * 2),
                        color='grey'))
            for d, r in zip(data, rows):
                for i, c in enumerate(r):
                    if i > 0:
                        print(spacer, end='')
                    if i == 3:
                        print(colored(c,
                                      color=ACTION_STATUS_COLOR.get(
                                          d['status'])),
                              end='')
                    else:
                        print(c, end='')
                print()

    def item_summary(self):
        data = call_rpc('item.summary')
        if current_command.json:
            print_result(data)
        else:
            print_result(data['sources'], name_value=['node', 'items'])
            print('total:', colored(data['items'], color='yellow'))
            print()

    def node_list(self, repl_svc, default_rpl):
        if default_rpl and repl_svc is None:
            repl_svc = DEFAULT_REPL_SERVICE
        if repl_svc is None:
            data = call_rpc('node.list')
            if not current_command.json:
                for d in data:
                    if d['remote']:
                        d['type'] = 'remote'
                    else:
                        d['type'] = 'local'
                    try:
                        info = d.pop('info')
                        d['version'] = info.get('version')
                        d['build'] = info.get('build')
                    except KeyError:
                        pass
            print_result(data,
                         cols=[
                             'name', 'svc', 'type', 'online', 'timeout',
                             'version', 'build'
                         ])
        else:
            data = call_rpc('node.list', target=repl_svc)
            print_result(data,
                         cols=[
                             'name', 'timeout', 'compress|n=compr',
                             'ping_interval|n=ping int',
                             'reload_interval|n=rel.int', 'static', 'enabled',
                             'managed', 'trusted', 'online',
                             'link_uptime|n=link upt|f=round:0', 'version',
                             'build'
                         ])

    def node_append(self, i, repl_svc, untrusted):
        call_rpc('node.append',
                 dict(i=i, trusted=not untrusted),
                 target=repl_svc)
        ok()

    def node_edit(self, i, repl_svc):

        def deploy_edited_node(cfg, i):
            call_rpc('node.deploy', dict(nodes=[cfg]), target=repl_svc)
            print(f'node re-deployed: {i}')
            print()

        config = call_rpc('node.get_config', dict(i=i), target=repl_svc)
        edit_config(config,
                    f'node|{i}|config',
                    deploy_fn=partial(deploy_edited_node, i=i))

    def node_export(self, i, repl_svc, output=None):
        configs = call_rpc('node.export', dict(i=i), target=repl_svc)
        c = len(configs['nodes'])
        if current_command.json:
            print_result(configs)
        else:
            import yaml
            dump = yaml.dump(configs, default_flow_style=False)
            if output is None:
                print(dump)
            else:
                write_file(output, dump)
                print(f'{c} node(s) exported')
                print()

    def node_test(self, i, repl_svc):
        call_rpc('node.test', dict(i=i), target=repl_svc)
        ok()

    def node_mtest(self, i, repl_svc):
        call_rpc('node.mtest', dict(i=i), target=repl_svc)
        ok()

    def node_remove(self, i, repl_svc):
        call_rpc('node.remove', dict(i=i), target=repl_svc)
        ok()

    def node_reload(self, i, repl_svc):
        call_rpc('node.reload', dict(i=i), target=repl_svc)
        print('reload request accepted')
        print()

    def node_deploy(self, repl_svc, file=None):
        import yaml
        nodes = yaml.safe_load(read_file(file).decode()).pop('nodes')
        call_rpc('node.deploy', dict(nodes=nodes), target=repl_svc)
        print(f'{len(nodes)} node(s) deployed')
        print()

    def node_undeploy(self, repl_svc, file=None):
        import yaml
        nodes = yaml.safe_load(read_file(file).decode()).pop('nodes')
        call_rpc('node.undeploy', dict(nodes=nodes), target=repl_svc)
        print(f'{len(nodes)} node(s) undeployed')
        print()

    def spoint_list(self):
        data = call_rpc('spoint.list')
        print_result(data, cols=['name', 'source', 'port', 'version', 'build'])

    def item_list(self, i, n=None):
        data = call_rpc(
            'item.list',
            dict(i=i, node=n),
        )
        print_result(data,
                     cols=[
                         'oid', 'status', 'value|l=20|n=value',
                         't|n=set time|f=time', 'node', 'connected', 'enabled'
                     ])

    def item_announce(self, i, n=None):
        data = call_rpc(
            'item.announce',
            dict(i=i, node=n),
        )
        ok()

    def item_export(self, i, full=False, output=None):
        c = 0
        configs = dict()
        for item in call_rpc('item.list', dict(i=i, node='.local')):
            c += 1
            configs[item['oid']] = call_rpc('item.get_config', dict(i=item['oid']))
        if full:
            for state in call_rpc('item.state', dict(i=list(configs))):
                oid = state['oid']
                configs[oid]['status'] = state['status']
                configs[oid]['value'] = state['value']
        result = dict(items=sorted([v for _, v in configs.items()], key= lambda k: k['oid']))
        if current_command.json:
            print_result(result)
        else:
            import yaml
            dump = yaml.dump(result, default_flow_style=False)
            if output is None:
                print(dump)
            else:
                write_file(output, dump)
                print(f'{c} item(s) exported')
                print()

    def item_set(self, i, status, value, p):
        val = format_value(value, p=p)
        payload = dict(status=1 if status is None else status,
                       value=val,
                       force=True)
        connect()
        common.bus.send(
            'RAW/' + i.replace(':', '/', 1),
            busrt.client.Frame(pack(payload), tp=busrt.client.OP_PUBLISH,
                               qos=1)).wait_completed()
        time.sleep(0.1)
        try:
            state = call_rpc('item.state', dict(i=i))[0]
        except IndexError:
            err('no such item')
            return
        try:
            if state['value'] != val or (status is not None and
                                         state['status'] != status):
                err('FAILED')
            else:
                ok()
        except KeyError:
            err(f'{i} has no state')

    def item_deploy(self, file=None):
        import yaml
        items = yaml.safe_load(read_file(file).decode()).pop('items')
        call_rpc('item.deploy', dict(items=items))
        print(f'{len(items)} item(s) deployed')
        print()

    def item_undeploy(self, file=None):
        import yaml
        items = yaml.safe_load(read_file(file).decode()).pop('items')
        call_rpc('item.undeploy', dict(items=items))
        print(f'{len(items)} item(s) undeployed')
        print()

    def item_create(self, i):
        call_rpc('item.create', dict(i=i))
        ok()

    def item_destroy(self, i):
        call_rpc('item.destroy', dict(i=i))
        ok()

    def item_enable(self, i):
        call_rpc('item.enable', dict(i=i))
        ok()

    def item_edit(self, i):

        def deploy_edited_item(cfg, i):
            call_rpc('item.deploy', dict(items=[cfg]))
            print(f'item re-deployed: {i}')
            print()

        config = call_rpc('item.get_config', dict(i=i))
        edit_config(config,
                    f'item|{i}|config',
                    deploy_fn=partial(deploy_edited_item, i=i))

    def item_disable(self, i):
        call_rpc('item.disable', dict(i=i))
        ok()

    def item_state(self, i, full):
        data = call_rpc(
            'item.state',
            dict(i=i, full=full),
        )
        print_result(data,
                     cols=[
                         'oid', 'status', 'value|l=20|n=value', 'ieid',
                         't|n=set time|f=time', 'node', 'connected', 'act'
                     ])

    def item_slog(self, i, db_svc, time_start, time_end, time_zone, limit):
        time_start = prepare_time(time_start)
        time_end = prepare_time(time_end)
        data = call_rpc('state_log',
                        dict(i=i,
                             t_start=time_start,
                             t_end=time_end,
                             limit=limit),
                        target=db_svc)
        print_result(
            data,
            cols=[
                'oid', 'status', 'value',
                't|n=time|f=time{}'.format(f':{time_zone}' if time_zone else '')
            ])

    def item_history(self, i, db_svc, time_start, time_end, time_zone, limit,
                     prop, fill):
        time_start = prepare_time(time_start)
        time_end = prepare_time(time_end)
        precs = None
        if fill is not None:
            if ':' in fill:
                fill, precs = fill.split(':', 1)
                precs = int(precs)
        data = call_rpc('state_history',
                        dict(i=i,
                             t_start=time_start,
                             t_end=time_end,
                             fill=fill,
                             precision=precs,
                             prop=prop,
                             limit=limit),
                        target=db_svc)
        cols = []
        if prop == 'status' or prop is None:
            cols += ['status']
        if prop == 'value' or prop is None:
            cols += ['value']
        cols += [
            't|n=time|f=time{}'.format(f':{time_zone}' if time_zone else '')
        ]
        print_result(data, cols=cols)

    def accounting_query(self, accounting_svc, time_start, time_end, time_zone,
                         node, user, source, svc, subject, oid, note, data,
                         code, err, full):
        payload = {}
        if time_start is not None:
            payload['t_start'] = time_start
        if time_end is not None:
            payload['t_end'] = time_end
        if node is not None:
            payload['node'] = node
        if user is not None:
            payload['u'] = user
        if source is not None:
            payload['src'] = source
        if svc is not None:
            payload['svc'] = svc
        if subject is not None:
            payload['subj'] = subject
        if oid is not None:
            payload['oid'] = oid
        if note is not None:
            payload['note'] = note
        if data is not None:
            payload['data'] = data
        if code is not None:
            payload['code'] = code
        if err is not None:
            payload['err'] = err
        data = call_rpc('query', payload, target=accounting_svc)
        cols = [
            't|n=time|f=time_sec{}'.format(
                f':{time_zone}' if time_zone else ''),
            'node',
            'svc',
            'u|n=user',
            'src|n=source',
            'subj|n=subject',
            'code',
            'oid',
            'note',
        ]
        if full:
            cols.append('data')
        cols.append('err')
        print_result(data, cols=cols)

    def item_watch(self, i, interval, rows, prop, chart_type):
        from . import charts
        import datetime
        vals = []
        to_clear = 0
        old_width = 0
        old_height = 0
        limit = rows
        limit_auto_set = rows is None

        def _append(label, value, limit):
            if chart_type == 'bar':
                vals.append((label, value))
            else:
                if isinstance(value, float):
                    vals.append(value)
                else:
                    try:
                        v = float(value)
                    except:
                        v = 0
                    vals.append(v)
            while len(vals) > limit:
                vals.pop(0)

        next_step = time.perf_counter() + interval
        try:
            while True:
                if chart_type == 'bar':
                    label = datetime.datetime.now().strftime('%T.%f')[:-5]
                else:
                    label = datetime.datetime.now().isoformat()
                width, height = get_term_size()
                if limit is None:
                    if chart_type == 'bar':
                        limit = height - 2
                    else:
                        limit = width - 12
                if width != old_width or height != old_height:
                    os.system('clear')
                    if chart_type == 'bar':
                        print(colored(i, color='yellow'))
                    old_width, old_height = width, height
                    if limit_auto_set:
                        if chart_type == 'bar':
                            limit = height - 2
                        else:
                            rows = height - 6
                elif chart_type == 'bar':
                    for t in range(to_clear):
                        sys.stdout.write('\033[F\033[K')
                elif chart_type == 'line':
                    os.system('clear')
                res = call_rpc('item.state', dict(i=i))
                for data in res:
                    if data['oid'] == i:
                        break
                else:
                    raise ValueError('no state returned')
                v = data.get(prop)
                if isinstance(v, str):
                    try:
                        v = int(v)
                    except:
                        try:
                            v = float(v)
                        except:
                            v = None
                if chart_type == 'bar':
                    _append(label, v, limit)
                    charts.plot_bar_chart(vals)
                    to_clear = len(vals)
                else:
                    _append(label, v, width)
                    max_value_width = 0
                    for z in vals:
                        x = len(str(z))
                        if x > max_value_width:
                            max_value_width = x
                    while len(vals) > width - (max_value_width if
                                               max_value_width > 7 else 7) - 5:
                        vals.pop(0)
                    max_value_width += 2
                    print(
                        f'{colored(i, color="yellow")} '
                        f'{colored(label, color="cyan")}: ',
                        end='')
                    if v is None:
                        print(colored('NaN', color='magenta'))
                    else:
                        print(colored(v, color='green', attrs='bold'))
                    charts.plot_line_chart(vals, rows, max_value_width)
                t = time.perf_counter()
                sleep_to = next_step - t
                if sleep_to > 0:
                    time.sleep(sleep_to)
                    next_step += interval
                else:
                    next_step = t
        except KeyboardInterrupt:
            return
        pass

    def lvar_set(self, i, status, value, p):
        params = dict(i=i)
        if status is not None:
            params['status'] = status
        if value is not None:
            params['value'] = format_value(value, p=p)
        call_rpc('lvar.set', params)
        ok()

    def lvar_reset(self, i):
        call_rpc('lvar.reset', dict(i=i))
        ok()

    def lvar_clear(self, i):
        call_rpc('lvar.clear', dict(i=i))
        ok()

    def lvar_toggle(self, i):
        call_rpc('lvar.toggle', dict(i=i))
        ok()

    def lvar_incr(self, i):
        val = call_rpc('lvar.incr', dict(i=i))
        print(val)

    def lvar_decr(self, i):
        val = call_rpc('lvar.decr', dict(i=i))
        print(val)

    def system_update(self):
        import shutil
        apt = shutil.which('apt')
        if apt:
            update = 'apt update'
            upgrade = 'apt -y upgrade'
            clean = 'apt -y clean'
        else:
            apk = shutil.which('apk')
            if apk:
                update = 'apk update'
                upgrade = 'apk upgrade'
                clean = None
            else:
                raise RuntimeError(
                    'Unsupported OS, please update the system manually')
        if update:
            xc(update,
               ps='Updating package indexes',
               verbose=current_command.debug)
        if upgrade:
            xc(upgrade, ps='Upgrading packages', verbose=current_command.debug)
        if clean:
            xc(clean, ps='Cleaning up', verbose=current_command.debug)
        xc('sync', ps='Flushing data to disk', verbose=current_command.debug)
        print('System updated')
        print()

    def system_reboot(self):
        import getch
        print('Reboot the system? (y/n) ', end='', flush=True)
        c = getch.getch()
        print()
        if c in ['y', 'Y']:
            warn('REBOOTING THE SYSTEM...')
            if os.system('reboot'):
                raise RuntimeError
            while True:
                time.sleep(1)

    def system_poweroff(self):
        import getch
        print('Power off the system? (y/n) ', end='', flush=True)
        c = getch.getch()
        print()
        if c in ['y', 'Y']:
            warn('POWERING OFF...')
            if os.system('poweroff'):
                raise RuntimeError
            while True:
                time.sleep(1)

    def version(self):
        from . import __version__
        info = get_node_svc_info()
        if current_command.json:
            d = {
                'cli_version': __version__,
                'build': info['build'],
                'version': info['version'],
                'arch': info.get('arch', '')
            }
        else:
            d = [{
                'name': 'version',
                'value': info['version']
            }, {
                'name': 'build',
                'value': info['build']
            }, {
                'name': 'arch',
                'value': info.get('arch', '')
            }, {
                'name': 'CLI version',
                'value': __version__
            }]
        ip = get_my_ip()
        if ip:
            if current_command.json:
                d['ip'] = ip
            else:
                d.append({'name': 'IP address', 'value': ip})
        print_result(d, need_header=False, name_value=False)

    def mirror_set(self, mirror_url):
        args = ''
        if current_command.debug:
            args += ' --verbose '
        args += f'node mirror-set "{mirror_url}"'
        exec_cmd('eva-cloud-manager', args)
        self.venv_mirror_set(mirror_url=mirror_url)

    def mirror_update(self, dest, current_arch_only, force, repository_url):
        args = ''
        if current_command.debug:
            args += ' --verbose '
        args += f'node mirror-update --timeout {current_command.timeout}'
        if dest:
            args += f' --dest "{dest}"'
        if repository_url:
            args = + f' --repository-url "{repository_url}"'
        if force:
            args += ' --force'
        if current_arch_only:
            args += ' --current-arch-only'
        exec_cmd('eva-cloud-manager', args)
        self.venv_mirror_update(dest=dest)
        print()
        self.mirror_info()

    def mirror_info(self):
        ip = get_my_ip()
        port = 7780
        if ip is None:
            ip = '<host/ip>'
        mirror_url = f'http://{ip}:{port}'
        print(f'Mirror URL: ' +
              colored(mirror_url, color='green', attrs=['bold']))
        print()
        print(f'EVA ICS update: -u http://{ip}:{port}/eva')
        print(f'pip-extra-options: "-i {mirror_url}/pypi/local'
              f' --trusted-host {ip}"')
        print()
        print('To automatically set the mirror config on'
              ' other nodes, execute there a command:')
        print()
        print(
            colored(f'    eva mirror set {mirror_url}',
                    color='green',
                    attrs=['bold']))
        print()
        print('Note: deploy the mirror service if not deployed.')
        print('If the mirror is not created yet, '
              'create it with "eva mirror update" command.')
        print('If the mirror service is set to different port, '
              'replace it in the URLs above.')

    def kiosk_list(self, kiosk_svc):
        data = call_rpc('kiosk.list', target=kiosk_svc)
        if current_command.json:
            print_result(data)
        else:
            formatted_data = []
            for d in data:
                state = d.get('state')
                info = OrderedDict()
                info['name'] = d.get('name')
                info['ip'] = d.get('ip')
                info['a.log'] = d.get('auto_login')
                info['agent'] = d.get('agent')
                info['version'] = d.get('version')
                info['state'] = state if state else 'offline'
                info['current url'] = d.get('current_url')
                formatted_data.append(info)
            spacer = '  '
            header, rows = format_table(formatted_data,
                                        fmt=FORMAT_GENERATOR_COLS)
            for i, c in enumerate(header):
                if i > 0:
                    print(spacer, end='')
                print(colored(c, color='blue'), end='')
            print()
            print(
                colored('-' * (sum(len(s) for s in header) +
                               (len(header) - 1) * 2),
                        color='grey'))
            for d, r in zip(formatted_data, rows):
                for i, c in enumerate(r):
                    if i > 0:
                        print(spacer, end='')
                    if i == 5:
                        if d['state'] == 'active':
                            color = 'green'
                        elif d['state'] == 'offline':
                            color = 'grey'
                        else:
                            color = None
                        print(colored(c, color=color), end='')
                    else:
                        print(c, end='')
                print()

    def kiosk_edit(self, i, kiosk_svc):

        def deploy(cfg, i):
            call_rpc('kiosk.deploy', dict(kiosks=[cfg]), target=kiosk_svc)
            print(f'Kiosk configuration re-deployed: {i}')
            print()

        config = call_rpc('kiosk.get_config', dict(i=i), target=kiosk_svc)
        edit_config(config, f'kiosk|{i}|config', deploy_fn=partial(deploy, i=i))

    def kiosk_export(self, i, kiosk_svc, output=None):
        c = 0
        configs = []
        for kiosk in call_rpc('kiosk.list', target=kiosk_svc):
            name = kiosk['name']
            if (i.startswith('*') and name.endswith(i[1:])) or \
                (i.endswith('*') and name.startswith(i[:-1])) or \
                (i.startswith('*') and i.endswith('*') and i[1:-1] in name) or \
                i == name:
                c += 1
                config = call_rpc('kiosk.get_config',
                                  dict(i=name),
                                  target=kiosk_svc)
                configs.append(config)
        result = dict(kiosks=configs)
        if current_command.json:
            print_result(result)
        else:
            import yaml
            dump = yaml.dump(result, default_flow_style=False)
            if output is None:
                print(dump)
            else:
                write_file(output, dump)
                print(f'{c} kiosk configuration(s) exported')
                print()

    def kiosk_deploy(self, kiosk_svc, file=None):
        import yaml
        kiosks = yaml.safe_load(read_file(file).decode()).pop('kiosks')
        call_rpc('kiosk.deploy', dict(kiosks=kiosks), target=kiosk_svc)
        print(f'{len(kiosks)} kiosk configuration(s) deployed')
        print()

    def kiosk_undeploy(self, kiosk_svc, file=None):
        import yaml
        kiosks = yaml.safe_load(read_file(file).decode()).pop('kiosks')
        call_rpc('kiosk.undeploy', dict(kiosks=kiosks), target=kiosk_svc)
        print(f'{len(kiosks)} kiosk configuration(s) undeployed')
        print()

    def kiosk_create(self, i, kiosk_svc):
        call_rpc('kiosk.deploy', dict(kiosks=[dict(name=i)]), target=kiosk_svc)
        ok()

    def kiosk_destroy(self, i, kiosk_svc):
        call_rpc('kiosk.destroy', dict(i=i), target=kiosk_svc)
        ok()

    def kiosk_login(self, i, kiosk_svc, acl, login):
        if acl:
            auth = dict(acls=acl, login=login)
        else:
            auth = None
        call_rpc('kiosk.login', dict(i=i, auth=auth), target=kiosk_svc)
        ok()

    def kiosk_logout(self, i, kiosk_svc):
        call_rpc('kiosk.logout', dict(i=i), target=kiosk_svc)
        ok()

    def kiosk_test(self, i, kiosk_svc):
        call_rpc('kiosk.test', dict(i=i), target=kiosk_svc)
        ok()

    def kiosk_info(self, i, kiosk_svc):
        result = call_rpc('kiosk.info', dict(i=i), target=kiosk_svc)
        print_result(result, name_value=True)

    def kiosk_alert(self, i, kiosk_svc, text, level=None, t=None):
        call_rpc('kiosk.alert',
                 dict(i=i, text=text, level=level, timeout=t),
                 target=kiosk_svc)
        ok()

    def kiosk_eval(self, i, code, kiosk_svc):
        call_rpc('kiosk.eval', dict(i=i, code=code), target=kiosk_svc)
        ok()

    def kiosk_navigate(self, i, url, kiosk_svc):
        call_rpc('kiosk.navigate', dict(i=i, url=url), target=kiosk_svc)
        ok()

    def kiosk_display(self, i, kiosk_svc, off, on, brightness=None):
        if on:
            dst = True
        elif off:
            dst = False
        else:
            dst = None
        call_rpc('kiosk.display',
                 dict(i=i, on=dst, brightness=brightness),
                 target=kiosk_svc)
        ok()

    def kiosk_zoom(self, i, level, kiosk_svc):
        call_rpc('kiosk.zoom', dict(i=i, level=level), target=kiosk_svc)
        ok()

    def kiosk_reload(self, i, kiosk_svc):
        call_rpc('kiosk.reload', dict(i=i), target=kiosk_svc)
        ok()

    def kiosk_reboot(self, i, kiosk_svc):
        call_rpc('kiosk.reboot', dict(i=i), target=kiosk_svc)
        ok()

    def kiosk_dev_open(self, i, kiosk_svc):
        call_rpc('kiosk.dev_open', dict(i=i), target=kiosk_svc)
        ok()

    def kiosk_dev_close(self, i, kiosk_svc):
        call_rpc('kiosk.dev_close', dict(i=i), target=kiosk_svc)
        ok()

    def generator_source_list(self, generator_svc):
        data = call_rpc('source.list', target=generator_svc)
        print_result(data, cols=['name', 'kind', 'active'])

    def generator_source_edit(self, i, generator_svc):

        def deploy(cfg, i):
            call_rpc('source.deploy',
                     dict(generator_sources=[cfg]),
                     target=generator_svc)
            print(f'generator source re-deployed: {i}')
            print()

        config = call_rpc('source.get_config', dict(i=i), target=generator_svc)
        edit_config(config,
                    f'generator.source|{i}|config',
                    deploy_fn=partial(deploy, i=i))

    def generator_source_deploy(self, generator_svc, file=None):
        import yaml
        sources = yaml.safe_load(
            read_file(file).decode()).pop('generator_sources')
        call_rpc('source.deploy',
                 dict(generator_sources=sources),
                 target=generator_svc)
        print(f'{len(sources)} generator source configuration(s) deployed')
        print()

    def generator_source_undeploy(self, generator_svc, file=None):
        import yaml
        sources = yaml.safe_load(
            read_file(file).decode()).pop('generator_sources')
        call_rpc('source.undeploy',
                 dict(generator_sources=sources),
                 target=generator_svc)
        print(f'{len(sources)} generator source configuration(s) undeployed')
        print()

    def generator_source_create(self, i, kind, sampling, target, generator_svc,
                                params):
        p_params = {}
        payload = {
            'name': i,
            'kind': kind,
            'sampling': sampling,
            'params': p_params
        }
        if target:
            payload['targets'] = target
        if params:
            for param in params:
                (name, value) = param.split('=', maxsplit=1)
                try:
                    value = int(value)
                except:
                    try:
                        value = float(value)
                    except:
                        pass
                p_params[name] = value
        call_rpc('source.deploy',
                 dict(generator_sources=[payload]),
                 target=generator_svc)
        ok()

    def generator_source_plan(self, kind, sampling, duration, fill, output,
                              generator_svc, params):
        p_params = {}
        payload = {
            'name': '',
            'kind': kind,
            'sampling': sampling,
            'params': p_params
        }
        if params:
            for param in params:
                (name, value) = param.split('=', maxsplit=1)
                try:
                    value = int(value)
                except:
                    try:
                        value = float(value)
                    except:
                        pass
                p_params[name] = value
        data = call_rpc('source.plan',
                        dict(source=payload, duration=duration, fill=fill),
                        target=generator_svc)
        if output == 'table':
            print_result(data, cols=['t', 'value'])
        else:
            from . import charts
            if output == 'bar':
                vals = [(str(x['t']), x['value']) for x in data]
                charts.plot_bar_chart(vals)
            elif output == 'line':
                vals = [x['value'] for x in data]
                max_value_width = 0
                for z in vals:
                    x = len(str(z))
                    if x > max_value_width:
                        max_value_width = x
                width, height = get_term_size()
                vals = vals[:width - max_value_width - 6]
                charts.plot_line_chart(vals, 20, max_value_width)

    def generator_source_apply(self,
                               kind,
                               sampling,
                               start,
                               target,
                               generator_svc,
                               params,
                               end=None):
        p_params = {}
        payload = {
            'name': '',
            'kind': kind,
            'sampling': sampling,
            'params': p_params
        }
        if params:
            for param in params:
                (name, value) = param.split('=', maxsplit=1)
                try:
                    value = int(value)
                except:
                    try:
                        value = float(value)
                    except:
                        pass
                p_params[name] = value
        result = call_rpc('source.apply',
                          dict(source=payload,
                               t_start=start,
                               t_end=end,
                               targets=[] if target is None else target),
                          target=generator_svc)
        if result is None:
            ok()
        else:
            import uuid
            result['job_id'] = str(uuid.UUID(bytes=result['job_id']))
            if current_command.json:
                print_result(result)
            else:
                print('apply job started:', result['job_id'])
                print()

    def generator_source_export(self, i, generator_svc, output=None):
        c = 0
        configs = []
        for source in call_rpc('source.list', target=generator_svc):
            name = source['name']
            if (i.startswith('*') and name.endswith(i[1:])) or \
                (i.endswith('*') and name.startswith(i[:-1])) or \
                (i.startswith('*') and i.endswith('*') and i[1:-1] in name) or \
                i == name:
                c += 1
                config = call_rpc('source.get_config',
                                  dict(i=name),
                                  target=generator_svc)
                configs.append(config)
        result = dict(generator_sources=configs)
        if current_command.json:
            print_result(result)
        else:
            import yaml
            dump = yaml.dump(result, default_flow_style=False)
            if output is None:
                print(dump)
            else:
                write_file(output, dump)
                print(f'{c} generator source configuration(s) exported')
                print()

    def generator_source_destroy(self, i, generator_svc):
        call_rpc('source.destroy', dict(i=i), target=generator_svc)
        ok()

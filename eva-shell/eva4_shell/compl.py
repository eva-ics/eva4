import os
from .client import call_rpc
from .sharedobj import common


class ComplOID:

    def __init__(self, fn='list'):
        self.fn = fn

    def __call__(self, prefix, **kwargs):
        if ':' not in prefix and prefix != '#':
            for k in ['unit:', 'sensor:', 'lvar:', 'lmacro:', '#', '+:']:
                if k.startswith(prefix):
                    yield k
        else:
            if '/' in prefix:
                mask = prefix.rsplit('/', 1)[0] + '/#'
            else:
                mask = prefix.split(':', 1)[0] + ':#'
            yield mask
            for r in call_rpc(f'item.{self.fn}', dict(i=mask)):
                yield r['oid']


class ComplOIDtp:

    def __init__(self, tp):
        self.tp = tp

    def __call__(self, prefix, **kwargs):
        if ':' not in prefix:
            yield f'{self.tp}:'
        else:
            if '/' in prefix:
                mask = prefix.rsplit('/')[0] + '/#'
            else:
                mask = prefix.rsplit(':')[0] + ':#'
            yield mask
            for r in call_rpc('item.list', dict(i=mask)):
                yield r['oid']


class ComplNode:

    def __call__(self, prefix, **kwargs):
        for r in call_rpc('node.list'):
            yield r['name']


class ComplSvc:

    def __init__(self, tp=None):
        self.prefix = None if tp is None else f'eva.{tp}.'

    def __call__(self, prefix, **kwargs):
        for r in call_rpc('svc.list'):
            i = r['id']
            if self.prefix is None or i.startswith(self.prefix):
                yield i


class ComplSvcRpcMethod:

    def __call__(self, prefix, parsed_args=None, **kwargs):
        if parsed_args:
            result = call_rpc('info', target=parsed_args.i)
            methods = list(result.get('methods', {})) + ['test', 'info']
            for m in methods:
                if m.startswith(prefix):
                    yield m


class ComplSvcRpcParams:

    def __call__(self, prefix, parsed_args=None, **kwargs):
        if parsed_args and not prefix.endswith('='):
            pp = [p.split('=')[0] for p in parsed_args.params]
            result = call_rpc('info', target=parsed_args.i)
            method_params = result.get('methods',
                                       {}).get(parsed_args.method,
                                               {}).get('params', {})
            for p in method_params:
                if p.startswith(prefix) and not p in pp:
                    yield f'{p}='


class ComplDeployFile:

    def __call__(self, prefix, **kwargs):
        import glob
        expanded = None

        def expand_user(pfx):
            nonlocal expanded
            if pfx.startswith('~'):
                expanded = os.path.expanduser('~')
                return expanded + pfx[1:]
            else:
                return pfx

        def contract_user(pfx):
            nonlocal expanded
            if expanded is None:
                return pfx
            else:
                return '~' + pfx[len(expanded):]

        if not prefix:
            masks = ['*.yml', '*.yaml', '*.json']
        elif prefix.endswith('.yml') or prefix.endswith(
                '.yaml') or prefix.endswith('.json'):
            yield prefix
        elif prefix.endswith('.'):
            prefix = expand_user(prefix)
            masks = [f'{prefix}*yml', f'{prefix}*yaml', f'{prefix}*json']
        else:
            prefix = expand_user(prefix)
            masks = [f'{prefix}*.yml', f'{prefix}*.yaml', f'{prefix}*.json']
        for mask in masks:
            for f in glob.glob(mask):
                yield contract_user(f)
            for f in glob.glob(f'{prefix}*'):
                if os.path.isdir(f):
                    yield contract_user(f'{f}/')


class ComplEdit:

    configs = [
        'config/core', 'config/bus', 'config/logs', 'config/python-venv',
        'config/registry', 'config/cloud-manager'
    ]

    def __call__(self, prefix, **kwargs):
        filemgr_svc = kwargs['parsed_args'].filemgr_svc
        yield 'xc/'
        if not prefix.startswith('x'):
            for c in self.configs:
                yield c
        elif prefix.startswith('xc/'):
            p = prefix.rsplit('/', maxsplit=1)
            pfx_dir = p[0]
            try:
                pmask = f'{p[1]}*'
            except:
                pmask = None
            try:
                f1 = call_rpc('list', {
                    'path': prefix,
                    'recursive': True
                },
                              target=filemgr_svc)
            except:
                f1 = []
            try:
                if pmask:
                    f2 = call_rpc('list', {
                        'path': pfx_dir,
                        'masks': [pmask]
                    },
                                  target=filemgr_svc)
                else:
                    raise Exception
            except:
                f2 = []
            for f in f1 + f2:
                path = f'{pfx_dir}/{f["path"]}'
                if f['kind'] == 'dir':
                    path += '/'
                yield path

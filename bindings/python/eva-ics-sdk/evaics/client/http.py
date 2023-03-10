import requests
import json
import msgpack

from functools import partial
from ..sdk import __version__
from ..sdk import rpc_e2e, pack, unpack
from ..sdk import ERR_CODE_ACCESS_DENIED

from types import SimpleNamespace

import threading
import logging

logger = logging.getLogger('evaics.client.http')


class Client:

    def __init__(self, url: str):
        self.login = None
        self.password = None
        self.api_key = None
        self._token = None
        self._post = partial(requests.post,
                             url,
                             headers={
                                 'content-type': 'application/msgpack',
                                 'user-agent': f'evaics-py {__version__}'
                             })
        self._url = url
        self.call_id = 1
        self.lock = threading.RLock()

    def _get_call_id(self):
        with self.lock:
            self.call_id += 1
            if self.call_id > 0xFFFF_FFFF:
                self.call_id = 1
            return self.call_id

    def test(self):
        return self.call('test')

    def authenticate(self):
        if self.login is None or self.password is None:
            raise RuntimeError('credentials not set')
        result = self.call('login', dict(u=self.login, p=self.password))
        self._token = result['token']

    def call(self, method: str, params: dict = None):
        params = {} if params is None else params
        logger.info(f'{self._url}::{method}')
        call_id = self._get_call_id()
        req = {
            'jsonrpc': '2.0',
            'id': call_id,
            'method': method,
            'params': params
        }
        need_k = method != 'login'
        token_auth = False
        if need_k:
            if self.api_key is not None:
                params['k'] = self.api_key
            elif self._token is not None:
                params['k'] = self._token
                token_auth = True
            else:
                self.authenticate()
                params['k'] = self._token
                # do not attempt to refresh newly issued tokens
        result = self._post(data=pack(req))
        if need_k:
            del params['k']
        if result.ok:
            payload = unpack(result.content)
            i = payload.get('id')
            if i is None:
                raise RuntimeError('Invalid API response')
            elif i != call_id:
                raise RuntimeError('Invalid API response ID')
            error = payload.get('error')
            if error is not None:
                code = error.get('code')
                if code == ERR_CODE_ACCESS_DENIED and token_auth:
                    self._token = None
                    return self.call(method, params)
                raise rpc_e2e(
                    SimpleNamespace(rpc_error_code=code,
                                    rpc_error_payload=error.get('message', '')))
            return payload.get('result')
        else:
            raise RuntimeError(
                f'API error, http code: {result.status_code}: {result.text}')

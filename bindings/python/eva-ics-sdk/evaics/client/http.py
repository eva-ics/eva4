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
    """
    HTTP client for EVA ICS (HMI HTTP API)

    Requires "requests" Python module to be manually installed
    """

    def __init__(self,
                 url: str = 'http://localhost:7727',
                 user: str = None,
                 password: str = None,
                 key: str = None,
                 timeout: float = 120):
        """
        Create a new HTTP client instance

        Args:
            url: HMI URL (proto://host:port)
        """
        import requests
        self.url = url
        self.user = user
        self.password = password
        self.key = key
        self.token = None
        self.timeout = timeout
        self._post = partial(requests.post,
                             url,
                             headers={
                                 'content-type': 'application/msgpack',
                                 'user-agent': f'evaics-py {__version__}'
                             },
                             timeout=timeout)
        self._call_id = 1
        self._lock = threading.RLock()

    def credentials(self, user: str, password: str):
        """
        Set authentication credentials

        Args:
            user: user name
            password: user password
        """
        self.user = user
        self.password = password
        return self

    def connect(self):
        """
        Blank method, tests HTTP connection only
        """
        import requests
        requests.get(self.url)

    def api_key(self, api_key: str):
        """
        Authenticate with API key

        Args:
            api_key: API key
        """
        self.key = api_key
        return self

    def _get_call_id(self):
        with self._lock:
            if self._call_id == 0xFFFF_FFFF:
                self._call_id = 1
            else:
                self._call_id += 1
            return self._call_id

    def test(self):
        """
        Call server test method

        Returns:
            API response payload object (server and session info)
        """
        from . import ServerInfo
        return ServerInfo(self.call('test'))

    def authenticate(self):
        """
        Authenticate the client

        Authenticates the client and stores the authentication token. The
        method may be called manually but is not mandatory to use
        """
        if self.user is None or self.password is None:
            raise RuntimeError('credentials not set')
        result = self.call('login', dict(u=self.user, p=self.password))
        self.token = result['token']

    def bus_call(self, method: str, params: dict = None, target='eva.core'):
        """
        Call BUS/RT EAPI method

        Requires admin permissions

        Args:
            method: API method

        Optional:

            params: API method parameters (dict)

            target: target service (default: eva.core)

        Returns:
            API response payload
        """
        return self.call(f'bus::{target}::{method}', params)

    def call(self, method: str, params: dict = None):
        """
        Call server API method

        Args:
            method: API method

        Optional:

            params: API method parameters (dict)

        Returns:
            API response payload
        """
        params = {} if params is None else params
        logger.info(f'{self.url}::{method}')
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
            if self.key is not None:
                params['k'] = self.key
            elif self.token is not None:
                params['k'] = self.token
                token_auth = True
            else:
                self.authenticate()
                params['k'] = self.token
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
                    self.token = None
                    return self.call(method, params)
                raise rpc_e2e(
                    SimpleNamespace(rpc_error_code=code,
                                    rpc_error_payload=error.get('message', '')))
            return payload.get('result')
        else:
            raise RuntimeError(
                f'API error, http code: {result.status_code}: {result.text}')

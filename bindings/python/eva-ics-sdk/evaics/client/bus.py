import busrt
import msgpack
import sys
import os

from functools import partial
from pathlib import Path

from ..sdk import __version__
from ..sdk import rpc_e2e, pack, unpack

from types import SimpleNamespace

import logging

logger = logging.getLogger('evaics.client.bus')


class Client:
    """
    BUS/RT client for EVA ICS (EAPI)
    """

    def __init__(self,
                 path: str = '/opt/eva4/var/bus.ipc',
                 name: str = None,
                 timeout: float = 120):
        """
        Create a new BUS/RT client instance

        Args:
            path: BUS/RT socket (default: /opt/eva4/var/bus.ipc)
            name: client name (default: PROGRAM.PID)
        """
        self.path = path
        self.name = name or f'{Path(sys.argv[0]).stem}.{os.getpid()}'
        self.timeout = timeout
        self.bus = None
        self.rpc = None

    def connect(self):
        """
        Connects the client
        """
        bus = busrt.client.Client(self.path, self.name)
        bus.timeout = self.timeout
        bus.connect()
        self.bus = bus
        self.rpc = busrt.rpc.Rpc(bus)

    def test(self):
        """
        Call eva.core test method

        Returns:
            API response payload object
        """
        from . import ServerInfo
        return ServerInfo(self.bus_call('test'))

    def authenticate(self):
        """
        Authenticate the client

        Blank method
        """
        pass

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
        if self.rpc is None:
            raise RuntimeError('client not connected')
        try:
            payload = self.rpc.call(
                target,
                busrt.rpc.Request(
                    method, None if params is None else
                    msgpack.dumps(params))).wait_completed().get_payload()
            if payload:
                return unpack(payload)
            else:
                return None
        except busrt.rpc.RpcException as e:
            raise rpc_e2e(e)

    def call(self, *args, **kwargs):
        """
        Alias for bus_call
        """
        return self.bus_call(*args, **kwargs)

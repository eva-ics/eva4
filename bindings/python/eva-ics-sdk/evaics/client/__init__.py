from .http import Client as HttpClient
from .bus import Client as BusClient
from .cloud import Client as CloudClient


class ServerInfo:

    def __init__(self, payload):
        self.__dict__.update(payload)


class Client:
    """
    BUS/RT HTTP client class

    Automatically connects either via BUS/RT or HTTP

    Optional:

        path: BUS/RT path or HTTP URI (default: /opt/eva4/var/bus.ipc')

        kwargs passed to client as-is

    """

    def __init__(self, path='/opt/eva4/var/bus.ipc', **kwargs):
        if path.startswith('http://') or path.startswith('https://'):
            self.client = HttpClient(url=path, **kwargs)
        else:
            self.client = BusClient(path=path, **kwargs)
        self.authenticate = self.client.authenticate
        self.bus_call = self.client.bus_call
        self.call = self.client.call
        self.connect = self.client.connect
        self.test = self.client.test

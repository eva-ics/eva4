from .http import Client as HttpClient


class ServerInfo:

    def __init__(self, payload):
        self.__dict__.update(payload)

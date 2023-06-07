__version__ = '0.0.1'

import evaics.sdk as sdk
from elasticsearch import Elasticsearch

from evaics.sdk import unpack, LOG_EVENT_TOPIC

from types import SimpleNamespace

d = SimpleNamespace(es=None, index=None, service=None)


def on_frame(frame):
    if frame.topic and frame.topic.startswith(LOG_EVENT_TOPIC):
        data = unpack(frame.payload)
        # do not process logs from the service itself
        if data.get('msg', '').startswith(f'{d.service.id} '):
            return
        d.es.index(index=d.index, document=data)


def run():
    info = sdk.ServiceInfo(author='Bohemia Automation',
                           description='EAPI to ElasticSearch',
                           version=__version__)
    service = sdk.Service()
    config = service.get_config()
    config_client = config['client']
    if not 'request_timeout' in config_client:
        config_client['request_timeout'] = service.timeout['default']
    d.es = Elasticsearch(**config_client)
    d.index = config.get('index', 'logs-eva')

    d.service = service
    d.timeout = service.timeout['default']

    service.init(info, on_frame=on_frame)
    service.bus.subscribe(f'{LOG_EVENT_TOPIC}#').wait_completed()
    service.block()


run()

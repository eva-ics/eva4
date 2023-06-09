__version__ = '0.0.1'

import evaics.sdk as sdk
import time
import traceback
import threading
import logging
import sys

from elasticsearch import Elasticsearch, logger as el_logger
from elasticsearch.helpers import bulk as el_bulk
from evaics.sdk import unpack, LOG_EVENT_TOPIC
from types import SimpleNamespace

LOG_PROCESS_DELAY = 1

d = SimpleNamespace(es=None,
                    index=None,
                    service=None,
                    records=[],
                    record_lock=threading.Lock())


def on_frame(frame):
    if frame.topic and frame.topic.startswith(LOG_EVENT_TOPIC):
        data = unpack(frame.payload)
        # do not process logs from the service itself
        if data.get('msg', '').startswith(f'{d.service.id} '):
            return
        with d.record_lock:
            d.records.append(data)


def logs_worker():
    while d.service.is_active():
        time.sleep(LOG_PROCESS_DELAY)
        process_logs()


def process_logs():
    try:
        with d.record_lock:
            records = d.records
            d.records = []
        if records:
            actions = [{'_index': d.index, '_source': l} for l in records]
            el_bulk(d.es, actions)
    except Exception as e:
        print(e, file=sys.stderr, flush=True)


def run():
    logging.getLogger('elasticsearch').setLevel(logging.WARNING)
    logging.getLogger('elastic_transport.transport').setLevel(logging.WARNING)
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

    service.init(info, on_frame=on_frame)
    service.bus.subscribe(f'{LOG_EVENT_TOPIC}#').wait_completed()
    threading.Thread(target=logs_worker, daemon=True).start()
    service.block()
    process_logs()


run()

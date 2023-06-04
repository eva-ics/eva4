__version__ = '0.0.1'

import evaics.sdk as sdk
import json

from evaics.sdk import unpack, OID, LOCAL_STATE_TOPIC, LOG_EVENT_TOPIC

from types import SimpleNamespace
from kafka3 import KafkaProducer

d = SimpleNamespace(producer=None,
                    topic_state=None,
                    topic_log=None,
                    service=None,
                    timeout=None)


def on_frame(frame):
    if frame.topic:
        if d.topic_state and frame.topic.startswith(LOCAL_STATE_TOPIC):
            kafka_topic = d.topic_state
            key = str(OID(frame.topic[len(LOCAL_STATE_TOPIC):], from_path=True))
            value = json.dumps(unpack(frame.payload))
        elif d.topic_log and frame.topic.startswith(LOG_EVENT_TOPIC):
            kafka_topic = d.topic_log
            data = unpack(frame.payload)
            # do not process logs from the service itself
            if data.get('msg', '').startswith(f'{d.service.id} '):
                return
            key = frame.topic[len(LOCAL_STATE_TOPIC):]
            value = json.dumps(data)
        else:
            kafka_topic = None
        if kafka_topic:
            d.producer.send(kafka_topic, key=key.encode(),
                            value=value.encode()).get(timeout=d.timeout)


def run():
    info = sdk.ServiceInfo(author='Bohemia Automation',
                           description='EAPI to Apache Kafka',
                           version=__version__)
    service = sdk.Service()
    config = service.get_config()

    kafka_config = config['kafka']
    kafka_target = kafka_config['target']
    d.producer = KafkaProducer(bootstrap_servers=kafka_target,
                               api_version=(0, 10))
    kafka_topics = kafka_config.get('topics', {})

    d.topic_state = kafka_topics.get('state')
    d.topic_log = kafka_topics.get('log')

    d.service = service
    d.timeout = service.timeout['default']

    service.init(info, on_frame=on_frame)
    if d.topic_state:
        service.subscribe_oids(config.get('oids', []), event_kind='local')
    if d.topic_log:
        service.bus.subscribe(f'{LOG_EVENT_TOPIC}#').wait_completed()
    service.logger.info(f'target: {kafka_target}')
    service.block()


run()

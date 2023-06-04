# bus-to-kafka svc

EVA ICS v4 local bus to Apache Kafka bridge.

Written in Python, requires EVA ICS v4 and EVA ICS venv set. Requires an
additional "kafka-python3" module:

```
eva venv add kafka-python3
```

## What does it do

The bridge collects bus log events and events for local items and sends them to
Apache Kafka server.

## Setup

Download [bus-to-kafka.py](bus-to-kafka.py), copy-paste the service template:

```yaml
bus:
  path: var/bus.ipc
# service location
command: venv/bin/python /opt/eva4/contrib/bus-to-kafka/bus-to-kafka.py
config:
  # OIDS to subscribe
  oids:
  - "#"
  kafka:
    targets:
      - 127.0.0.1:9092
    topics:
      state: state
      log: log
user: nobody
workers: 1
```

(edit the command path if necessary), then run

```shell
eva svc create eva.bridge.kafka1 /path/to/template.yml
```

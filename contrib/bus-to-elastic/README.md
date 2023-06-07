# bus-to-elastic svc

EVA ICS v4 local bus to ElasticSearch bridge.

Written in Python, requires EVA ICS v4 and EVA ICS venv set. Requires an
additional "elasticsearch" module (tested with 8.8.0):

```
eva venv add elasticsearch
```

## What does it do

The bridge collects bus log events and sends them to a ElasticSearch server.

## Setup

Download [bus-to-elastic.py](bus-to-elastic.py), copy-paste the service
template:

```yaml
bus:
  path: var/bus.ipc
# service location
command: venv/bin/python /opt/eva4/contrib/bus-to-elastic/bus-to-elastic.py
config:
  # passed to the client as-is. see https://elasticsearch-py.readthedocs.io
  client:
    hosts:
      - host: localhost
        port: 9200
        scheme: http
    basic_auth: ['elastic', 'changeme']
  # index name to store logs in
  index: logs-eva
user: nobody
workers: 1
```

(edit the command path if necessary), then run

```shell
eva svc create eva.bridge.elastic1 /path/to/template.yml
```

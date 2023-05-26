# bus-to-udp svc

EVA ICS v4 local bus to UDP bridge.

Written in Python, requires EVA ICS v4 and EVA ICS venv set. No additional
Python modules are required.

## What does it do

The bridge collects bus events for local items and sends them via UDP to a
chosen target.

The UDP payload can be encoded in MessagePack (default) or JSON.

The UDP payload contains additional "oid" field with item OID.

## Setup

Download [bus-to-udp.py](bus-to-udp.py), copy-paste the service template:

```yaml
bus:
  path: var/bus.ipc
# service location
command: venv/bin/python /opt/eva4/contrib/bus-to-udp/bus-to-udp.py
config:
  # OIDS to subscribe
  oids:
  - sensor:nr/temp
  # target to send UDP events to
  target: 172.17.0.2:9999
  # msgpack (default) or json
  #format: json
user: nobody
workers: 1
```

(edit the command path if necessary), then run

```shell
eva svc create eva.bridge.udp1 /path/to/template.yml
```

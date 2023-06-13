# raw-udp-to-bus svc

Raw UDP to EVA ICS v4 local bus bridge.

Written in Python, requires EVA ICS v4 and EVA ICS venv set. No additional
Python modules are required.

## What does it do

The service opens UDP input ports (one per item) which allows to connect
various 3rd party software, such as Matlab Simulink (using DSP system toolbox
-> Sinks -> UDP send) with paced simulation models.

![Simulink UDP](ss1.png?raw=true)

## Setup

Download [raw-udp-to-bus.py](raw-udp-to-bus.py), copy-paste the service
template:

```yaml
bus:
  path: var/bus.ipc
# service location
command: venv/bin/python /opt/eva4/contrib/raw-udp-to-bus/raw-udp-to-bus.py
config:
  # ip address to listen to
  listen: 0.0.0.0
  map:
    # UDP ports, one per item
    # 
    # The service sets item values only, status register is always 1
    25001:
      # for decode options, see https://docs.python.org/3/library/struct.html,
      # the default encoding is "<d" (little-endian IEE754 double)
      decode: "<d"
      oid: sensor:tests/t1
    25002:
      oid: sensor:tests/t2
user: nobody
workers: 1
```

(edit the command path if necessary), then run

```shell
eva svc create eva.controller.raw_udp1 /path/to/template.yml
```

## EAPI methods

The service provides an additional RPC method "port.list" which output all the
mapped ports:

```shell
eva svc call eva.controller.raw_udp1 port.list
```

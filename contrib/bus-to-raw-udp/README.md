# bus-to-raw-udp svc

EVA ICS v4 local bus to raw UDP bridge.

Written in Python, requires EVA ICS v4 and EVA ICS venv set. No additional
Python modules are required.

## What does it do

The service works in two modes:

* If interval is specified, it collects states of the specified EVA ICS items
with the chosen sampling rate and sends them to the specified UDP ports.

* If interval is not specified, UDP packets are sent on bus events only.

Using with Matlab Simulink:

![Simulink UDP](ss.png?raw=true)

* For interval sampling, use DSP System Toolbox -> Sources -> UDP Receive. The
sampling rate must match the service interval.

* For event processing, use Instrument Control Toolbox -> UDP Receive in
non-blocking mode plus DSP System Toolbox -> Signal Operations -> Sample and
Hold.

For the opposite operation, see [raw-bus-to-udp](../raw-udp-to-bus)

## Setup

Download [bus-to-raw-udp](bus-to-raw-udp.py), copy-paste the service template:

```yaml
bus:
  path: var/bus.ipc
# service location
command: venv/bin/python /opt/eva4/contrib/bus-to-raw-udp/bus-to-raw-udp.py
config:
  # ip address to send packets to
  target: 127.0.0.1
  source_port: 9999
  # sampling interval. if specified, real-time events are not processed
  #interval: 1
  map:
    # UDP ports, one per item
    # 
    # The service sets item values only, status register is always 1
    sensor:tests/t1:
      # for encode options, see https://docs.python.org/3/library/struct.html,
      # the default encoding is "<d" (little-endian IEE754 double)
      encode: "<d"
      port: 35001
    sensor:tests/t2:
      port: 35002
user: nobody
workers: 1
```

(edit the command path if necessary), then run

```shell
eva svc create eva.bridge.raw_udp1 /path/to/template.yml
```

## EAPI methods

The service provides an additional RPC method "port.list" which output all the
mapped ports:

```shell
eva svc call eva.bridge.raw_udp1 port.list
```

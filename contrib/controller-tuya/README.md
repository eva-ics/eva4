# Tuya services

## Tuya controller

[Tuya controller](https://www.tuya.com)

### What does it do

Tuya controller allows to connect Tuya devices to [EVA
ICS](https://www.eva-ics.com).

### Setup

Install required modules:

```
eva venv add tinytuya==1.15.1
```

Download
[controller-tuya.py](https://github.com/eva-ics/eva4/blob/main/contrib/controller-tuya/controller-tuya.py),
copy-paste the service template:

```yaml
bus:
  path: var/bus.ipc
# service location
command: venv/bin/python /opt/eva4/contrib/controller-tuya/controller-tuya.py
config:
  device:
    # Tuya device ID
    id: DEVICE_ID
    # Tuya device IP address
    addr: DEVICE_IP_ADDR
    # Tuya device local key
    local_key: DEVICE_LOCAL_KEY
    # Tuya device API version: 3.1, 3.2, 3.3, 3.4 or 3.5
    version: 3.3
  # polling interval in seconds
  pull_interval: 2
  # DPS - EVA ICS OID map
  map:
    '16':
      oid: 'unit:t1/switch'
    '101':
      oid: 'sensor:t1/voltage'
      # divide by 10
      factor: 10
    '103':
      oid: 'sensor:t1/temp'
  # Action map
  action_map:
    'unit:t1/switch':
      dps: '16'
      # control or set_value
      kind: control
      # convert action value to boolean
      convert_bool: true
react_to_fail: true
user: nobody
workers: 1
```

### Configuration

* Each Tuya device requires own service instance

* The service requires Tuya local key for the device. The key can be obtained
  using [Tuya Cloud Explorer](https://eu.platform.tuya.com/cloud/explorer)

## Tuya discovery service

### Setup

Install required modules:

```
eva venv add pycryptodomex==3.11.0
```

Download
[tuya-discover.py](https://github.com/eva-ics/eva4/blob/main/contrib/controller-tuya/tuya-discover.py),

The service does not require any configuration, use a dummy template:

```shell
eva svc create eva.svc.tuya /opt/eva4/share/svc-tpl/svc-tpl-dummy.yml
```

(replace "command" field to `venv/bin/python /path/to/tuya-discover.py`)

### Usage

The service responds to `discover` command and returns a list of Tuya devices
in the local network:

```shell
eva -T15 -J svc call eva.svc.tuya discover discovery_timeout=10
```

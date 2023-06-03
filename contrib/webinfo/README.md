# webinfo svc

EVA ICS v4 RESTFul Web Info (node test, item states) (Rust/Axum)

## Usage

All requests must contain a header X-Auth-Key with a valid API key or token

Methods:

* /api/test - get node info (core test)

* /api/item.state/OID - get item/group state (OID as path, e.g. sensor/tests/t1)

## Setup

Compile with:

```
cargo build --release
```

copy-paste the service template:

```yaml
bus:
  path: var/bus.ipc
# service location
command: /path/to/eva-webinfo
config:
  # ip/port listen to
  listen: 0.0.0.0:9922
  # the service uses a HMI service instance to authenticate API calls. An
  # instance must be deployed on the node
  hmi_svc: eva.hmi.default
  # optional real ip header
  #real_ip_header: X-Real-Ip
user: nobody
workers: 1
```

(edit the command path), then run

```shell
eva svc create eva.svc.webinfo /path/to/template.yml
```

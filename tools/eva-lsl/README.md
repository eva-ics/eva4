# EVA ICS v4 Local Service Launcher

Local service launcher for [EVA ICS v4](https://www.eva-ics.com) industrial
automation platform.

Allows to run and test EVA ICS services locally.

More info about EVA ICS Rust SDK: <https://info.bma.ai/en/actual/eva4/sdk/rust/index.html>

## Installation

```bash
cargo install eva-lsl
```

## Usage

### Creating a basic service

```bash
eva-lsl new myservice
```

See also: <https://info.bma.ai/en/actual/eva4/sdk/rust/service_example.html>

### Testing a service

* Deploy the service configuration to a EVA ICS node. The service can be
  disabled and point to any non-existing command.

* Allow remote connections to the node (`eva edit config/bus`)

* Inside a Rust service project, run:

```bash
eva-lsl run -b IP:PORT SVC_ID
```

The command builds the service and run it locally, connecting to the node bus.
Log messages are printed to the local console.

The command can also override, the service bus id, user, data path etc. Check
other options with `eva-lsl --help`.

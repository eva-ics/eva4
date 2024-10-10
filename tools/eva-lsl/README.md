# EVA ICS v4 Local Service Launcher

Local service launcher for [EVA ICS v4](https://www.eva-ics.com) industrial
automation platform.

Allows to run and test EVA ICS services locally.

## Installation

```bash
cargo insall eva-lsl
```

## Usage

* Deploy the service configuration to a EVA ICS node. The service can be
  disabled and point to any non-existing command.

* Allow remote connections to the node (`eva edit config/bus`)

* Inside a Rust service project, run:

```bash
eva-lsl -b IP:PORT svc.id
```

The command builds the service and run it locally, connecting to the node bus.
Log messages are printed to the local console.

The command can also override, the service bus id, user, data path etc. Check
other options with `eva-lsl --help`.

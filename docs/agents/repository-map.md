# Repository Map

This file maps the major first-party areas of the repository. It is not a full
tree dump; it is the shortest useful map for editing work.

## Core runtime and platform

- `eva/`:
  Node core crate. Builds `eva-node`, `init-registry`, and `rt-launch`.
- `share/registry/`:
  Registry schema and default configuration keys used by the core.
- `share/svc-tpl/`:
  Deployment templates for built-in services. Add or update templates here when
  a service interface changes.
- `runtime/`:
  Runtime-oriented skeleton data such as registry and cross-compile support
  assets.
- `etc/`, `bin/`, `sbin/`, `install/`:
  System integration, packaging, and deployment support files.

## First-party services

- `svc/aaa-*`:
  Authentication and authorization services. Elevated review risk.
- `svc/controller-*`:
  Fieldbus, protocol, and controller services. Many are boundary/parsing-heavy.
- `svc/db-*`:
  Database and persistence backends.
- `svc/repl`, `svc/repl-uni`, `svc/repl-legacy`:
  Replication services. `repl-legacy` is intentionally legacy.
- `svc/filemgr`, `svc/hmi`, `svc/mirror`, `svc/eva-mcp`:
  HTTP, file, UI, or integration-facing services.
- `svc/svc-template`:
  The canonical starting point for a new Rust service in this repo.
- `svc/svc-example-*`:
  Example crates excluded from the main workspace but still useful as references.
- `svc/controller-py`, `svc/aaa-ldap`, `svc/bridge-udp`, `svc/svc-tts`,
  `svc/svc-yolo-detector`, `svc/repl-legacy`:
  Python-based or non-workspace service trees that still matter operationally.

## Shared crates and support libraries

- `eva-internal/`:
  Internal shared functionality used by multiple services.
- `eva-gst/`:
  GStreamer plugins for EVA-related multimedia work.
- `psrpc/`:
  Shared RPC/helper crate used by several services.
- `dobj-codegen/`:
  Data-object code generation utility.
- `logreducer/`:
  Shared log compaction/filtering helper used by controller services.
- `tracing-async-mutex/`, `genpass-native/`, `crypto-tools/`:
  Focused support crates/utilities.

## Bindings and client tooling

- `bindings/js/`:
  Official JS/TS SDK package.
- `bindings/python/eva-ics-sdk/`:
  Official Python SDK package.
- `bindings/python/examples/`:
  Python service examples worth reading before writing Python services.
- `cli-js/`:
  Standalone JS CLI package.
- `eva-shell/`:
  EVA shell packaging/distribution subtree.
- `tools/eva-cloud-manager/`, `tools/gen-intl/`:
  Workspace tools.
- `tools/eva-lsl/`:
  Excluded from the workspace, but useful for local service launch/testing.

## Packaging, deployment, and ops assets

- `docker/`:
  Container deployment and test environment files.
- `docker.cross/`:
  Cross-compilation image definitions.
- `dev/`:
  Release, versioning, and repository maintenance scripts.
- `share/`:
  Templates, registry schema, and distributable shared assets.

## Secondary or specialized areas

- `benchmarks/`:
  Benchmark crates.
- `eva_pg/`:
  Separate crate kept outside the main workspace.
- `ui/`, `vendored-apps/`:
  UI assets and bundled applications. Inspect locally before changing; they do
  not follow the same workflow as the Rust workspace.
- `pvt/`, `var/`, `misc/`:
  Support data and project-specific extras.

## Out of scope by default

- `contrib/`:
  Present in the repository, but excluded from this documentation initiative
  and from agent assumptions unless the task explicitly requests it.

# Core And Services

This file describes the architectural center of the repository: the node core
and the first-party service families around it.

## The node core

`eva/` is the node server crate. It exposes:

- `eva-node`:
  Main node process.
- `init-registry`:
  Initializes/imports registry defaults and schema.
- `rt-launch`:
  Launch helper for real-time service execution.

## Core module map

The files in `eva/src/` are named clearly enough to use as a navigation map:

- `main.rs`:
  CLI entry point for `eva-node`.
- `lib.rs`:
  Shared constants, environment helpers, system configuration, and runtime
  support.
- `node.rs`:
  Top-level node startup flow, broker/core initialization, and runtime mode
  selection.
- `bus.rs`:
  BUS/RT broker wrapper and bus configuration handling.
- `eapi.rs`:
  Core EAPI RPC/frame handling and bus-facing control surface.
- `core.rs`:
  Main domain logic for items, replication, actions, and node operations.
- `items.rs`:
  Item state/config structures and inventory logic.
- `actmgr.rs`:
  Action manager and action lifecycle handling.
- `svcmgr.rs`:
  Service management, lifecycle, deployment, and launcher coordination.
- `launcher.rs`:
  Service process launch and supervision support.
- `inventory_db.rs`:
  Persistent inventory/state storage backend integration.
- `logs.rs`:
  Memory/bus/console log handling.
- `spoint.rs`:
  Secondary point/local clustering support.
- `seq.rs`:
  Sequence execution support.
- `regsvc.rs`:
  Registry service configuration loading.
- `svc.rs`:
  Small service-facing helpers.

## Service families

The `svc/` tree is organized more by behavior than by language.

- `aaa-*`:
  Authentication, authorization, and related identity flows.
- `controller-*`:
  Hardware, fieldbus, network, or protocol controllers.
- `db-*`:
  Persistence/event archiving services.
- `repl*`:
  Replication and cluster transport.
- `hmi`, `filemgr`, `mirror`, `eva-mcp`:
  UI, HTTP, filesystem, or integration endpoints.
- `svc-*`:
  Generic services and templates.
- `ffi`, `gst-pipeline`, `videosink`:
  FFI, multimedia, and boundary-heavy integrations.

## Modern versus legacy service style

The repository contains more than one service style on purpose.

- Modern Rust services typically use `eva-sdk`, `#[svc_main]`, and `eapi_bus`.
- Some existing Rust services still use direct `busrt` flows or compatibility
  shims because that is their current contract surface.
- Some existing services also use older SDK helper patterns built around
  `svc_*` functions. Treat that as legacy style: preserve it in legacy services,
  but do not use it as the default for new services.
- Python services and examples use the Python SDK plus direct BUS/RT concepts.
- `svc/repl-legacy` is explicitly legacy and should stay that way unless a task
  says otherwise.
- `svc/ffi` contains a compatibility path (`into_legacy_compat`) and should not
  be casually normalized.

## High-risk zones

Read `AICODE.md` before touching these areas:

- `eva/`
- `svc/aaa-*`
- `svc/controller-*`
- `svc/repl*`
- `svc/ffi`
- `crypto-tools`
- Any parser, protocol handler, configuration loader, or external input bridge

In practice, protocol controllers and authentication services should be treated
as Level 2-3 style work even when the diff looks small.

## Operational assets tied to services

When a service changes, the code is rarely the whole change set. Check:

- `share/svc-tpl/` for deployment template changes
- `share/registry/defaults/config/` if config defaults or registry keys change
- `docker/` or packaging assets if the change affects deployment/runtime
- SDK bindings or examples if the external contract changes

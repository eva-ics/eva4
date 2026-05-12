# Service Authoring

Use this file before adding a new service or substantially reshaping an
existing one.

## First choice for new Rust services

Start from `svc/svc-template/`. Its `src/main.rs` shows the expected modern
service lifecycle:

- deserialize config with `#[serde(deny_unknown_fields)]`
- construct `ServiceInfo`
- initialize the bus with `eapi_bus::init(...)`
- drop privileges after bus connection
- initialize logs with `eapi_bus::init_logs(...)`
- start signal handlers
- mark ready, block, then mark terminating

That template is the preferred baseline for new first-party Rust services.

## New service checklist

For a new first-party Rust service, check all of the following:

1. Create the service under `svc/` with the established naming pattern.
2. Base startup/lifecycle on `svc/svc-template`.
3. Reuse `eva-common` and `eva-sdk` types before introducing local copies.
4. Add a deployment template under `share/svc-tpl/`.
5. Add the crate to the workspace if it is intended to be a main workspace
   member.
6. Add tests only where they materially validate local logic; final integration
   testing remains human-run.

## When not to modernize

Do not convert an existing service to a new SDK style unless the task explicitly
asks for it.

Examples of intentionally non-uniform areas:

- services that still use direct `busrt` interactions internally
- services that use older SDK helper style based on `svc_*` functions
- `svc/ffi`, which contains compatibility handling
- Python services and examples built around the Python SDK
- `svc/repl-legacy`, which is legacy by definition

The repository accepts mixed implementation styles where compatibility or
operational continuity matters more than visual consistency.

## Good reference points

- `svc/svc-template/`:
  minimal modern Rust service
- `svc/eva-mcp/`:
  modern Rust service that also exposes an HTTP/MCP surface and config allowlist
- `svc/controller-dobj/`:
  modern Rust controller-style service using `eapi_bus`
- `bindings/python/examples/`:
  sanctioned Python service examples
- `tools/eva-lsl/README.md`:
  optional local workflow for running a service against a node bus during
  development

## Configuration and templates

Service code is only half the contract. Keep these aligned:

- runtime config schema in the service code
- deployment template in `share/svc-tpl/`
- any registry defaults if new keys are required
- user-visible docs/examples if the service is meant to be consumed externally

## Boilerplate avoidance

If a needed abstraction obviously belongs in `eva-common` or `eva-sdk` but is
missing there, stop and ask for an SDK update instead of implementing repo-local
boilerplate in the main platform tree.

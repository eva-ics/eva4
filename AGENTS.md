# EVA4 Agent Guidance

This repository is large, mixed-language, and service-heavy. Read the local
agent docs before editing:

- `docs/agents/README.md`
- `docs/agents/repository-map.md`
- `docs/agents/core-and-services.md`
- `docs/agents/workflows.md`
- `docs/agents/service-authoring.md`
- `AICODE.md`

Primary coding conventions live in Tabularium at `/kb/coding-rules`.

## Scope and intent

- For the current documentation initiative, treat everything under this working
  tree as in scope except `contrib/`.
- The documentation under `docs/agents/` is for coding agents, not public human
  product docs.
- Public-facing docs must stay neutral and enterprise-appropriate. Do not write
  internal liturgy, "spirits", "Omnissiah", or similar wording into public
  documentation.

## Architecture rules

- `eva/` is the node core. Treat it as the highest-context Rust crate in the
  repo.
- `svc/` contains first-party services. Some are modern `eva-sdk` services,
  some intentionally retain older BUS/RT-oriented patterns.
- New Rust services should follow `svc/svc-template` and the `eva-sdk`
  `eapi_bus` lifecycle.
- Existing legacy services must not be migrated to new SDK APIs unless the task
  explicitly requests modernization.
- For legacy services, older SDK helper styles such as `svc_*` patterns are
  acceptable when they are already part of the service. New services should
  avoid that style and use the modern `eapi_bus` lifecycle instead.
- If `eva-common` or `eva-sdk` lacks a type/helper that should exist, stop and
  ask for an SDK update instead of adding local boilerplate inside this repo.

## Verification rules

- Canonical repository-wide verification command:
  `DOCKER_OPTS="-v /opt:/opt -e ARCH_SFX=aarch64" cross check --target aarch64-unknown-linux-gnu`
- Other build/test commands may be useful for local iteration, but the command
  above is the canonical check for this repo and architecture target.
- Unit tests are useful when you touch local logic, but final testing is
  human-run.
- Run `cargo fmt --all` after Rust changes.
- For JS/TS changes, use the local package scripts and format with Prettier when
  relevant.

## Risk boundaries

- Read `AICODE.md` before modifying auth, ACL, crypto, FFI, protocol handling,
  parsers, configuration loaders, or core node behavior.
- `eva/`, `svc/aaa-*`, `svc/controller-*`, `svc/ffi`, `svc/repl*`,
  `crypto-tools`, and any external protocol integration should be treated as
  elevated-risk areas.
- AI-generated tests for security behavior are not acceptable where `AICODE.md`
  forbids them.

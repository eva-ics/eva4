# EVA4 Agent Docs

This directory is the agent-oriented entry point for the EVA ICS v4 source
tree. It is intentionally operational: the goal is to help an agent navigate
the repository, add a service, or make bounded core changes without inventing
architecture.

## Read order

1. `repository-map.md` for top-level structure and ownership clues.
2. `core-and-services.md` for the node core, service families, and risk areas.
3. `workflows.md` for canonical build, validation, and editing workflow.
4. `manage-and-test.md` for common item/service deployment and node-side
   validation tasks.
5. `service-authoring.md` before adding or reshaping a service.

## Repository invariants

- `eva/` is the node core and has different risk than an isolated helper crate.
- `svc/` contains both modern and legacy service implementations.
- New Rust services should use `eva-sdk` plus `eapi_bus`.
- New Rust services should avoid legacy `svc_*` SDK helper style.
- Legacy service internals are not to be modernized unless the task says so.
- Prefer shared abstractions from `eva-common` and `eva-sdk` over repo-local
  reinvention.
- The canonical repository-wide validation command is:
  `DOCKER_OPTS="-v /opt:/opt -e ARCH_SFX=aarch64" cross check --target aarch64-unknown-linux-gnu`

## Primary references

- Platform docs: <https://info.bma.ai/en/actual/eva4/index.html>
- Rust SDK docs: <https://info.bma.ai/en/actual/eva4/sdk/rust/index.html>
- Python SDK docs: <https://info.bma.ai/en/actual/eva4/sdk/python/index.html>
- AI policy: `AICODE.md`
- Coding rules: Tabularium `/kb/coding-rules`

## Scope note

For this documentation set, cover everything in the repository except
`contrib/`. `contrib/` is present but intentionally out of scope unless a task
explicitly pulls it in.

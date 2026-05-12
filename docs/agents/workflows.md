# Workflows

This file captures the workflow future agents should follow when modifying the
repository.

## Default editing workflow

1. Read the relevant subtree and nearby templates before changing anything.
2. Use `rg` to find patterns across services instead of assuming one service is
   representative.
3. Keep changes focused on the target crate/service unless the task explicitly
   asks for broader cleanup.
4. Reuse existing `eva-common`/`eva-sdk` types and helpers where possible.
5. Update templates or docs when the change affects service deployment or agent
   navigation.

## Canonical verification

The repository-wide canonical check is:

```bash
DOCKER_OPTS="-v /opt:/opt -e ARCH_SFX=aarch64" cross check --target aarch64-unknown-linux-gnu
```

Use that command for final machine verification unless the task says otherwise.
Other checks may fail or may not represent the supported path as reliably.

## Useful local iteration patterns

- Rust crate-level iteration:
  `cargo check -p <crate-name>`
- Workspace formatting after Rust edits:
  `cargo fmt --all`
- JS SDK build:
  `cd bindings/js && npm run build`
- Python SDK/examples:
  Inspect the local subtree README/Makefile first; there is no single root
  Python workflow for the whole repo.

These are iteration aids, not replacements for the canonical cross-check above.

## Service-aware workflow

When changing a service, check whether the change also requires:

- a matching `share/svc-tpl/svc-tpl-*.yml` update
- config/defaults updates under `share/registry/defaults/config/`
- SDK-facing documentation or example changes
- packaging/deployment changes under `docker/`, `install/`, or service-specific
  build helpers

## Documentation workflow

- Agent-facing repository docs live under `docs/agents/`.
- Common operational recipes for item/service management and node-side testing
  live in `docs/agents/manage-and-test.md`.
- Public-facing docs must stay neutral in tone and terminology.
- If you add a new important subtree or workflow, extend the relevant document
  here rather than hiding the convention inside a commit message.

## Caution on "cleanup"

Do not use repository-wide cleanup as a default mode.

- Do not mass-modernize legacy service APIs.
- Do not normalize imports, comments, or naming across unrelated services.
- Do not move code across crates because two files "look similar".

This repository has historical layers. Some of them are design, not debt.

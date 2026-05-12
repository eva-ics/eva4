# Manage And Test Typical Things

This file is an agent-facing operations recipe for common EVA ICS tasks that
come up while developing or validating a service change. It is intentionally
practical and environment-neutral: no local VM assumptions, no host-specific
paths beyond the standard EVA runtime layout, and no project-meeting lore.

Use these patterns when you need to manage items, update a service, or verify a
state-processing flow by yourself.

## Before touching a node

- Build or check the relevant crate locally first.
- Prefer crate-scoped iteration during development:
  `cargo check -p <crate-name>`
- Use the canonical repository verification before closing work:
  `DOCKER_OPTS="-v /opt:/opt -e ARCH_SFX=aarch64" cross check --target aarch64-unknown-linux-gnu`
- If you changed Rust code, run:
  `cargo fmt --all`

These checks do not replace node-side validation, but they usually catch the
boring failures before you start operating on a real service.

## General deployment loop

For most item or service work, the safe loop is:

1. Export the current live definition.
2. Edit the exported YAML instead of inventing structure from memory.
3. Deploy the edited YAML back to the node.
4. Verify live state and logs.

This avoids schema drift and matches the node's current contract surface.

## Manage items

Export a baseline item definition:

```bash
sudo eva item export -y -o items.yml '#'
```

For a narrower change, export only the relevant mask or OID instead of `#`.

Edit the YAML under the top-level `items:` key. Keep the exported structure
intact; use the live export as the schema reference.

Deploy the updated item set:

```bash
sudo eva item deploy -f items.yml
```

Verify one item quickly:

```bash
sudo eva item state sensor:tests/example
```

When testing a controller or parser change, create only the smallest item set
needed for the scenario. Do not drag unrelated inventory into the test file.

## Manage services

Export the current live service definition:

```bash
sudo eva svc export -o svc.yml <service_id>
```

Edit the YAML under the top-level `svcs:` key. Keep the service shape from the
export, especially `id`, `command`, `config`, timeouts, launcher settings, and
user/workers settings.

Deploy the edited service definition:

```bash
sudo eva svc deploy -f svc.yml
```

If the service configuration changed in the repository, also update the matching
template under `share/svc-tpl/`.

## Replace a service binary safely

If you need to swap the executable path or replace the binary with a freshly
built one, do not overwrite a live binary in place while the old process still
holds it.

Use this sequence:

1. Build the new binary.
2. Copy it to the target location.
3. Destroy the old service instance.
4. Deploy the service again from YAML.

Typical commands:

```bash
sudo eva svc destroy <service_id>
sudo eva svc deploy -f svc.yml
```

If you are testing a temporary binary, point `command:` in the service YAML to
that binary explicitly. Do not rely on memory or on whatever happened to be in
the old path.

## Observe item and bus state

For quick state verification:

```bash
sudo eva item state <oid>
```

To observe raw bus traffic directly:

```bash
/opt/eva4/sbin/bus /opt/eva4/var/bus.ipc listen -t 'RAW/#'
```

Use a narrower topic when possible, for example:

```bash
/opt/eva4/sbin/bus /opt/eva4/var/bus.ipc listen -t 'RAW/sensor/tests/example'
```

This is useful when a service publishes raw-state frames but the final item
state does not look right yet.

## Parser and raw-state testing

Some services, including `eva-controller-lm` parsers and opener flows, consume
or emit raw-state events rather than simple CLI-set values.

For this class of test:

- publish to the raw-state bus topic:
  `RAW/<oid.as_path()>`
- pack the payload as `RawStateEventOwned` MessagePack
- include at least the source `status` and `value`

Important distinctions:

- `RAW/...` is the raw-state input lane
- `ST/LOC/...` and `ST/REM/...` are regular state topics observed after core
  processing
- if you need to inject an arbitrary structured value, raw-state publish is the
  correct path, not a guessed state topic

There is no single stock EVA CLI command that conveniently emits a custom
`RawStateEventOwned` payload for you. In practice, use a small temporary helper
based on the available SDK or BUS/RT tooling in your environment and keep that
helper out of the repository unless the task explicitly requires a permanent
tool.

## Parser-specific service update checklist

When changing a parser-capable service such as `eva-controller-lm`, verify all
of the following together:

- the service code
- the matching `share/svc-tpl/svc-tpl-*.yml` template
- the item/service deploy recipe used for validation
- the expected source topics and output topics

For `eva-controller-lm` parsers specifically:

- parser source items are normal EVA items
- parsed outputs are published to `RAW/<target.as_path()>`
- parser config belongs in `config.parsers`
- each parser group may omit **`config`** entirely (defaults: **`ignore_events: false`**, no **`interval`**); if **`config`** is present, unknown keys are rejected; **`ignore_events`** and **`interval`** are optional fields with the same defaults when omitted inside **`config`**
- items without `map` may still participate in event or interval flow but emit
  no parsed outputs by design

### JSON path shapes (`config.parsers` → `map[].path`)

Paths must start with **`$.`**. Segments are split on **`.`**. A segment may be
**`name[index]`** to index into an array value under map key `name`, then
continue with further segments (for example **`$.readings[0].v`**).

Prefer chaining segments instead of stacking multiple `[i][j]` brackets inside
one token unless you have verified the lookup rules for that pattern.

### Null / missing `value` on pull

Periodic `item.state` RPC may return **`value: null`**. The implementation maps
that to **`Value::Unit`**; JSON-path extraction then finds nothing — no parsed
outputs until the item carries a structured value again. This is expected, not
an error condition.

## Typical validation pattern

For a bounded service change, the usual validation sequence is:

1. Run local crate checks.
2. Deploy the minimum item set needed for the scenario.
3. Deploy the updated service definition.
4. Trigger the input condition or publish the raw source event.
5. Inspect resulting item state.
6. Optionally listen on the bus for intermediate topics.

If the change is parser-, protocol-, or config-loader-heavy, treat the node-side
check as Level 2 style work per `AICODE.md`: verify unhappy paths, malformed
inputs, and the exact topic/payload contract rather than only the happy path.

## What not to do

- Do not write deployment YAML from memory when `eva item export` or
  `eva svc export` can give you the live schema.
- Do not overwrite a running service binary in place and hope the launcher sorts
  it out.
- Do not assume `ST/LOC/...` is the right injection path for structured raw
  values.
- Do not copy machine-local smoke scripts into repository docs as if they were
  portable doctrine.

# YAML parser migration

Direct YAML consumers use `serde-saphyr` 2.1.1. Only `serialize` and
`deserialize` features are enabled; include resolvers, filesystem includes,
property interpolation and robotics expressions are not enabled.

## Reader policy

Use the same explicit options at every reader:

- `DuplicateKeyPolicy::PassThrough`: let the target Serde visitor handle
  duplicates, rather than silently collapsing duplicate struct fields.
- `MergeKeyPolicy::AsOrdinary`: preserve literal `<<` entries, without expanding
  merges (matching the previous readers, which did not call `apply_merge`).
- `strict_booleans: true`: use YAML 1.2 booleans; `yes`, `no`, `on` and `off`
  remain strings during untyped reads.
- `leading_zero_numbers_as_strings` and `underscored_numbers_as_strings`: keep
  implicit numeric-looking strings as with `serde_yaml`.
- `NonFiniteFloatPolicy::PassThrough`: let the target visitor handle NaN and
  infinities as before.

This is not a guarantee of full `serde_yaml` compatibility. In particular:

- A plain null or empty YAML value targeting `String` now fails instead of
  retaining its spelling. Quote text such as `""`, `"null"` and `"~"` explicitly;
  use `Option<String>` only when the existing data model actually permits null.
- Serialized YAML formatting can change; compare decoded values, not text.
- Default parser budgets and alias replay limits remain enabled. Inputs that
  exceed these limits can be rejected even if the previous parser accepted them.
- Explicit tags are checked by the new parser and can reject previously
  accepted mismatched tag/target combinations.

## Verification for review

A temporary differential harness (not part of the workspace) compared the
107 tracked YAML documents outside `contrib/` and `vendor/` using both parsers:

- `eva_common::Value`: 107 equal results, no rejections or differences.
- `serde_json::Value`: 106 equal results; both readers reject the same remaining
  unrendered template, `docker/test/data/init/init1.yml`. Its template placeholder
  is interpreted as nested YAML mappings with a non-string key.
- For accepted documents, serialization by the new writer was read back by both
  readers with equal decoded values.

This checks the repository corpus, not every downstream configuration. Review
MIME string maps and typed agent configurations with the null/String difference
in mind. The Windows agent reader is migrated and formatted, but a Windows
build/runtime test is not included in the aarch64 Linux checks.

The canonical workspace check and a separate Linux agent check are:

```sh
DOCKER_OPTS="-v /opt:/opt -e ARCH_SFX=aarch64" cross check --target aarch64-unknown-linux-gnu
DOCKER_OPTS="-v /opt:/opt -e ARCH_SFX=aarch64" cross check --target aarch64-unknown-linux-gnu -p eva-controller-system --no-default-features --features agent-linux
```

Do not combine the Linux agent feature with the default service feature when
checking that binary.

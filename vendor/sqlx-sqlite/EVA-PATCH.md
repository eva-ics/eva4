# EVA SQLite driver patch

Source: sqlx-sqlite 0.8.6 from crates.io (SQLx upstream).
Original MIT and Apache-2.0 licenses are retained.

Local changes: mark the vendored crate non-publishable and update
libsqlite3-sys from 0.30.1 to 0.39.1 (bundled SQLite 3.54.1).
This addresses RUSTSEC-2026-0459 / CVE-2026-119375 without changing
the SQLx 0.8.6 API or replacing the existing EVA SQLx facade/macros.
The original upstream manifest is preserved as Cargo.toml.orig.

Remove this patch once the supported upstream SQLx driver uses a fixed
libsqlite3-sys version (>=0.39.0). Do not enable bundled-sqlcipher as a
substitute: the advisory also covers the older SQLite bundled there.

## Verification (2026-11-11)

- Canonical aarch64 cross check: passed.
- `ARCH_SFX=xxx /Users/gigabito/workstuff/bin/clippy`: exit 0; one
  unrelated warning in unchanged controller-opcua/src/comm.rs
  (`items_after_test_module`).
- Functional smoke check with the vendored driver: SQLite reports 3.54.1;
  WAL, DDL, bound text/blob/NULL, commit, rollback, checkpoint, reopen and
  the SQLx Any driver passed on the laptop. This is not a security regression
  test or a substitute for the human verification required by AICODE.md.
- Upstream Rust sources and both license files are unchanged byte-for-byte.
- Workspace cargo audit: zero vulnerabilities; existing pin-utils and
  unsafe-libyaml maintenance warnings remain outside this patch.

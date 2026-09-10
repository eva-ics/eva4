# Gettext for Rust

[Documentation (latest stable)](https://docs.rs/gettext/)

## Roadmap for now
- [x] Parsing MO files (10.3)
- [x] Parsing metadata (6.2)
- [x] Supporting encodings other than UTF-8
- [x] Parsing the plural expression (11.2.6)
- [ ] Correct pathfinding? (11.2.3)
# EVA ICS internal fork

This directory is an internal, non-publishable fork of
[`gettext` 0.4.0](https://github.com/justinas/gettext). It preserves the
original crate API and GNU MO parsing behavior while replacing the
unmaintained `encoding` dependency with `encoding_rs`.

The original MIT license and copyright notice are retained in `LICENSE`.

# Rust discovery harness

`cargo test` of the main crate compiles browser-commander's `chromiumoxide_cdp`,
which needs more memory than some sandboxes allow. This crate builds only the
browser-free discovery modules of `rust/src` (through `#[path]`) so their
tests and a JSON inspection example run anywhere.

```bash
./generate-publisher-tests.py      # plan-free tests of rust/tests/unit/publishers.rs
cargo test                         # skips.rs unchanged + generated publisher tests
node compare.mjs [repository...]   # defaults to this repository and fixtures
```

`compare.mjs` runs the JavaScript `inspectRepository` and the Rust `inspect`
example on each repository and reports `same` or the first difference, which
checks Rust/JavaScript parity on real repositories.

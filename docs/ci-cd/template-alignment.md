# CI/CD Template Alignment

This repository is intentionally polyglot: repository policy stays at the
root, while every publishable package owns its manifest, lockfile, source,
tests, documentation, changelog, and release helpers under its language
directory.

The full tracked trees were compared against these upstream snapshots:

- [`rust-ai-driven-development-pipeline-template` at `e7d4a5b`](https://github.com/link-foundation/rust-ai-driven-development-pipeline-template/tree/e7d4a5b)
- [`js-ai-driven-development-pipeline-template` at `f2cd4d8`](https://github.com/link-foundation/js-ai-driven-development-pipeline-template/tree/f2cd4d8)
- [`command-stream` at `be6773b`](https://github.com/link-foundation/command-stream/tree/be6773b), the current Rust/JavaScript monorepo precedent

## Adopted and adapted practices

| Template area | This repository | Decision |
| --- | --- | --- |
| Package layout | `rust/`, `js/` | Adopted from the multi-language precedent; there is no root package manifest. |
| Reproducible installs | `rust/Cargo.lock`, `js/package-lock.json` | Adopted; CI uses both committed lockfiles and lock-derived cache keys. |
| Package documentation | Per-package README, LICENSE, and CHANGELOG | Adopted; root documentation describes the combined product. |
| Rust quality gates | rustfmt, Clippy, rustdoc warnings, unit/integration/doc tests, script tests, crate-size guard | Adopted from the Rust template and made manifest-path aware. |
| JavaScript quality gates | syntax check, ESLint, Prettier, jscpd, Node tests, audit, and dry-run package inspection | Adopted from the JavaScript template with configuration scoped to `js/`. |
| Runtime matrix | Rust on Linux/macOS/Windows; Node 20 and 24 across Linux/macOS/Windows | Adapted to supported runtimes. Bun and Deno are not claimed because Playwright and the package's Node CLI are the supported execution contract. |
| Fresh-merge validation | `scripts/simulate-fresh-merge.sh` | Adopted once at repository scope and checks both manifests. |
| Supply-chain checks | pinned third-party actions, CodeQL, dependency review, cargo-audit, npm audit, secretlint, actionlint, and zizmor | Adopted at repository scope so one policy covers both packages. |
| Timeouts and concurrency | step budgets, job backstops, matrix-specific cancellation groups, non-cancellable writers | Adopted; write jobs share one serialization group. |
| Release metadata | `rust/changelog.d/` and synchronized package versions | Adapted to one product release train rather than two competing version sources. |
| Registry publication | crates.io and npm publication with availability verification | Adopted; npm removes deprecated auth configuration, verifies npm's OIDC-compatible minimum version, and uses `NPM_TOKEN` only as a first-publication fallback. |
| GitHub Pages and desktop artifacts | Root workflows, Rust output under `rust/target/` | Adapted because these are repository products rather than npm-package files. |

## Deliberately not copied

Template demonstration applications, historical case-study data, generated
screenshots, benchmark corpora, and template-renaming utilities are not product
source and therefore are not copied. Browser, desktop, and mobile example
matrices from the JavaScript template are also not enabled: this package is a
Node CLI, and claiming unsupported runtimes would turn a template checklist
into a false compatibility promise.

The repository keeps one root workflow suite instead of duplicating a complete
workflow directory under each package. This follows `command-stream`: shared
security and policy gates run once, while commands and cache paths explicitly
select `rust/` or `js/`.

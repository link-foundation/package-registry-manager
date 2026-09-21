# Contributing to package-registry-manager

Thank you for helping improve package registry setup automation. Changes should
preserve the Rust/JavaScript behavioral contract and the safety rule that the
tool does not upload package artifacts.

## Development Setup

Install stable Rust, Node.js 20 or newer, and the Rust components used by CI:

```bash
rustup component add rustfmt clippy
cargo install rust-script
cd js && npm ci && cd ..
```

Build both CLIs:

```bash
cargo build
node js/src/cli.mjs --help
```

The browser path requires an installed Chrome-family browser. Automated tests
do not require a registry account or browser. Never use a personal default
profile for development automation; use the CLI's dedicated profile.

## Development Workflow

Start by adding the smallest fixture and test that reproduces the behavior.
Shared manifests belong in `tests/fixtures/polyglot`; language-specific unit
tests belong in `tests/unit/` or `js/test/`.

Run the focused suite while developing, then run the complete local checks:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features
cargo test --all-targets
./scripts/test-scripts.sh

cd js
npm run check
npm test
```

Keep command execution in exact argument-vector form. Do not introduce shell
string interpolation for repository-derived values. Keep tracing behind
`--verbose`, and do not log credentials, cookies, or registry tokens.

Public Rust APIs need documentation comments. Keep source files below 1,000
lines and documentation below 2,500 lines; CI enforces these limits.

## Changelog Management

Every user-facing change needs one Markdown fragment in `changelog.d/`:

```text
changelog.d/YYYYMMDD_HHMMSS_short_description.md
```

Use `### Added`, `### Changed`, `### Fixed`, or `### Removed` and describe the
observable behavior. Do not edit the package version in a pull request; the
release workflow consumes fragments and applies the next version on `main`.

## Pull Request Process

1. Rebase or merge the current `main` branch and resolve conflicts locally.
2. Add a reproducing automated test before changing behavior.
3. Run the Rust and JavaScript checks above.
4. Verify `git status` contains no generated browser profile, dependencies, or
   unrelated files.
5. Explain the reproduction, solution, and tests in the pull request body.
6. For browser UI changes, include before/after evidence without exposing
   account details.
7. Review the final diff for unexpected feature removal and secret material.

Maintainers may ask for a live browser verification because registry sites can
change independently of this repository. Such verification complements rather
than replaces the deterministic prefill and plan tests.

## Release Pipeline

The root Cargo package uses the established fragment-driven release workflow.
The JavaScript package has its own lockfile and CI check; registry publication
must use a reviewed trusted-publishing workflow. Package setup and package
publication remain separate operations.

## License

Contributions are released under the repository's [Unlicense](LICENSE).

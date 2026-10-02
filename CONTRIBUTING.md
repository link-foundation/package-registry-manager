# Contributing to package-registry-manager

Thank you for helping improve package registry setup automation. Changes should
preserve the Rust/JavaScript behavioral contract and the safety rule that the
tool does not upload package artifacts.

## Development Setup

Install stable Rust, Node.js 22 or newer, and the Rust components used by CI:

```bash
rustup component add rustfmt clippy
cargo install rust-script
cd js && npm ci && cd ..
```

Build both CLIs:

```bash
cargo build --manifest-path rust/Cargo.toml
node js/src/cli.mjs --help
```

Form filling requires an installed Chrome-family browser; sign-in URLs open in
the default browser. Automated tests do not require a registry account or
browser. Never use a personal default profile for development automation; use
the CLI's dedicated profile, which lives in the per-user state directory and
must never be placed where Git can commit it.

## Development Workflow

Start by adding the smallest fixture and test that reproduces the behavior.
Shared manifests belong in `tests/fixtures/polyglot`; language-specific unit
tests belong in `rust/tests/unit/` or `js/test/`.

Run the focused suite while developing, then run the complete local checks:

```bash
cargo fmt --all --manifest-path rust/Cargo.toml -- --check
cargo clippy --manifest-path rust/Cargo.toml --all-targets --all-features
cargo test --manifest-path rust/Cargo.toml --all-targets
./rust/scripts/test-scripts.sh

cd js
npm run check
npm test
```

Changes to how the automated browser starts need the browser smoke test. It
launches the automated profile in an installed Chrome and asserts, through
browser-commander's `measureParity`, that `navigator.webdriver` is `false` and
that the command line has no switches a browser started by hand would not
have. It needs a display, so it is skipped unless `PRM_BROWSER_SMOKE=1`:

```bash
PRM_BROWSER_SMOKE=1 xvfb-run -a node --test js/test/browser-parity.test.mjs
PRM_BROWSER_SMOKE=1 xvfb-run -a cargo test --manifest-path rust/Cargo.toml --test integration browser_parity
```

The link-foundation libraries (browser-commander, command-stream, and
lino-arguments) must stay at their latest release in both manifests.
`node scripts/check-upstream-dependencies.mjs` reports any that are behind;
CI runs it on every pull request and daily, and Dependabot opens the bump.
Bump the JavaScript and Rust packages together and adopt new upstream
features in both implementations.

Keep command execution in exact argument-vector form. Do not introduce shell
string interpolation for repository-derived values. Keep tracing behind
`--verbose`, and do not log credentials, cookies, or registry tokens.

Public Rust APIs need documentation comments. Keep source files below 1,000
lines and documentation below 2,500 lines; CI enforces these limits.

## Changelog Management

Every user-facing Rust change needs one Markdown fragment in `rust/changelog.d/`:

```text
rust/changelog.d/YYYYMMDD_HHMMSS_short_description.md
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

The Rust package under `rust/` uses the established fragment-driven release
workflow. That release synchronizes the JavaScript package version under `js/`,
then the shared workflow publishes and verifies both registry artifacts.
JavaScript has its own lockfile and CI check; registry publication must use a
reviewed trusted-publishing workflow. Package setup and package publication
remain separate operations.

## License

Contributions are released under the repository's [Unlicense](LICENSE).

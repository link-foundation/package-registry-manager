# package-registry-manager

Inspect a source repository, identify its publishable packages, and walk a
maintainer through secure registry setup. Equivalent Rust and JavaScript CLIs
cover the maintained language ecosystems in the hive-mind CI/CD guidance.

[![CI/CD Pipeline](https://github.com/link-foundation/package-registry-manager/actions/workflows/release.yml/badge.svg)](https://github.com/link-foundation/package-registry-manager/actions/workflows/release.yml)
[![Security](https://github.com/link-foundation/package-registry-manager/actions/workflows/security.yml/badge.svg)](https://github.com/link-foundation/package-registry-manager/actions/workflows/security.yml)
[![License: Unlicense](https://img.shields.io/badge/license-Unlicense-blue.svg)](http://unlicense.org/)

## Features

- Discovers package manifests recursively while ignoring generated dependency,
  build, virtual-environment, and vendor directories.
- Supports npm, crates.io, PyPI, Go modules, NuGet, Maven Central, and Packagist.
- Emits deterministic text or JSON setup plans for automation and review.
- Uses each ecosystem's official CLI for local validation, with exact argument
  arrays through
  [command-stream](https://github.com/link-foundation/command-stream).
- Opens a real installed browser with a dedicated profile through
  [browser-commander](https://github.com/link-foundation/browser-commander), so
  the maintainer authenticates directly with the registry.
- Prefills npm trusted-publisher fields from the GitHub remote and release
  workflow, then requires explicit confirmation before submitting.
- Never publishes an artifact. Validation, identity setup, and the later
  release itself remain separate auditable operations.
- Ships equivalent Rust and JavaScript implementations with fixture-based unit
  and CLI integration tests.

## Quick Start

Build the Rust CLI from the repository:

```bash
cargo build --release --manifest-path rust/Cargo.toml
./rust/target/release/package-registry-manager inspect --repository /path/to/repo
```

Or run the JavaScript CLI with Node.js 20 or newer:

```bash
cd js
npm ci
node src/cli.mjs inspect --repository /path/to/repo
```

The global options may appear before or after the subcommand in the Rust CLI.
The JavaScript CLI accepts them in either position as well.

Generate all setup plans without executing anything:

```bash
cargo run --manifest-path rust/Cargo.toml -- plan --repository /path/to/repo
cargo run --manifest-path rust/Cargo.toml -- plan --repository /path/to/repo --registry npm --format json

node js/src/cli.mjs plan --repository /path/to/repo
node js/src/cli.mjs plan --repository /path/to/repo --registry npm --format json
```

Review npm setup as a dry run, then opt into validation and browser automation:

```bash
cargo run --manifest-path rust/Cargo.toml -- setup --repository /path/to/repo --registry npm
cargo run --manifest-path rust/Cargo.toml -- setup --repository /path/to/repo --registry npm --execute

node js/src/cli.mjs setup --repository /path/to/repo --registry npm
node js/src/cli.mjs setup --repository /path/to/repo --registry npm --execute
```

If more than one package targets the selected registry, add
`--package <package-name>`. Add `--verbose` when troubleshooting to show the
exact validation commands and their output. Tracing is off by default.

### Safety model

`inspect` and `plan` only read local files. `setup` is also a dry run unless
`--execute` is present. Execution runs validation commands and opens a browser,
but never calls a registry's artifact-publish operation. Browser form
submission is separately confirmed; `--yes` may be used in a supervised run to
confirm the npm submission in advance.

Do not pass a normal browser profile to `--browser-profile`. Chrome 136 and
newer intentionally restrict automation of the default profile. The default is
an isolated, reusable profile under
`.package-registry-manager/browser-profile`, which lets the user sign in
without exposing credentials to the CLI.

Use `--no-browser` with `--execute` on a machine without a graphical browser.
The CLI runs the validation step and prints the official setup URL for opening
elsewhere.

## Supported registries

| Ecosystem | Manifest | Registry | Validation | Authenticated setup |
| --- | --- | --- | --- | --- |
| JavaScript / TypeScript | `package.json` | npm | `npm pkg get ...` | Trusted publisher form, prefilled for GitHub Actions |
| Rust | `Cargo.toml` | crates.io | `cargo publish --dry-run` | Account/token or trusted-publishing settings |
| Python | `pyproject.toml`, `setup.py` | PyPI | `python -m build` | Trusted publisher settings |
| Go | `go.mod` | Go module proxy | `go test ./...` | Version tag and proxy request; no central account |
| C# / .NET | `*.csproj`, `*.fsproj`, `*.vbproj` | NuGet | `dotnet pack --configuration Release` | Trusted-publishing credential |
| Java | `pom.xml`, `build.gradle`, `build.gradle.kts` | Maven Central | `mvn --batch-mode verify` | Central namespace verification |
| PHP | `composer.json` | Packagist | `composer validate --strict` | Repository submission |

The generated plan links to the registry and preserves the detected manifest,
package name, version, repository coordinates, and ordered setup steps. A
manifest marked private (`package.json`) or unpublishable (`Cargo.toml`) is
reported, and execution refuses to proceed for it.

## Registry walkthroughs

### npm trusted publishing

1. Ensure `package.json` has the final package name and is not marked private.
2. Add a GitHub Actions release workflow with `id-token: write` and an npm
   publish step. The workflow filename—not a path—is the identity npm records.
3. Run `plan --registry npm --format json`. Verify `organization`,
   `repository`, and `workflow` in `trusted_publisher`.
4. Run `setup --registry npm --execute`. The CLI checks package metadata, then
   opens the package access page. No npm token is needed by the CLI.
5. Sign in directly in the visible browser and navigate to the trusted
   publisher form. Press Enter in the terminal so the CLI can select GitHub
   Actions and fill the three identity fields.
6. Review every field. Answer `y` only when the visible values are correct.
7. Keep publishing in CI; do not add a long-lived npm token as a fallback.

See npm's official
[trusted publishing documentation](https://docs.npmjs.com/trusted-publishers/)
for supported CI providers and workflow requirements.

### crates.io

The CLI runs `cargo publish --dry-run` in the manifest directory, then opens
the crates.io account settings. Sign in with GitHub and configure the release
identity supported by your workflow. Artifact upload is deliberately left to
the repository's reviewed release job. See the Cargo
[publishing reference](https://doc.rust-lang.org/cargo/reference/publishing.html).

### PyPI

Install the project's build tooling, then let the CLI run `python -m build` and
open the project's publishing settings. Add the GitHub owner, repository,
workflow filename, and optional environment as a trusted publisher. For a
brand-new project, create a pending publisher first. See
[PyPI trusted publishers](https://docs.pypi.org/trusted-publishers/).

### Go modules

Go has no central package-owner account to configure. The CLI runs the module
tests and links to the official publication flow: commit the module, push a
semantic-version tag (including a subdirectory prefix for modules in a
monorepo), then request that version through `proxy.golang.org`. See
[Publishing a module](https://go.dev/ref/mod#publishing-a-module).

### NuGet

The CLI creates a Release package locally with `dotnet pack`, then opens
NuGet.org trusted publishing. Add the GitHub repository and workflow as a
federated credential, scope it to the package, and keep the actual push in CI.
See [NuGet trusted publishing](https://learn.microsoft.com/nuget/nuget-org/trusted-publishing).

### Maven Central

The CLI runs the Maven verification lifecycle and opens Central Portal
namespace management. Sign in, verify the namespace represented by the
package group ID, and configure the deployment credentials in the CI system.
See the [Central Portal publishing guide](https://central.sonatype.org/publish/publish-portal-guide/).

### Packagist

The CLI strictly validates `composer.json` and opens Packagist's submission
page. Sign in, submit the public VCS repository URL, and enable the GitHub hook
when prompted so Packagist observes new tags. See
[Packagist package submission](https://packagist.org/about#how-to-submit-packages).

## Configuration

The CLIs intentionally use the same options and JSON schema:

| Option | Purpose |
| --- | --- |
| `--repository <path>` | Repository root to inspect; defaults to the current directory |
| `--format text\|json` | Human or machine-readable output |
| `--verbose` | Enable command and browser tracing |
| `--registry <name>` | Restrict `plan`, or select exactly one registry for `setup` |
| `--package <name>` | Disambiguate multiple packages for one registry |
| `--execute` | Opt into validation commands and the browser step |
| `--yes` | Pre-confirm npm's final form submission; requires `--execute` |
| `--no-browser` | Print the setup URL after validation; requires `--execute` |
| `--browser-channel <name>` | Choose installed Chrome, Chromium, Edge, or Brave |
| `--browser-profile <path>` | Choose a dedicated automation profile |

Registry aliases such as `cargo`, `python`, `go`, `dotnet`, `maven`, and
`composer` are accepted. Canonical JSON values are `npm`, `crates-io`, `pypi`,
`go-modules`, `nuget`, `maven-central`, and `packagist`.

The repository coordinates come from `.git/config`. npm workflow selection
prefers a sorted `.github/workflows/*.yml` file containing `npm publish` and
falls back to a filename containing `release`. Missing metadata is never
invented: npm execution reports exactly which repository identity is absent.

## Architecture

The independently publishable implementations follow the same polyglot layout
as command-stream and the other maintained multi-language repositories:

| Path | Purpose |
| --- | --- |
| `rust/` | Rust crate, tests, examples, changelog, and release helpers |
| `js/` | npm package, tests, changelog, and JavaScript checks |
| `tests/fixtures/` | Shared cross-language contract fixtures |
| `.github/workflows/` | Repository-level CI, release, security, and policy gates |

Both implementations expose discovery, plan, browser-script, and execution
modules and validate the same polyglot fixture.

The [template-alignment audit](docs/ci-cd/template-alignment.md) records which
Rust and JavaScript CI/CD practices are adopted, adapted, or intentionally not
applicable to this Node CLI and Rust crate.

Rust uses [lino-arguments](https://github.com/link-foundation/lino-arguments)
for CLI configuration. Both implementations use command-stream for argument-
preserving process execution and browser-commander for the visible authenticated
browser. Those focused integrations follow the formal-ai associative stack
without coupling registry metadata to unrelated components.

```text
repository files
    -> manifest discovery
    -> normalized inspection JSON
    -> registry setup plans
    -> exact-argv validation (opt in)
    -> dedicated authenticated browser (opt in)
    -> reviewed npm form submission (explicit confirmation)
```

## Development

Run all local checks:

```bash
cargo fmt --all --manifest-path rust/Cargo.toml -- --check
cargo clippy --manifest-path rust/Cargo.toml --all-targets --all-features
cargo test --manifest-path rust/Cargo.toml --all-targets

cd js
npm ci
npm run check
npm test
```

The fixture in `tests/fixtures/polyglot` is shared by both suites. It is the
minimum reproducible example for all seven registry detectors. Browser tests
exercise the generated prefill program without requiring credentials or a live
registry; a maintainer performs the final authenticated browser verification.

## Contributing

Contributions are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md), add or update
a reproducing fixture and automated test, run both language suites, and include
a changelog fragment for user-facing changes.

## License

[Unlicense](LICENSE) — this software is released into the public domain.

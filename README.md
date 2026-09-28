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
- Supports npm, crates.io, PyPI, Go modules, NuGet, Maven Central, Packagist,
  and container images on Docker Hub and GitHub Container Registry.
- Looks packages up on the public registry and plans exactly what is still
  missing: a first publish (`bootstrap`), attaching trusted publishing to an
  existing package (`attach`), or nothing at all (`complete`).
- Takes a never-published npm package to OIDC trusted publishing with one
  command. The only maintainer input is browser sign-in and 2FA; the CLI never
  reads, creates, or asks for an npm access token.
- Emits deterministic text or JSON setup plans for automation and review.
- Uses each ecosystem's official CLI for local validation, with exact argument
  arrays through
  [command-stream](https://github.com/link-foundation/command-stream).
- Opens sign-in and approval URLs in the maintainer's default browser, where
  they are usually already signed in, so the maintainer authenticates directly
  with the registry. Forms are filled in a real installed browser with a
  dedicated profile through
  [browser-commander](https://github.com/link-foundation/browser-commander).
- Prefills npm trusted-publisher fields from the GitHub remote and release
  workflow, then requires explicit confirmation before submitting.
- Publishes only the very first version of a package that does not exist yet,
  from a clean temporary worktree of the default branch and after explicit
  confirmation. Every later release goes through CI with trusted publishing.
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

Review npm setup as a dry run, then run the whole flow:

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

`inspect` and `plan` read local files and make anonymous, read-only lookups on
the public registries to learn whether each package exists and already uses
trusted publishing. `--offline` skips the lookups; unknown state keeps every
conditional step in the plan. `setup` is a dry run (`--dry-run`) unless
`--execute` is present.

With `--execute`, each step that changes something outside the local machine
asks first: the first publish, deleting a leftover `NPM_TOKEN` secret, setting
GitHub variables and secrets, and submitting a browser form. `--yes` answers
those prompts in advance for a supervised run. Registry sign-in uses each
tool's own web login, so credentials, cookies, and tokens never pass through
the CLI and are never logged. The session the CLI opened is signed out and the
temporary worktree removed when the run ends, even after a failure.

Web-auth URLs printed by the registry CLIs (`npm login --auth-type=web`, the
`npm publish` 2FA link, and `npm trust`) need no automation, because the tool
waits for completion. `--browser default`, the default, opens them in the
user's default browser (`open` on macOS, `xdg-open` on Linux, the URL handler
on Windows), where the maintainer is usually already signed in and only
approves. `--browser automated` opens them in the automation profile instead.
If the web login is not completed and npm falls back to its legacy `Username:`
prompt, the run stops and asks to re-run for a fresh login link.

The automation profile is only needed to fill a form, such as the npm
trusted-publisher fallback. Do not pass a normal browser profile to
`--browser-profile`; Chrome 136 and newer intentionally restrict automation of
the default profile. The default is an isolated, reusable profile in the
per-user state directory, outside any repository:

| OS | Default `--browser-profile` |
|----|-----------------------------|
| macOS | `~/Library/Application Support/package-registry-manager/browser-profile` |
| Linux | `$XDG_STATE_HOME/package-registry-manager/browser-profile`, or `~/.local/state/package-registry-manager/browser-profile` |
| Windows | `%LOCALAPPDATA%\package-registry-manager\browser-profile` |

The profile holds registry session cookies, so it must never be committed.
When `--browser-profile` points inside a Git work tree, the CLI writes a
`.gitignore` containing `*` before the browser starts (into
`.package-registry-manager/` for `.package-registry-manager/browser-profile`,
otherwise into the profile) and refuses to continue if `git check-ignore`
still reports the profile as not ignored. A profile left in
`.package-registry-manager/browser-profile` by an earlier release is protected
the same way, with a warning to delete it.

Use `--no-browser` with `--execute` on a machine without a graphical browser.
The CLI runs the validation step and prints the official setup URL for opening
elsewhere.

## Supported registries

| Ecosystem | Manifest | Registry | Validation | Authenticated setup |
| --- | --- | --- | --- | --- |
| JavaScript / TypeScript | `package.json` | npm | `npm pkg get ...` | Web-login first publish, then `npm trust github` |
| Rust | `Cargo.toml` | crates.io | `cargo publish --dry-run` | Revocable first-publish token, then trusted publishing settings |
| Python | `pyproject.toml`, `setup.py` | PyPI | `python -m build` | Pending publisher, first OIDC publish from CI |
| Go | `go.mod` | Go module proxy | `go test ./...` | Version tag and proxy request; no central account |
| C# / .NET | `*.csproj`, `*.fsproj`, `*.vbproj` | NuGet | `dotnet pack --configuration Release` | Trusted-publishing credential |
| Java | `pom.xml`, `build.gradle`, `build.gradle.kts` | Maven Central | `mvn --batch-mode verify` | Central namespace verification |
| PHP | `composer.json` | Packagist | `composer validate --strict` | Repository submission |
| Container image | `Dockerfile` + Docker Hub release workflow | Docker Hub | Registry lookup | Repository, scoped token, `DOCKERHUB_*` variables and secret |
| Container image | `Dockerfile` + `ghcr.io` workflow | GHCR | `packages: write` check | Link the package to the repository |

The generated plan links to the registry and preserves the detected manifest,
package name, version, repository coordinates, and ordered setup steps. A
manifest marked private (`package.json`) or unpublishable (`Cargo.toml`) is
reported, and execution refuses to proceed for it.

## Registry walkthroughs

### npm trusted publishing

One command takes a package from "never published" to OIDC trusted
publishing. The only maintainer input is browser sign-in and 2FA:

```bash
package-registry-manager setup --repository /path/to/repo --registry npm --execute
```

Before running it, make sure `package.json` has the final name and is not
private, and that a GitHub Actions release workflow has `id-token: write` and
an `npm publish` step. The workflow filename, not a path, is the identity npm
records. Without `--execute` the same command prints the whole flow.

The CLI looks the package up on the registry and chooses a mode:

| Mode | When | What runs |
| --- | --- | --- |
| `bootstrap` | the package does not exist | sign in, first publish, attach trusted publisher |
| `attach` | the package exists without trusted publishing | sign in, attach trusted publisher |
| `complete` | the latest version came from a trusted publisher | nothing |

A bootstrap run:

1. Validates `package.json` and checks the npm session with `npm whoami`.
2. Signs in with `npm login --auth-type=web` only when needed. The CLI opens
   the printed login URL in the default browser, where the maintainer is
   usually already signed in and approves, completing 2FA if asked.
3. Checks out the default branch into a temporary worktree, runs
   `npm pack --ignore-scripts`, and lists every file with the packed and
   unpacked size.
4. After confirmation, publishes that tarball once with
   `npm publish --auth-type=web`. npm asks for 2FA in the browser.
5. Waits until the registry serves the version.
6. Attaches the trusted publisher with
   `npx -y npm@latest trust github <package> --repo <owner/repo> --file <workflow>`.
   If that command fails, the CLI opens the package's access page and prefills
   the trusted publisher form instead.
7. Confirms the trust with `npm trust list`, then deletes a leftover
   `NPM_TOKEN` repository secret after confirmation.
8. Signs npm out and removes the temporary worktree.

Add `--verify-release` to also dispatch the release workflow, watch it with
`gh run watch`, and confirm that the next version was published with
provenance. See npm's official
[trusted publishing documentation](https://docs.npmjs.com/trusted-publishers/)
and [`npm trust`](https://docs.npmjs.com/cli/v11/commands/npm-trust).

### crates.io

crates.io only offers trusted publishing for crates that already exist, so the
first version must be published with an API token ("you'll need to publish
your first release manually", per the
[crates.io announcement](https://blog.rust-lang.org/2025/07/11/crates-io-development-update-2025-07/)).
The CLI keeps that token as short-lived as possible: it runs
`cargo publish --dry-run`, opens the token page, lets `cargo login` read the
token directly from the terminal, publishes once from a temporary worktree of
the default branch, and then opens the token page again so the maintainer can
revoke it. It then opens the crate's trusted publisher settings and runs
`cargo logout`. An existing crate goes straight to the trusted publisher
settings. See the Cargo
[publishing reference](https://doc.rust-lang.org/cargo/reference/publishing.html).

### PyPI

PyPI needs no upload from the maintainer's machine. For a project that does
not exist yet, the CLI opens the account's pending publisher form, dispatches
the release workflow with `gh workflow run`, and waits until CI has published
the first version through OIDC. For an existing project it opens the project's
publishing settings instead. It runs `python -m build` first as a local check.
See [PyPI trusted publishers](https://docs.pypi.org/trusted-publishers/).

### Docker Hub

A `Dockerfile` with a release workflow that logs in to Docker Hub is reported
as a `docker-hub` package. Its name is the image the workflow pushes, or
`<owner>/<repository>` from the GitHub remote when the workflow reads the name
from a variable. The plan opens the
repository creation page when the repository does not exist, then the page for
a Read & Write access token scoped to that repository. It sets the
`DOCKERHUB_IMAGE` and `DOCKERHUB_USERNAME` repository variables and stores
`DOCKERHUB_TOKEN` with `gh secret set`, which reads the value from the
terminal. The CLI never sees the token.

### GitHub Container Registry

A `Dockerfile` with a workflow that pushes to `ghcr.io` is reported as a
`ghcr` package. The plan reports when the workflow lacks the
`packages: write` permission and links to the package page, where the package
is connected to the repository after the first push. GHCR needs no stored
secret because the workflow uses its own `GITHUB_TOKEN`.

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
| `--offline` | Skip the public registry lookups; unknown state keeps every conditional step |
| `--dry-run` | Print the whole setup flow without running it; the default |
| `--execute` | Run the setup flow and open the browser |
| `--verify-release` | Also dispatch and watch the release workflow and confirm provenance |
| `--yes` | Pre-confirm publishing, secret changes, and form submission; requires `--execute` |
| `--no-browser` | Print the setup URL after validation; requires `--execute` |
| `--browser <default\|automated>` | Open sign-in and approval URLs in the default browser (default) or the automation profile |
| `--browser-channel <name>` | Choose installed Chrome, Chromium, Edge, or Brave |
| `--browser-profile <path>` | Choose the dedicated automation profile used to fill forms (default: per-user state directory) |

Registry aliases such as `cargo`, `python`, `go`, `dotnet`, `maven`, and
`composer`, `docker`, and `ghcr-io` are accepted. Canonical JSON values are
`npm`, `crates-io`, `pypi`, `go-modules`, `nuget`, `maven-central`,
`packagist`, `docker-hub`, and `ghcr`.

Registry lookups use `https://registry.npmjs.org`, `https://crates.io/api/v1`,
`https://pypi.org`, and `https://hub.docker.com/v2`. Point them at a mirror or
a test server with `PACKAGE_REGISTRY_MANAGER_NPM_REGISTRY`,
`PACKAGE_REGISTRY_MANAGER_CRATES_IO_API`, `PACKAGE_REGISTRY_MANAGER_PYPI_API`,
and `PACKAGE_REGISTRY_MANAGER_DOCKER_HUB_API`.

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
    -> public registry lookup (exists? trusted publishing?)
    -> bootstrap / attach / complete setup plans
    -> exact-argv commands (opt in)
    -> web login in the default browser (opt in)
    -> form filling in a dedicated browser profile (fallback)
    -> confirmed first publish, trust attachment, and secret changes
    -> sign-out and temporary worktree cleanup
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
minimum reproducible example for every registry detector. The fake `npm`,
`npx`, `git`, and `gh` tools in `tests/fixtures/fake-tools` and a local mock
registry let both suites run the whole npm bootstrap without credentials or
network access, and assert the exact argument vectors. Browser tests exercise
the generated prefill program; a maintainer performs the final authenticated
browser verification.

## Contributing

Contributions are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md), add or update
a reproducing fixture and automated test, run both language suites, and include
a changelog fragment for user-facing changes.

## License

[Unlicense](LICENSE) — this software is released into the public domain.

# package-registry-manager

Inspect a source repository, identify its publishable packages, and walk a
maintainer through secure registry setup. Equivalent Rust and JavaScript CLIs
cover the maintained language ecosystems in the hive-mind CI/CD guidance.

[![CI/CD Pipeline](https://github.com/link-foundation/package-registry-manager/actions/workflows/release.yml/badge.svg)](https://github.com/link-foundation/package-registry-manager/actions/workflows/release.yml)
[![Security](https://github.com/link-foundation/package-registry-manager/actions/workflows/security.yml/badge.svg)](https://github.com/link-foundation/package-registry-manager/actions/workflows/security.yml)
[![License: Unlicense](https://img.shields.io/badge/license-Unlicense-blue.svg)](http://unlicense.org/)

Account scans and setup are available in both ports:

```sh
package-registry-manager inspect --org link-foundation
package-registry-manager plan --user LOGIN --format json
package-registry-manager setup --org link-foundation --all --execute --browser-import auto
```

Scans read matched manifests/workflows through gh-manager. Account setup shares
one browser, groups npm approvals, and uses CI evidence to preserve healthy tokens
or verify replacements before revocation. `--secret-name '{REGISTRY}_TOKEN_{REPO}'`
selects custom credential names.

Account scans print progress to stderr (`[37/169] owner/repository`) and stream
text inspection results as each repository finishes. JSON inspection remains one
report on stdout. Repository, manifest, registry lookup, and release-log failures are
included in each repository's `error` and structured `errors` fields; valid
packages and later repositories are retained. The command exits nonzero after
printing all results, and `setup --org ORG --all` still plans the usable packages.
Release health checks only the latest default-branch push or manual run of each
detected publishing workflow; it downloads logs only when that run failed.
Expired logs retain the failed run's URL and conclusion alongside the error.
Account `plan` and `setup` JSON reports also include a `plans` array.

Without `--repository`, online commands prefer the current repository's
organization, then a sole organization membership, then the signed-in GitHub
user. Multiple memberships fall back to the user; select an organization with
`--org`. If GitHub account discovery is unavailable, the current directory is
inspected. Explicit `--repository` (including `--repository .`), `--ref`, and
local `--offline` commands keep local scope. Both CLIs' GitHub dependency needs
Node.js 22.12 or newer and npm; JavaScript checks the runtime before loading it.

See the [bootstrap, wrapper and credential guide](docs/registry-setup.md) for branch/PR setup, dual npm names, registry policies and integration limits.

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
- Reuses one dedicated browser profile for npm sign-in, publish approvals
  and publisher forms. Other web-auth links follow `--browser`; an explicit
  `--open-with` selects a browser application. Forms are filled through
  [browser-commander](https://github.com/link-foundation/browser-commander),
  which launches it the way a maintainer would start it by hand: no
  automation switches and `navigator.webdriver === false`.
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

Or run the JavaScript CLI with Node.js 22.12 or newer:

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

Set up every publishable package together with `setup --all --execute` in
either CLI. An optional `--registry` limits the batch. The run reuses one
automated browser and existing registry sign-ins, keeps them for the next run
(`--sign-out` ends them at the end), and prints a per-package summary. If a package fails, cleanup still runs and the
summary identifies the remaining packages as `not-run`.

For npm, crates.io and PyPI, inspection follows repository scripts invoked by
workflow jobs, including Node.js, shell and Python scripts. It reports the
calling job. When CI has no publishing job, inspection and planning warn that
releases will not reach the registry, and setup offers a draft pull request
adding the missing jobs to the workflow already used by the other registries.
The complete YAML is shown before confirmation. The proposal starts from the
default branch in a temporary worktree and pushes a new `prm/publish-jobs-*`
branch. Review its triggers, versioning and build dependencies, merge it, then
re-run setup to attach the publishers to that workflow file.

Use `--add-publish-job --workflow release.yml` to choose a proposal target when
the repository has multiple publishing workflows; `--environment` chooses a
shared GitHub environment. Without `--add-publish-job`, an explicit `--workflow`
remains an override for trusted-publisher attachment. Generated jobs use OIDC:
npm 11 with provenance and public access, crates.io's authentication action,
and PyPI's publishing action. They grant `id-token: write` and require no
long-lived npm token.

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

Web-auth URLs printed by npm (`npm login --auth-type=web`, `npm publish`
2FA and `npm trust`) open in the same dedicated automated profile as publisher
forms, so browser sign-in carries over to every approval. Other web-auth links
follow `--browser default|automated`. With `default`, the CLI detects and names
the system browser and uses its URL handler. `--open-with <app>` explicitly
opens links in another application (`open -a <app>` on macOS, `<app> <url>`
elsewhere); it cannot be combined with `--no-browser`.

npm's login and approval links stay valid for about 5 minutes, so every link
is printed with its deadline, such as `Sign in within about 5 minutes (until
14:05).`; approval links add a reminder that npm asks for the 2FA code on the
page. When a link expires (npm falls back to its legacy `Username:` prompt or
reports `Invalid response from web login endpoint`), the CLI asks npm for a
fresh link, up to 3 links in total, and then stops with a message to re-run
when you are ready to approve within 5 minutes.

The npm session the CLI opened stays signed in after the run: the token stays
in npm's user configuration, and the next run reuses it while `npm whoami`
succeeds, so setting up several packages one after another needs one sign-in.
npm ends the session token after two hours. `--sign-out` runs `npm logout` at
the end instead, except after a naming refusal, which keeps the session for a
renamed retry.

The automation profile holds npm approvals and publisher forms. Do not pass a normal browser profile to
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

The automated profile starts fresh, from the installed Chrome channel, with
no extra switches. Every change to that is opt-in:

- `--browser-executable <path>` launches a specific browser binary instead of
  the channel's.
- `--browser-channel <channel>` selects any browser with a launch control
  protocol in Browser Commander's catalogue, including Opera, Opera GX, Yandex, Vivaldi, Arc,
  Whale, 360, QQ, and Sogou. Existing aliases such as `msedge` also work.
  Firefox, Firefox Developer/Nightly, LibreWolf, Waterfox, Zen, Floorp and Tor
  use WebDriver BiDi. Install Firefox (or the chosen fork) and geckodriver;
  JavaScript includes selenium-webdriver, while Rust uses native WebDriver.
  Each Firefox variant uses its own dedicated profile beside the default
  Chromium profile. `--help` lists launchable channels from the dependency.
- `--browser-import <browser>[:<profile>]` copies
  cookies, history, and other data from one of your browser profiles into the
  dedicated profile before it starts, so registry sessions carry over without
  automating your real profile. `default` takes the system default browser
  and `auto` first discovers registry cookies, then usable saved logins.
  Both CLIs accept every id in Browser Commander's public catalogue, including
  Safari, Safari Technology Preview, Opera, Vivaldi, Arc, and Firefox forks.
  The import list, default-browser identification, and display names come from
  that catalogue. Invalid imports list all sources and discovered installed
  executables or profile roots, without reading cookies.
  `--browser-import-scope domains` imports only the cookies of the registry's
  sign-in domains (`crates.io` and `github.com` for crates.io, `npmjs.com`
  for npm, `pypi.org` and `github.com` for PyPI) instead of the whole profile;
  it is the default for `default`, `auto`, and all Firefox-target imports.
  When `auto` finds no cookies, it imports only usable saved passwords/passkeys
  for those domains. Password autofill requires a Chromium target; Safari
  passwords need an export and native passkeys are currently non-exportable.
  If no usable login exists, the CLI names the installed browsers before
  opening the form. Safari access denial opens Full Disk Access settings and
  explains that the terminal app must be restarted after granting access.
- Without `--browser-import`, the profile stays fresh. When a sign-in step
  finds no session and an installed browser holds cookies for the registry's
  sign-in domains, the CLI asks once whether to import only those cookies.
  Safari imports require permission to read its protected profile files on
  macOS. Catalogue entries describe discovery; available data classes depend
  on the source and Browser Commander's migration support. DuckDuckGo is
  currently detection-only. Firefox imports sign-in cookies through BiDi;
  full target migration, snapshots, `--browser-pref`, and Chromium launch
  restrictions are unavailable for Firefox in the current upstream release
  and produce explicit errors. Safari and Safari Technology Preview are
  import sources; selecting them as launch channels reports that the catalogue
  has no control protocol, pending [Safari launcher support](https://github.com/link-foundation/browser-commander/issues/126).
- `--browser-attach snapshot[:<profile>]` fills forms in a temporary copy of
  your own profile of the `--browser-channel` browser (default `Default`),
  which is deleted afterwards. `--browser-attach extension` drives your
  running browser through the Browser Commander extension: the CLI writes the
  unpacked extension next to the dedicated profile and explains how to load
  it from `chrome://extensions`. Neither mode can be combined with
  `--browser-profile` or `--browser-import`, and the extension mode launches
  nothing, so it also rejects `--browser-executable`, `--browser-pref`, and
  `--browser-restriction`.
- `--browser-pref <key=value>` writes a browser preference, such as
  `intl.accept_languages=en-US`, into the profile. Values are parsed as JSON
  when possible (`true`, `4`, `"text"`), dotted keys nest, and later values
  win.
- `--browser-restriction <name>` adds a launch restriction or preset from
  browser-commander's catalogue, such as `no-extensions`; an unknown name
  lists the valid ones.

Use `--no-browser` with `--execute` on a machine without a graphical browser.
The CLI runs the validation step and prints the official setup URL for opening
elsewhere.

## Supported registries

| Ecosystem | Manifest | Registry | Validation | Authenticated setup |
| --- | --- | --- | --- | --- |
| JavaScript / TypeScript | `package.json` | npm | `npm pkg get ...` | Web-login first publish, then `npm trust github` |
| Rust | `Cargo.toml` | crates.io | `cargo publish --dry-run` | First publish and trusted publisher through the crates.io API with a self-revoking 1-hour token |
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
| `complete` | the latest version came from a trusted publisher | nothing, except the [GitHub Pages checks](#github-pages) |

A bootstrap run:

1. Validates `package.json` and checks the npm session with `npm whoami`.
2. Checks out the default branch into a temporary worktree, runs
   `npm pack --ignore-scripts`, lists every file with the packed and
   unpacked size, installs the tarball into a scratch directory, and runs
   each bin through its `node_modules/.bin` link with `--version`. A bin that
   fails or prints nothing stops the run before anything is published, and
   so does a tarball with repository development directories (`.github/`,
   `tests/`, `test/`, `__tests__/`, `coverage/`, `experiments/`, `target/`,
   `.changeset/`, `.husky/`, `.idea/`, `.vscode/`), which a package.json
   without a `files` allowlist lets in.
3. Signs in with `npm login --auth-type=web` only when needed, right before
   publishing, so the approval links do not expire while packing. The CLI
   opens the printed login URL in the dedicated profile reused for subsequent
   approvals; the maintainer completes sign-in and 2FA if asked. It
   then checks that the account has 2FA turned on.
4. After confirmation, publishes that tarball once with
   `npm publish --auth-type=web`. npm asks for 2FA in the browser.
5. Waits until the registry serves the version.
6. Attaches the trusted publisher with
   `npx -y npm@^11.10 trust github <package> --repo <owner/repo> --file <workflow>`.
   If that command fails, the CLI opens the package's access page and prefills
   the trusted publisher form instead.
7. Takes npm's `Trust configuration created successfully` as the
   confirmation; after the browser fallback it confirms the trust with
   `npm trust list --browser=false`, whose 2FA approval link it opens like
   the others. Token secrets are retained as fallback. Explicit
   `--delete-token-secrets` offers deletion only for unused credentials after
   release verification.
8. Signs npm out (only with `--sign-out`) and removes the temporary
   worktree, even when an earlier step failed, then explains that every
   later version is published by the trusted workflow.

The trusted publisher is the workflow file whose job runs `npm publish` (or
`npm`/`pnpm`/`yarn` publishing through a script), not a workflow that merely
mentions npm. When several workflows publish, `--workflow <file>` chooses
one.

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
The CLI does that through the crates.io API, so the first publish takes one
browser sign-in (or none, when the automated profile or an imported session
already has it) and one confirmation (#26):

1. It runs `cargo publish --dry-run`, then opens `https://crates.io/` in the
   automated browser. When `GET /api/v1/me` answers 200 the session is
   reused; otherwise it clicks "Log in with GitHub" and waits until it does.
   crates.io needs a verified email address to publish and to add trusted
   publishers, so a missing one stops the run with the settings link.
2. One confirmation (`--yes` pre-confirms it): create a 1-hour token limited
   to the crate with the `publish-new` and `trusted-publishing` scopes,
   publish, attach the release workflow as trusted publisher, and revoke the
   token.
3. The token is created with `PUT /api/v1/me/tokens` from inside the
   crates.io page, so the session cookie authenticates it and the token never
   reaches the DOM, the terminal, the clipboard, or
   `~/.cargo/credentials.toml`.
4. `cargo publish` runs once from a temporary worktree of the default branch
   with `CARGO_REGISTRY_TOKEN` set only in that child process; there is no
   `cargo login` or `cargo logout`.
5. After the registry shows the version, the token attaches the trusted
   publisher with `POST /api/v1/trusted_publishing/github_configs`. Should
   that fail, the crate's trusted publisher form opens instead.
6. Whatever happened before, the token revokes itself with
   `DELETE /api/v1/tokens/current`, and the run fails while crates.io still
   accepts it.

Leftover `CARGO_TOKEN`/`CARGO_REGISTRY_TOKEN` repository secrets are deleted
after confirmation, and the crates.io sign-in the run brought into the
automated profile is kept for the next run; `--sign-out` removes it. An existing
crate only gets the trusted publisher. `--manual` keeps the previous
checklist: create the token in the form, let `cargo login` read it, revoke it
by hand, and fill the trusted publisher form. See the Cargo
[publishing reference](https://doc.rust-lang.org/cargo/reference/publishing.html).

### PyPI

PyPI needs no upload from the maintainer's machine. For a project that does
not exist yet, the CLI opens the account's pending publisher form, dispatches
the release workflow with `gh workflow run`, and waits until CI has published
the first version through OIDC. For an existing project it opens the project's
publishing settings instead. It runs `python -m build` first as a local check.
The local check reads `project.requires-python`, probes `python`, `python3`
and versioned `python3.<minor>` executables on `PATH`, and chooses a compatible
stable interpreter. For example, `>=3.13` chooses `python3.13` when the default
is 3.12. If none matches, setup stops with an installation hint. Compound
constraints, exclusions, compatible releases and equality wildcards are
supported; unsupported version syntax is treated as incompatible.
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

Configure the explicit NuGet login profile, package owner, exact IDs and push
operation in `.package-registry-manager.json`. Setup discovers the policy in a
dedicated authenticated browser, verifies the active saved state, and ensures
the effective `NUGET_USER` Actions variable through gh-manager. It never registers
template placeholders or broadens a policy to work around an incompatible probe.

Generated CI uses official `NuGet/login` first and retains an existing provided
fallback (`NUGET_TOKEN`, `NUGET_API_KEY` or the workflow's custom name). Only an
unavailable/rejected trusted path can select fallback. Authentication retries
reuse the same validated package; actual push, exact public download and consumer
checks remain required. Setup itself does not upload NuGet artifacts.
See [configuration and verification requirements](docs/issue-73-requirements.md)
and [NuGet trusted publishing](https://learn.microsoft.com/nuget/nuget-org/trusted-publishing).

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

While repositories still use a legacy crates.io secret, explicitly rotate it:

```sh
package-registry-manager setup --org link-foundation --registry crates-io --rotate-token CARGO_TOKEN
package-registry-manager setup --org link-foundation --registry crates-io --rotate-token CARGO_TOKEN --execute --browser-import auto
package-registry-manager inspect --org link-foundation --token-expiry-days 7
```

Rotation matches the private crates.io token inventory by name and creation time
against GitHub's secret `updated_at`. Ambiguous matches stop before creating a
token; set `tokens.crates-io.token_id` in `.package-registry-manager.json` to choose
the known ID. The replacement preserves endpoint and crate scopes, expires after
30 days (`tokens.crates-io.expiry_days`, 1–90), and is stored through gh-manager
with `CARGO_TOKEN_EXPIRES_AT`. Organization storage preserves visibility and
selected grants; repository fallback and existing repository overrides receive
the replacement too. Every discovered consumer, including readers without a
publishable manifest, must pass CI verification before
the old token is revoked. Failed or inconclusive verification retains both
credentials, and incomplete account scans block rotation.

The command lists consumers to migrate to trusted publishing. Ordinary crates.io
setup still uses trusted publishing. Scripted publishing and local reusable
workflow callers are detected during account scans. Expiry inspection opens the
dedicated crates.io browser session and reports expired tokens and those expiring
within the inclusive N-day window; it requires online account scope. The default
scan uses public registry metadata and does not open this private inventory.

Migrate existing crates.io, npm or PyPI publishing with the same reviewed setup:

```sh
package-registry-manager setup --org link-foundation --registry crates-io --migrate
package-registry-manager setup --org link-foundation --registry crates-io --migrate --execute --browser-import auto
```

The first execution proposes a draft PR converting existing publishing jobs and
local reusable callers. Each pipeline tries OIDC first and retains token fallback.
crates.io uses the pinned auth action with `continue-on-error: true`; consumers
read its token output, then `CARGO_REGISTRY_TOKEN` or `CARGO_TOKEN`. npm first
publishes with npm 11 and empty token variables, and retries with token credentials
only if that attempt fails. PyPI uses the publishing action first with OIDC,
then with a token password if needed. Inspection reports
`oidc-with-token-fallback` as the desired policy and flags missing OIDC/fallback.

Conversion preserves release commands, conditions, environments, caller secret
mappings and `secrets: inherit`, and makes reusable fallback inputs optional.
Inline Cargo token arguments, command substitutions, referenced scripts that
build publish commands, token preflights and mixed authentication are supported.
Authentication precedes the first consumer, with a separate auth step for each
condition. Existing variable gates on the old auth action are replaced by an
OIDC attempt on every publishing path. `--migrate` selects every matching crate
without requiring `--package`, converts shared jobs once and registers each crate.
Inline environment mappings and complex Twine commands still require expansion.

Preview uses the inspected working tree. Execution explains that it recomputes
from the latest default branch; changed proposals are displayed before approval.
Review and merge the draft PR, then rerun `--migrate --execute` to register trust
using the top-level caller filename and the publishing callee's environment.
After a real OIDC release at the default-branch head, the command reports retained
fallback tokens and expiry metadata. Existing token rotation remains available.

Deletion requires `--delete-token-secrets` and proof that no workflow consumes
the secret, plus verified OIDC publication. Organization deletion additionally
checks every repository, including archived repositories and forks. Incomplete
scans, dynamic secret references and branch changes retain the credentials.
Normal migration continues for clean repositories and lists skipped failures.

Public crates.io scans use one shared request queue (at most one start per second)
and retry HTTP 403/429 with backoff and `Retry-After`. Failed existence checks stay
unknown with HTTP evidence. Owner-only publisher settings are verified in the
signed-in setup session, independently of public existence and provenance.
Saved crates.io sessions run API tasks headlessly in one shared batch browser;
sign-in or approval escalates to a visible window when needed.

The CLIs intentionally use the same options and JSON schema:

| Option | Purpose |
| --- | --- |
| `--repository <path>` | Inspect only this repository; online commands otherwise prefer organization/user scope |
| `--format text\|json` | Human or machine-readable output |
| `--verbose` | Enable command and browser tracing |
| `--registry <name>` | Restrict `plan`, or select exactly one registry for `setup` |
| `--rotate-token <secret>` | Explicitly replace an existing crates.io token during `setup --registry crates-io` |
| `--migrate` | Convert to OIDC first with token fallback, register trust, and verify publication |
| `--delete-token-secrets` | Opt in to deletion of unused tokens after verified OIDC publication |
| `--token-expiry-days <N>` | Read private crates.io token expiry during an online account scan (0–365 days) |
| `--package <name>` | Plan only this package, or select it for `setup` when a registry has several |
| `--offline` | Skip the public registry lookups; unknown state keeps every conditional step |
| `--dry-run` | Print the whole setup flow without running it; the default |
| `--execute` | Run the setup flow and open the browser |
| `--verify-release` | Also dispatch and watch the release workflow and confirm provenance |
| `--yes` | Pre-confirm publishing, secret changes, and form submission; requires `--execute` |
| `--no-browser` | Print the setup URL after validation; requires `--execute` |
| `--browser <default\|automated>` | Select the browser for web-auth links; npm always reuses the automation profile unless `--open-with` is set |
| `--browser-channel <name>` | Choose an installed launchable browser or alias from Browser Commander's catalogue; `--help` lists launchable channels (default: `chrome`) |
| `--browser-executable <path>` | Launch this browser executable instead of the channel's |
| `--browser-profile <path>` | Choose the dedicated automation profile used to fill forms (default: per-user state directory) |
| `--browser-import <browser>[:<profile>]\|default\|auto` | Import from any browser id in Browser Commander's catalogue; `default` selects the system default browser and `auto` discovers a registry sign-in |
| `--browser-import-scope <full\|domains>` | Import the whole profile, or only registry-domain data; `auto` falls back from cookies to supported saved logins (default: `full` for a named browser, `domains` otherwise) |
| `--browser-attach snapshot[:<profile>]\|extension` | Fill forms in a temporary copy of your profile, or in your running browser through the Browser Commander extension |
| `--browser-pref <key=value>` | Set a preference in the automated profile; may be repeated |
| `--browser-restriction <name>` | Add a launch restriction or preset, such as `no-extensions`; may be repeated |
| `--open-with <app>` | Open sign-in and approval URLs with this browser application instead of the default browser |
| `--manual` | crates.io: create, use, and revoke the first-publish token by hand instead of through the crates.io API |
| `--sign-out` | At the end of setup, run `npm logout` for the npm session the run signed in and remove the crates.io sign-in it brought into the automated profile; without it the next run reuses both sessions |
| `--workflow <file>` | Trusted-publisher workflow in `.github/workflows`, when detection finds several or the wrong one |
| `--environment <name>` | GitHub environment of the trusted publisher |

Registry aliases such as `cargo`, `python`, `go`, `dotnet`, `maven`, and
`composer`, `docker`, and `ghcr-io` are accepted. Canonical JSON values are
`npm`, `crates-io`, `pypi`, `go-modules`, `nuget`, `maven-central`,
`packagist`, `docker-hub`, and `ghcr`.

Registry lookups use `https://registry.npmjs.org`, `https://crates.io/api/v1`,
`https://pypi.org`, `https://hub.docker.com/v2`, NuGet, RubyGems and JSR metadata APIs. Point them at a mirror or
a test server with `PACKAGE_REGISTRY_MANAGER_NPM_REGISTRY`,
`PACKAGE_REGISTRY_MANAGER_CRATES_IO_API`, `PACKAGE_REGISTRY_MANAGER_PYPI_API`,
`PACKAGE_REGISTRY_MANAGER_DOCKER_HUB_API`, `PACKAGE_REGISTRY_MANAGER_NUGET_API`,
`PACKAGE_REGISTRY_MANAGER_RUBYGEMS_API`, and `PACKAGE_REGISTRY_MANAGER_JSR_API`.

The repository coordinates come from `.git/config`. The trusted-publisher
workflow is the one whose job actually publishes the package (see
[npm trusted publishing](#npm-trusted-publishing)); `--workflow` overrides it.
Missing metadata is never invented: npm execution reports exactly which
repository identity is absent.

Manifests inside `tests`, `test`, `fixtures`, `__fixtures__`, `__tests__`, or
`examples` directories are skipped unless a workflow mentions their directory,
so test fixtures are never planned as packages. List more paths to skip as
glob patterns (`*`, `**`, `?`) relative to the repository root in an `ignore`
array of `.package-registry-manager.json`:

```json
{ "ignore": ["experiments/**", "packages/internal-*"] }
```

With `--verbose`, the inspection lists every skipped manifest and its reason
under `skipped`.

### GitHub Pages

When a workflow deploys with `actions/configure-pages`,
`actions/upload-pages-artifact`, or `actions/deploy-pages`, every plan ends
with repository checks, even when the package is already complete:

1. `check-pages` reads the site with `gh api repos/{owner}/{repo}/pages`.
2. `enable-pages` runs only when Pages is not enabled (the API answers 404,
   which makes the deployment fail with "Get Pages site failed ... Not
   Found"). After a confirmation it creates the site with GitHub Actions as its
   source: `gh api -X POST repos/{owner}/{repo}/pages -f build_type=workflow`.
3. `use-pages-workflow` runs only when Pages builds from a branch. After a
   confirmation it switches the source with the same request as `PUT`.

Both changes need repository administrator rights. When `gh` is refused, the
tool prints a warning with the `https://github.com/{owner}/{repo}/settings/pages`
link instead of failing, so the source can be set by hand.

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
preserving process execution, including the `open`, `xdg-open`, and Windows
URL-handler calls behind browser-commander's `openInUserBrowser`, and
browser-commander for the visible authenticated browser: `launchRealBrowser`,
`migrateProfile`, and `attachUserBrowser`. Those focused integrations follow the formal-ai associative stack
without coupling registry metadata to unrelated components.

```text
repository files
    -> manifest discovery
    -> normalized inspection JSON
    -> public registry lookup (exists? trusted publishing?)
    -> bootstrap / attach / complete setup plans
    -> exact-argv commands (opt in)
    -> web login and npm approvals in a shared dedicated profile (opt in)
    -> publisher form polling, filling and submission
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

A smoke test launches the automated profile in an installed Chrome and
compares it with a browser started by hand through browser-commander's
`measureParity`: `navigator.webdriver` must be `false` and the command line
must carry no extra switches. It needs a display, so it only runs on request:

```bash
PRM_BROWSER_SMOKE=1 xvfb-run -a node --test js/test/browser-parity.test.mjs js/test/publisher-dom.test.mjs
PRM_BROWSER_SMOKE=1 xvfb-run -a cargo test --manifest-path rust/Cargo.toml --test integration browser_
```

`node scripts/check-upstream-dependencies.mjs` fails when `js/package.json` or
`rust/Cargo.toml` requires an older browser-commander, command-stream, or
lino-arguments than the latest release. A 0.x caret never reaches the next
minor release, so CI runs this check daily and Dependabot proposes the bump.

## Contributing

Contributions are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md), add or update
a reproducing fixture and automated test, run both language suites, and include
a changelog fragment for user-facing changes.

## License

[Unlicense](LICENSE) — this software is released into the public domain.

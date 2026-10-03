# CI/CD Troubleshooting Guide

This guide covers common CI/CD issues and their solutions for Rust projects using this template.

## Table of Contents

1. [Release Jobs Skipped](#release-jobs-skipped)
2. [Version Already Released (False Positive)](#version-already-released-false-positive)
3. [Missing Committed Cargo.lock](#missing-committed-cargolock)
4. [Transient Cargo Registry Download Failures](#transient-cargo-registry-download-failures)
5. [Crates.io Publishing Fails](#cratesio-publishing-fails)
6. [Crate Package Too Large (HTTP 413)](#crate-package-too-large-http-413)
7. [Docker Hub Publishing Fails](#docker-hub-publishing-fails)
8. [Release Preflight Verdicts](#release-preflight-verdicts)
9. [npm Publication Skipped](#npm-publication-skipped)
10. [Documentation Deployment Skipped (Pages Disabled)](#documentation-deployment-skipped-pages-disabled)
11. [Secret Configuration Issues](#secret-configuration-issues)
12. [Multi-Language Repository Issues](#multi-language-repository-issues)

---

## Release Jobs Skipped

### Symptom
Release jobs (auto-release or manual-release) are skipped even though you expected them to run.

### Common Causes

#### 1. Upstream job was skipped
When a job like `detect-changes` is skipped (e.g., on `workflow_dispatch`), all dependent jobs are also skipped by default.

**Solution:** Ensure dependent jobs use `always() && !cancelled()` in their conditions:
```yaml
if: |
  always() && !cancelled() && (
    github.event_name == 'push' ||
    github.event_name == 'workflow_dispatch' ||
    needs.detect-changes.outputs.rs-changed == 'true'
  )
```

#### 2. Build or test failed
Release jobs depend on `build` which depends on `lint` and `test`. If any of these fail, release jobs won't run.

**Solution:** Check the logs for lint, test, and build jobs. Fix any failures before releasing.

#### 3. Wrong trigger condition
The job condition may not match your trigger event.

**Solution:** Verify the job's `if` condition matches your trigger:
- `github.event_name == 'push'` for automatic releases on merge
- `github.event_name == 'workflow_dispatch'` for manual triggers

### Reference
- [GitHub Actions Runner Issue #491](https://github.com/actions/runner/issues/491)

---

## Version Already Released (False Positive)

### Symptom
The release workflow says "version already released" but the package is not actually on crates.io.

### Root Cause
The workflow was checking git tags instead of crates.io. Git tags can exist without the package being published (e.g., from previous GitHub-only releases).

### Solution
This template now checks crates.io directly using the API:
```javascript
const response = await fetch(
  `https://crates.io/api/v1/crates/${crateName}/${version}`
);
const isPublished = response.ok && (await response.json()).version;
```

### Verification
Check if your package exists on crates.io:
```bash
curl -s "https://crates.io/api/v1/crates/YOUR_CRATE_NAME" | jq
```

### Reference
- [browser-commander Issue #29](https://github.com/link-foundation/browser-commander/issues/29)

---

## Missing Committed Cargo.lock

### Symptom
The `Cargo.lock Guard` job fails with an error like:

```
Binary package ./Cargo.toml requires a committed ./Cargo.lock
```

or:

```
Binary package ./Cargo.toml has ./Cargo.lock, but it is not committed at HEAD
```

### Root Cause
The package has a binary target (`[[bin]]` or `src/main.rs`), but `Cargo.lock`
is missing from the committed repository state. Binary crates should commit the
lockfile so CI, releases, and downstream installs use a deterministic dependency
graph.

Without a committed lockfile, GitHub Actions cache keys such as
`hashFiles('**/Cargo.lock')` degrade to the same empty hash. That can cache an
unpinned dependency graph and let dependency re-resolution drift appear only in
fresh or cache-less jobs.

### Solution
Generate and commit the lockfile:

```bash
cargo generate-lockfile --manifest-path rust/Cargo.toml
git add rust/Cargo.lock
git commit -m "chore: commit Cargo.lock"
```

For a multi-language repository where Rust lives under `rust/`, commit
`rust/Cargo.lock` instead.

Library-only crates can leave `Cargo.lock` uncommitted. If the package contains
`src/main.rs` only for local development, set `autobins = false` in `[package]`
or remove the binary target.

### Verification
Run the guard locally:

```bash
rust-script rust/scripts/check-cargo-lock.rs
```

---

## Transient Cargo Registry Download Failures

### Symptom
Cargo fails while updating the crates.io registry or downloading a crate with a
transient network error, for example:

```
curl failed
[16] Error in the HTTP2 framing layer
```

### Root Cause
GitHub-hosted runners occasionally hit transient registry, CDN, or HTTP/2
transport failures. These errors can affect any workflow step that invokes
Cargo, including `cargo install`, `cargo build`, `cargo test`, `cargo publish`,
and `cargo metadata`.

### How This Template Prevents It
The workflow sets Cargo network options at top-level `env` so all Cargo commands
inherit them:

```yaml
CARGO_NET_RETRY: '10'
CARGO_HTTP_MULTIPLEXING: 'false'
```

`CARGO_NET_RETRY` increases retry attempts for network operations.
`CARGO_HTTP_MULTIPLEXING=false` disables libcurl HTTP multiplexing, which avoids
the HTTP/2 framing failure mode while keeping the rest of Cargo's registry
behavior unchanged.

### Solution
If a downstream repository has copied an older workflow, add the same top-level
environment variables near the existing Cargo settings:

```yaml
env:
  CARGO_TERM_COLOR: always
  RUSTFLAGS: -Dwarnings
  CARGO_NET_RETRY: '10'
  CARGO_HTTP_MULTIPLEXING: 'false'
```

---

## Crates.io Publishing Fails

### Symptom
The "Publish to Crates.io" step fails, or it is skipped with a
"crates.io publication skipped" warning.

### How publishing authenticates
The release workflow stores no crates.io token. `auto-release` and
`manual-release` hold `id-token: write`. The hash-pinned
`rust-lang/crates-io-auth-action` exchanges the workflow's OIDC identity for a
short-lived token. Only the "Publish to Crates.io" step receives that token:

```yaml
- name: Authenticate to crates.io with trusted publishing
  id: crates-io-auth
  uses: rust-lang/crates-io-auth-action@<full commit SHA> # v1.0.5

- name: Publish to Crates.io
  env:
    CARGO_REGISTRY_TOKEN: ${{ steps.crates-io-auth.outputs.token }}
  run: rust-script rust/scripts/publish-crate.rs --rust-root rust
```

The action's post step revokes the token when the job ends. `publish-crate.rs`
reads the token from Cargo's own `CARGO_REGISTRY_TOKEN` variable and never puts
it on the command line.

### Common Errors

#### "crates.io publication skipped: the crate is not on crates.io yet"
**Cause:** Trusted publishing cannot create a crate. Someone has to publish the
first version with a personal token. The release preflight reports
`crates=bootstrap`. The GitHub release, npm and Docker publications still run;
only crates.io is skipped.

**Solution:** Bootstrap the crate once from a maintainer machine, then configure
the trusted publisher:
```bash
package-registry-manager setup --registry crates-io --execute
```
On crates.io, open the crate's **Settings → Trusted Publishing** and add a
GitHub publisher with repository owner `link-foundation`, repository
`package-registry-manager` and workflow `release.yml`. Afterwards, delete the
personal token you used for the first publication.

The release check keeps `should_release=true` until the crate exists. The next
push to `main` therefore publishes the pending version, without bumping it
again.

#### "403 Forbidden" or "unauthorized"
**Cause:** The crate exists, but crates.io refused the trusted-publishing token.
Usually no trusted publisher is configured, or the one that is configured names
a different repository or workflow file.

**Solution:** Check the trusted publisher on the crate's settings page. It must
match `link-foundation/package-registry-manager` and `release.yml` exactly. Also
check that the job still declares `id-token: write`.

#### "please provide a non-empty token"
**Cause:** The publish step ran without a token. Usually the
`crates-io-auth` step was skipped or removed.

**Solution:** Keep the three steps in order: the readiness check
(`id: crates-io`), then the authentication step (`id: crates-io-auth`), then the
publish step. The authentication and publish steps share the condition
`steps.crates-io.outputs.publish == 'true'`.

#### "already uploaded" or "already exists"
**Cause:** This version was already published to crates.io.

**Note:** This is handled gracefully by the script and is not a failure.

### Reference
- [package-registry-manager Issue #16](https://github.com/link-foundation/package-registry-manager/issues/16)
- [crates.io trusted publishing](https://crates.io/docs/trusted-publishing)
- [rust-lang/crates-io-auth-action](https://github.com/rust-lang/crates-io-auth-action)
- [browser-commander Issue #33](https://github.com/link-foundation/browser-commander/issues/33)
- [Cargo Publishing Documentation](https://doc.rust-lang.org/cargo/reference/publishing.html)

---

## Crate Package Too Large (HTTP 413)

### Symptom
`cargo publish` is rejected by crates.io with:

```
error: failed to publish to registry
the remote server responded with an error (status 413 Payload Too Large):
max upload size is 10485760
```

### Root Cause
The generated `.crate` archive exceeds the crates.io upload limit of **10 MiB
(10485760 bytes)**. This usually happens when documentation, case studies,
generated CI artifacts, datasets, or experiment files are silently bundled into
the package.

### How This Template Prevents It

#### 1. Pre-publish size guard
`rust/scripts/check-crate-size.rs` builds the `.crate` archive and fails the workflow
**before** publishing when the archive is over the limit. It runs in the `build`
job (early PR feedback) and again right before the publish step in both the
`auto-release` and `manual-release` jobs.

Run it locally before pushing:
```bash
rust-script rust/scripts/check-crate-size.rs
```

#### 2. Narrow `include` allowlist
`Cargo.toml` declares an `include` list so only the crate sources and a few
documentation files ship in the release archive:
```toml
include = [
    "src/**/*.rs",
    "examples/**/*.rs",
    "README.md",
    "LICENSE",
    "CHANGELOG.md",
]
```

### Solution When the Guard Fails
1. Inspect what is being packaged:
   ```bash
   cargo package --manifest-path rust/Cargo.toml --list --allow-dirty
   ```
2. Tighten the `include` allowlist in `rust/Cargo.toml` (or add an `exclude` list) to
   drop large files such as docs, datasets, generated logs, and experiments.
3. Re-run the size guard to confirm the archive is under 10 MiB.

### Reference
- [Cargo `include`/`exclude` fields](https://doc.rust-lang.org/cargo/reference/manifest.html#the-exclude-and-include-fields)
- [Cargo packaging documentation](https://doc.rust-lang.org/cargo/reference/publishing.html#packaging-a-crate)

---

## Docker Hub Publishing Fails

### Symptom
The crates.io publish succeeds, but the release workflow fails before or during Docker Hub publishing.

### Required Configuration

Docker Hub publishing is optional. It runs only when all of these are true:

- A root `Dockerfile` exists
- Repository variable `DOCKERHUB_IMAGE` is set to `namespace/repository`
- `DOCKERHUB_USERNAME` is set as a repository variable or secret
- Repository secret `DOCKERHUB_TOKEN` is set
- The release preflight proved that the token can push the image (`docker=ok`)

### Common Errors

#### "Docker Hub publishing requires DOCKERHUB_USERNAME and DOCKERHUB_TOKEN"
**Cause:** `DOCKERHUB_IMAGE` and `Dockerfile` enabled Docker publishing, but credentials are incomplete.

**Solution:** Set `DOCKERHUB_USERNAME` and create a Docker Hub access token stored as `DOCKERHUB_TOKEN`.

#### Docker tag is missing after crates.io already published
**Cause:** A previous release run published the crate, then failed before Docker Hub or GitHub Release completed.

**Solution:** Re-run the release workflow after fixing the Docker Hub configuration. The release check treats the version as incomplete and recreates missing artifacts without bumping the Cargo version again.

### Verification
Check whether a Docker Hub tag exists:

```bash
curl -fsSL "https://hub.docker.com/v2/repositories/NAMESPACE/REPOSITORY/tags/VERSION"
```

### Reference
- [Docker GitHub Actions guide](https://docs.docker.com/build/ci/github-actions/)

---

## Release Preflight Verdicts

### Symptom
A release run publishes to some registries and skips others. The
**Release Preflight** job summary shows a table like this:

| registry | verdict | publishes this run |
| --- | --- | --- |
| crates.io | bootstrap | no |
| npm | ok | yes |
| Docker Hub | skipped | no |

### How it works
`release-preflight` runs `rust/scripts/preflight-credentials.sh` and exposes
one verdict per registry as a job output: `crates`, `npm` and `docker`. Each
publishing job gates on its own verdict, so one registry's problem never blocks
the others:

| Output | Gates | Probe |
| --- | --- | --- |
| `crates` | "Authenticate to crates.io" and "Publish to Crates.io" in `auto-release` / `manual-release` | `GET https://crates.io/api/v1/crates/<crate>` |
| `npm` | `javascript-release` | `GET https://registry.npmjs.org/<package>` |
| `docker` | "Configure Docker Hub publishing", `docker-publish`, `docker-merge-manifest` | Docker Hub token exchange with `push` scope |

| Verdict | Meaning | Publishes |
| --- | --- | --- |
| `ok` | The registry is ready for this release | yes |
| `bootstrap` | The crate or package does not exist yet. Trusted publishing cannot create it | no; the run shows the bootstrap command as a warning |
| `refused` | The registry rejected the credential (Docker Hub only) | no; `::error::` in release mode |
| `unknown` | The registry could not be reached or answered unexpectedly | no; re-run when the registry recovers |
| `skipped` | The registry is not configured (for example, no `DOCKERHUB_IMAGE`) | no |

The script always exits 0. The GitHub release is created as long as crates.io
was either published or deliberately skipped. Only `ok` publishes; every other
verdict skips that registry.

Trusted publishing mints its crates.io and npm tokens inside the publishing
jobs. The preflight therefore checks only that the crate and package exist; it
cannot test the OIDC exchange in advance. A missing or mismatched trusted
publisher still fails at the publish step. See
[Crates.io Publishing Fails](#cratesio-publishing-fails).

### Reproducing locally
```bash
PREFLIGHT_MODE=report bash rust/scripts/preflight-credentials.sh
```

---

## npm Publication Skipped

### Symptom
`javascript-release` is skipped, and the preflight warns that the package "is
not on npm yet".

### Cause
npm publishes with trusted publishing (OIDC) only; the workflow has no
`NPM_TOKEN` fallback. Trusted publishing cannot create a package, so the job
runs only when the preflight reports `npm=ok`.

### Solution
Publish the first version once from a maintainer machine:
```bash
package-registry-manager setup --registry npm --execute
```
On npmjs.com, open the package's **Settings → Trusted Publisher** and add a
GitHub Actions publisher for `link-foundation/package-registry-manager` with
workflow `release.yml`. Later releases then publish with the workflow's OIDC
identity.

---

## Documentation Deployment Skipped (Pages Disabled)

### Symptom
`deploy-docs` is green, but every step after "Check whether GitHub Pages is
enabled" is skipped, and the run shows this warning:

```
Documentation deployment skipped: GitHub Pages is not enabled for OWNER/REPO. Enable it once with GitHub Actions as the source: gh api -X POST repos/OWNER/REPO/pages -f build_type=workflow ...
```

Before this check existed, the job failed on every push to `main` with
`Get Pages site failed`.

### Cause
The repository has never enabled GitHub Pages, or Pages builds from a branch
instead of from GitHub Actions. `actions/configure-pages` can enable Pages
itself (`enablement: true`), but only with a token that has administration
rights. The workflow's `GITHUB_TOKEN` does not have them, and the job
deliberately keeps its permissions minimal: `contents: read`, `pages: write`
and `id-token: write`.

### Solution
A repository administrator enables Pages with GitHub Actions as the source,
once:
```bash
gh api -X POST repos/OWNER/REPO/pages -f build_type=workflow
```
If Pages is already enabled but builds from a branch, switch it to GitHub
Actions instead:
```bash
gh api -X PUT repos/OWNER/REPO/pages -f build_type=workflow
```
The next push to `main` deploys the documentation. Any other answer from the
Pages API fails the check step loudly rather than being silently skipped.

---

## Secret Configuration Issues

### Required Secrets

| Secret Name | Purpose | Where to Get |
|------------|---------|--------------|
| `DOCKERHUB_TOKEN` | Publish to Docker Hub when `DOCKERHUB_IMAGE` is configured | https://docs.docker.com/security/access-tokens/ |
| `GITHUB_TOKEN` | Create GitHub releases | Automatic (provided by GitHub) |

crates.io and npm need no secret, because both publish with trusted publishing
(OIDC). Delete any leftover `CARGO_TOKEN`, `CARGO_REGISTRY_TOKEN` or `NPM_TOKEN`
repository or organization secrets:
```bash
gh secret delete CARGO_TOKEN --repo link-foundation/package-registry-manager
gh secret delete CARGO_REGISTRY_TOKEN --repo link-foundation/package-registry-manager
gh secret delete NPM_TOKEN --repo link-foundation/package-registry-manager
```
A long-lived publish token in the repository is a standing credential that any
compromised workflow step could exfiltrate. The release workflow never reads
one, and `rust/tests/unit/ci-cd/trusted_publishing.rs` fails if one is
reintroduced.

### Mapping a secret that is still needed

Map the secret on the step that uses it, not in the workflow-level `env:`. The
workflow-level `env:` is inherited by every job, including the `pull_request`
jobs that compile and run code from the branch under review:
```yaml
- name: Probe every release registry
  env:
    DOCKERHUB_TOKEN: ${{ secrets.DOCKERHUB_TOKEN }}
  run: bash rust/scripts/preflight-credentials.sh
```

### Reference
- [GitHub Actions Secrets Documentation](https://docs.github.com/actions/security-guides/using-secrets-in-github-actions)

---

## Multi-Language Repository Issues

### Symptom
Scripts fail to find `Cargo.toml` or run in the wrong directory.

### Solution
This template auto-detects the repository structure:
- **Single-language:** `Cargo.toml` in repository root
- **Multi-language:** `Cargo.toml` in `rust/` subfolder

If auto-detection fails, you can explicitly configure the Rust root:
```bash
# Via environment variable
RUST_ROOT=rust rust-script rust/scripts/publish-crate.rs

# Via CLI argument
rust-script rust/scripts/publish-crate.rs --rust-root rust
```

### Workflow Configuration
For this multi-language repository, keep workflow commands at the root and use
explicit paths so shared and package-local files are unambiguous:
```yaml
steps:
  - name: Publish to Crates.io
    run: rust-script rust/scripts/publish-crate.rs --rust-root rust
```

### Reference
- [browser-commander Issue #31](https://github.com/link-foundation/browser-commander/issues/31)

---

## General Debugging Tips

### 1. Check Job Dependencies
View the workflow graph in GitHub Actions to see which jobs depend on which.

### 2. Download Full Logs
```bash
gh run view <run-id> --repo owner/repo --log > ci-logs.txt
```

### 3. Enable Debug Logging
Add this secret to enable debug logging:
- Name: `ACTIONS_STEP_DEBUG`
- Value: `true`

### 4. Check crates.io Status
Sometimes crates.io has issues. Check: https://status.crates.io/

### 5. Verify Package Locally
Before pushing, verify your package builds and passes checks:
```bash
cargo fmt --manifest-path rust/Cargo.toml --all -- --check
cargo clippy --manifest-path rust/Cargo.toml --all-targets --all-features
cargo test --manifest-path rust/Cargo.toml --all-features
cargo package --manifest-path rust/Cargo.toml --list
rust-script rust/scripts/check-crate-size.rs
```

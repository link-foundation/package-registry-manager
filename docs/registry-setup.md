# Bootstrap refs, wrapper packages and publishing credentials

Both CLIs use the same options and registry policy. `setup` prints a reviewable
plan; `--execute` runs it.

## Repair publishing after a GitHub repository transfer

For npm, crates.io and PyPI, inspection compares the canonical GitHub repository
with manifest URLs, the latest release's provenance and available publisher
settings. An older successful trusted publication does not prove that today's
publisher configuration names the correct repository. Warnings identify each
stale source, and setup plans a repair when settings are stale or unverified.

Run `setup --registry npm --package PACKAGE --execute` from the transferred
repository. Setup reads authenticated registry settings, attaches the current
repository's publisher when needed, and verifies its repository, workflow and
environment before offering to remove obsolete publishers. If attachment or
verification fails, the old configuration remains in place.

For stale manifest metadata, setup offers a separate branch and draft pull
request, retaining fields such as npm's `repository.directory`. Merge that PR
and run setup again. Release retries wait for corrected remote metadata: a failed
run whose original SHA has correct metadata can be rerun; otherwise setup starts
a fresh workflow from the corrected default branch. Tag-only workflows may need
a new release tag. See the [transfer regression case study](case-studies/issue-41/README.md).

## Publish a new npm name before merging

Push a branch containing the new manifest and its publishing workflow, then run:

```sh
package-registry-manager setup --repository /path/to/repository \
  --registry npm --package gh-upload --ref rename-branch --execute
# A PR number or GitHub PR URL also selects its head:
package-registry-manager setup --repository /path/to/repository \
  --registry npm --package gh-upload --ref 37 --execute
```

Inspection and packing use disposable worktrees. The existing checkout stays
unchanged. The plan reports the manifest name/version on remote main and verifies
the packed name/version against the inspected ref before uploading. Push changes
before setup; local uncommitted changes are not included.

A bootstrap version is already published when the PR merges. Bump the merge's
version to the next unpublished version, or have the release workflow skip exact
versions that already exist. A name/version pair cannot be reused, even after
unpublish. `--ref` does not edit release versions or release workflows for you.

The first-publish precheck validates syntax, checks the exact name and a bounded
set of punctuation variants (including the punctuation-free moniker), then dry
runs the reviewed tarball before sign-in or publishing approval. npm has private
server-side policies; a dry run does not reserve a name or guarantee acceptance.
A policy E403 stops setup and suggests a scoped or more descriptive name.

## Keep both names receiving releases with a wrapper

Use a second real manifest rather than renaming away the old package. For example:

```json
{
  "name": "gh-upload-log",
  "version": "1.1.0",
  "type": "module",
  "exports": "./index.mjs",
  "bin": { "gh-upload-log": "./cli.mjs" },
  "dependencies": { "gh-upload": "1.1.0" },
  "publishConfig": { "access": "public" }
}
```

Place it at `packages/gh-upload-log/package.json`. Its `index.mjs` can re-export
`gh-upload`. Its executable `cli.mjs` must invoke a public CLI entry point exported
by the main package (and retain its shebang and executable bit). Keep the main
package's public exports and command arguments stable.

Release both manifests at each version. Explicit steps make coverage reviewable:

```yaml
permissions:
  id-token: write
steps:
  - run: npm publish --provenance --access public
  - run: npm publish --provenance --access public
    working-directory: packages/gh-upload-log
```

Use an existing-version guard for each step. Inspect lists each name separately
and probes its own npm existence and trust state. It warns about manifests with
no publisher and wrappers whose versions diverge from the dependency they wrap.
Set up the trusted publisher for **each** package against that workflow. For
complex scripts that dynamically choose manifest paths, use explicit publishing
steps or review and select the workflow manually.

Cross-links are opt-in. Add a README notice that both names remain maintained, or
explicitly run `npm deprecate gh-upload-log "Also available as gh-upload"` if a
maintainer wants a deprecation notice. Setup never deprecates the old name.

## Registry policy

| Registry | Credential policy |
| --- | --- |
| npm, PyPI | Trusted publishing, remove unused long-lived secrets after verification |
| crates.io | Trusted publishing; existing short-lived in-memory first-publish token flow |
| RubyGems | Register pending/existing trusted publisher for the GitHub workflow |
| NuGet | Trusted policy plus NuGet/login's temporary OIDC API key, never a stored fallback |
| JSR | Repository-linked OIDC publishing |
| GHCR | GITHUB_TOKEN with packages: write |
| Go modules, Packagist | Public repository tags |
| Docker Hub, Central, VS Code Marketplace, Open VSX, Chrome Web Store | Provider credential lifecycle through gh-manager; see current limits below |

After OIDC release verification, setup audits repository secrets and offers to
delete unused known token names. It retains secrets referenced by any workflow;
remove those references after migration. Organization credentials can be shared
by other repositories and need an organization-wide audit before deletion.

Cleanup requires an explicit successful-release confirmation even with `--yes`.
For npm, `--verify-release` checks registry provenance before cleanup and supplies
that confirmation automatically. Other registries require maintainer verification.

## Scan and set up an organization or user

```sh
package-registry-manager inspect --org link-foundation
package-registry-manager inspect --user LOGIN --format json
package-registry-manager plan --org link-foundation --registry npm
package-registry-manager setup --org link-foundation --all --execute \
  --browser automated --browser-import auto
```

Scans use gh-manager repository/file APIs to fetch manifests, publishing workflows
and configuration, without cloning every repository. The table reports unpublished
packages, published packages without trusted publishing, publishers naming another
repository, and recent default-branch release failures matching registry errors.
JSON includes the run URL and matching log evidence. Unknown registry answers stay
unknown; they never authorize publication or token rotation.

Setup selects findings, fetches only the affected repositories into disposable
worktrees, and shares one automated browser and domain-scoped sign-in import.
An explicit `--open-with` uses the selected external application instead. npm packages
run together before other registries, with sign-out deferred until the batch ends.
Workflow proposals use each repository's own checkout. A repository failure is
reported in the summary and other selected repositories can continue. Scans
exclude archived repositories and forks according to gh-manager's defaults.

## Configure CI-driven publishing credentials

Token setup uses gh-manager's library in JavaScript and its pinned CLI in Rust.
Sign in with `gh auth login` first. Configuration is optional:

```json
{
  "tokens": {
    "docker-hub": {
      "secret": "DOCKERHUB_TOKEN",
      "expiry_days": 30,
      "level": "org"
    }
  }
}
```

The default scope is organization-level, with selected repository grants and a
repository-secret fallback when GitHub refuses organization storage. The summary
shows the actual scope and fallback reason. Existing repository secrets override
organization secrets, so a broken repository override is replaced too.

Use `--secret-name '{REGISTRY}_TOKEN'` for a shared name, or
`--secret-name '{REGISTRY}_TOKEN_{REPO}'` for a repository-specific name.
`{REGISTRY}`, `{REPO}`, `{ORG}` and `{OWNER}` expand to uppercase letters, digits
and underscores. The CLI override takes precedence over configuration and detected
workflow secret names. Update workflows to reference the chosen name before setup.

The lifecycle is driven by CI evidence, with this tool's registry error patterns:

- **ok:** keep the existing credential, regardless of recorded expiry.
- **auth-failing:** create a token in the browser, ensure storage, test CI, then
  revoke the replaced token only after the relevant CI tests succeed.
- **unknown, secret present:** test first; leave the credential active if CI
  remains unknown or fails for an unrelated reason.
- **unknown, secret absent:** create, store and test. An authentication failure
  permits one new candidate and one more test; further failures stop the cycle.

Deploy a dispatchable workflow that uses the secret to the default branch.
gh-manager discovers the workflows and correlates newly dispatched runs before
classifying the steps that use the secret. It does not treat an unrelated passing
job as verification. The [Docker login example](../examples/registry-credentials/verify-docker-login.yml)
can be used as a non-publishing check. Its legacy nonce input is supported through
optional `verification_workflow` configuration.

Browser tokens remain in memory: JavaScript passes them through an acquisition
callback; Rust sends them through stdin. Protocol tracing is suppressed for token
setup. Token IDs, never values, are retained in the per-user state directory to
support later revocation; set `token_id` in registry configuration for an older
credential created outside this tool. GitHub cannot return a previous token value
or its registry ID. Untracked old registry tokens require manual revocation.

Use a dedicated profile and domain-scoped `--browser-import auto|default|BROWSER`;
Safari support comes from browser-commander. Review the provider's scope and expiry
before creating a token. `expiry_days`, when supplied, must be an integer from 1 to
90; it controls new tokens rather than deciding when existing tokens rotate.
Failed storage or uncertain verification leaves existing credentials active.

## Dependency and registry lookup limits

The scoped npm release is still unavailable. Both ports pin gh-manager to source
commit `29808e088ac8fd8374361920bbebab07a4d63ac6`, which implements repository/run
discovery and generic secret health/ensure/test APIs from gh-manager#13. JavaScript
installs it as `@link-foundation/gh-manager`; Rust resolves the same immutable source
archive through `npx`, so Node.js 22.12+ and npm are required for account scans and
token setup. An ambient `gh-manager` executable is no longer required.

Public existence probes cover npm, crates.io, PyPI, Docker Hub, NuGet, RubyGems
and JSR. NuGet, RubyGems and JSR public metadata does not establish configured
trusted-publisher identity; those fields remain unknown until registry settings
are verified. Other registries currently retain unknown public state and can
still be selected by registry-matched CI failures or a local setup command.

Provider pages are not uniform APIs. Browser form helpers require human review and
creation/revocation in the dedicated browser; the generic one-time-value and
identified-list selectors have deterministic tests, not live account validation.
Unrecognized values, identifiers, supplied expiry or token lists stop the flow. Fully
unattended provider creation and API-backed revocation remain unverified.

Docker personal Read & Write tokens are not limited to one repository. Prefer
organization access tokens with image-push permission on one repository where the
account supports them. Central returns a token username/password pair; it must be
stored as a pair and wired into Maven settings, not treated as an ordinary single
PAT. Chrome uses an OAuth client plus refresh token, which does not supply a normal
PAT expiry/identifier. The generic token path refuses these
incompatible responses rather than claiming a successful lifecycle. Provider-specific
pair/OAuth adapters are required for those cases.

## Safe publish-job proposals

`--add-publish-job` remains a reviewable workflow proposal. npm and cargo jobs read
name/version from the checkout and check the exact registry version with a unique
query and no-cache headers. Only HTTP 404 allows publishing; existing versions
skip, while network errors and forbidden responses fail before publication.
PyPI uses `skip-existing: true`.

When exactly one existing job exposes `published_version`, proposals depend on it,
require a nonempty output, check out the default branch after it finishes, and
compare the manifest version with its output before npm/cargo publication. A
mismatch stops the job. Multiple producers require explicit human workflow editing.
An existing job that exposes no release output still uses the exact-version guard;
review its ordering before accepting the proposal.

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

## Configure a token verification workflow

Token setup requires `.package-registry-manager.json`:

```json
{
  "tokens": {
    "docker-hub": {
      "secret": "DOCKERHUB_TOKEN",
      "verification_workflow": "verify-docker-login.yml",
      "expiry_days": 30,
      "level": "org"
    }
  }
}
```

The default secret level is `org` with visibility `selected`, limited to the
current GitHub repository. Set `level` to `repo` for repository secrets. Expiry
must be an integer from 1 to 90 days. Existing credentials rotate when missing,
invalid, within seven days of expiry, or missing verifiable expiry metadata.

The reviewed workflow must define `workflow_dispatch.inputs.prm_nonce`, include
`${{ inputs.prm_nonce }}` in its `run-name`, and actually validate the credential
without publishing. See [the Docker login example](../examples/registry-credentials/verify-docker-login.yml).
Deploy that workflow to the default branch before running setup. A unique nonce
correlates the dispatched run; only that run's completed success permits old-token
revocation. An unrelated successful run cannot authorize it.

Tokens travel from the dedicated browser to gh-manager through stdin. Mutation
output and browser protocol tracing are suppressed. Setup does not accept attached
browser sessions or full-profile imports for this flow. Review scopes and expiry
in the provider page before creating the token; never paste the value into the CLI.
Replacement verification precedes old-token revocation. If storage or verification
fails, both tokens remain active until the replacement is repaired; GitHub cannot
return the old secret value for automatic rollback.

## Current integration limits

[gh-manager#6](https://github.com/link-foundation/gh-manager/issues/6) is still open.
The published implementation does not provide the required secret commands.
This PR adds a tested adapter contract, not an implementation of those commands
in another repository. It fails before browser credential creation when that
contract is unavailable. The proposed transport is:

```text
gh-manager secret get-metadata NAME --org OWNER --json
gh-manager secret ensure NAME --org OWNER --visibility selected --repos OWNER/REPO \
  --expires-at RFC3339 --token-id REGISTRY_ID --registry REGISTRY
# Credential value is stdin, never an argument.
```

`get-metadata` must return `{present, valid, expires_at, token_id}`; absent credentials
must return `present: false`. `ensure` must replace credentials whose validator was
rejected as well as expired credentials. The `token-id` and `registry` metadata
arguments are the adapter's proposed extension and must be implemented upstream.

Provider pages are not uniform APIs. Browser form helpers require human review and
creation/revocation in the dedicated browser; the generic one-time-value and
identified-list selectors have deterministic tests, not live account validation.
Unrecognized values, identifiers, expiry or token lists stop the flow. Fully
unattended provider creation and API-backed revocation remain unverified.

Docker personal Read & Write tokens are not limited to one repository. Prefer
organization access tokens with image-push permission on one repository where the
account supports them. Central returns a token username/password pair; it must be
stored as a pair and wired into Maven settings, not treated as an ordinary single
PAT. Chrome uses an OAuth client plus refresh token, which does not supply a normal
PAT expiry/identifier. The generic expiring-token path deliberately refuses these
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

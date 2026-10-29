# package-registry-manager (JavaScript)

Node.js implementation of
[package-registry-manager](https://github.com/link-foundation/package-registry-manager).
It discovers package manifests, emits registry setup plans, validates packages
with exact argument vectors, and guides authenticated setup in the default
browser, filling forms in a dedicated visible browser profile.

```bash
npx package-registry-manager inspect --repository /path/to/repository
npx package-registry-manager plan --repository /path/to/repository --format json
npx package-registry-manager setup --repository /path/to/repository --registry npm
npx package-registry-manager setup --repository /path/to/repository --all --execute
```

The package installs the command as both `package-registry-manager` and
`package-registry-manager-js`; use the second name when the Rust command of the
same name is also on your `PATH`.

Setup is a dry run unless `--execute` is present. It can publish the first
version after confirmation; later versions publish through CI. Missing CI
publishing jobs are offered in a draft pull request, and PyPI builds select an
interpreter satisfying `requires-python`. See the repository README for the
registry walkthroughs and safety details.
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

For legacy crates.io consumers, use `setup --org ORG --registry crates-io
--rotate-token CARGO_TOKEN --execute --browser-import auto`. Rotation preserves
scopes and GitHub visibility, stores an expiry companion secret, verifies every
consumer in CI before revocation, and lists migration targets. Ordinary crates.io
setup uses trusted publishing. `inspect --org ORG --token-expiry-days 7` opens the
private browser token inventory for advance expiry warnings. See the
[rotation configuration](../README.md#configuration) for matching and failure behavior.

`setup --org ORG --registry crates-io --migrate --execute --browser-import auto`
proposes a reviewed conversion of existing jobs and reusable callers. After merging,
rerun it to register trust, and rerun after a real OIDC release to remove verified
legacy secrets. Shared organization secrets remain while other consumers exist.
The same flow supports `--registry npm` and `--registry pypi`; see the
[migration workflow](../README.md#configuration) for supported layouts and safeguards.

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

See the [bootstrap, wrapper and credential guide](../docs/registry-setup.md) for branch/PR setup, dual npm names, registry policies and integration limits.

Migration tries trusted publishing first and retains token fallback and expiry
metadata by default. `--delete-token-secrets` explicitly opts into cleanup after
OIDC publication and verification that no workflow still uses the credentials.
Saved crates.io API sessions run headlessly; sign-in opens a visible window only
when user action is needed. Organization migration skips and reports scan errors
while continuing for clean repositories.

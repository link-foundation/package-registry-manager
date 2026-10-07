# Issue 39: requirements, research and implementation evidence

Scope: [parent #39](https://github.com/link-foundation/package-registry-manager/issues/39),
[#36](https://github.com/link-foundation/package-registry-manager/issues/36),
[#37](https://github.com/link-foundation/package-registry-manager/issues/37), and
[#38](https://github.com/link-foundation/package-registry-manager/issues/38), including
all comments. Deliver all changes in [PR #40](https://github.com/link-foundation/package-registry-manager/pull/40).

## Work plan

- [x] Confirm the prepared branch and clean initial worktree.
- [x] Read contributing guidance, parent and child issues, and all issue/PR comments.
- [x] Research registry policies, token APIs, existing components, and gh-manager's secret contract.
- [x] Trace both implementations, including discovery, plans, setup, browser flows, and workflow proposals.
- [x] Add minimal failing regressions before each behavior change.
- [ ] Implement every requirement in Rust and JavaScript; preserve unrelated behavior.
- [x] Document usage, assumptions, evidence, and known service limitations.
- [x] Add release changelog fragment (the repository's automatic release trigger).
- [x] Run focused tests, then all contributing-guide checks and script tests.
- [x] Fetch current main (already included), review the implementation diff, and commit atomic changes.
- [ ] Push only issue-39-c0ec0775d7d2; update title/body with all four closing references.
- [ ] Inspect CI timestamps/SHAs, preserve failed logs, resolve failures, and wait for completion.
- [ ] Confirm clean worktree and mark PR #40 ready.

## Requirement inventory and solution choices

| Requirement | Possible approaches | Selected implementation plan |
| --- | --- | --- |
| #36.1 Bootstrap from branch or PR before merge | Fetch ref into isolated worktree; require renamed manifest on main | Add `setup --ref`, resolve branch/PR head, pack isolated worktree, validate the selected identity/version, report relationship to main. |
| #36.2 Keep both names receiving releases | Manifest-name overrides and alias configuration; wrapper packages | Recognize and document wrapper manifests, detect package-specific workflow coverage, list both registry/trust states, warn about release/version gaps. |
| #36.3 Check npm availability and name policy before approval | Registry lookups plus normalized variants and local dry-run; third-party name libraries | Validate names and lookup punctuation-normalized conflicts, run `npm publish --dry-run` before publishing approval; stop E403 without retry and suggest alternatives. Explain that dry-run cannot guarantee acceptance of undocumented server policies. |
| #36.4 Optional old-name cross-link | Explicit deprecate command; README notice | Document opt-in README/deprecation cross-links and preserve both names by default. |
| #37.1 Ignore script comments and nonexecuted publish text | Syntax parsers; conservative command-aware matching | Strip shell/Python/YAML and JS/Rust comments with quote awareness; detect command invocation forms and ignore inert documentation/string mentions; test preflight-only scripts. |
| #37.2 Exclude snapshots and experiments | Directory exclusions; workflow-to-manifest association | Skip docs, experiments, and case-studies paths by default, report verbose skips, require genuine package-specific publishing coverage for automatic setup selection. Test browser-commander layout. |
| #38 Registry decision table | Token default everywhere; explicit per-registry policy | Use trusted publishing for npm/PyPI/crates/RubyGems/NuGet/JSR; built-in GITHUB_TOKEN for GHCR; scoped expiring tokens for Docker Hub/Central/VS Code/Open VSX/Chrome Web Store. Print policy in plans. |
| #38.1 Detect required token credentials | Secrets and registry publish/login commands | Detect each registry's publishing jobs and secret names, including docker/login-action and mvn/vsce/ovsx/webstore. |
| #38.2 Check stored credentials | gh CLI metadata; gh-manager lifecycle operations | Integrate gh-manager secret status/ensure contract; consider absent, expiring, and validator-rejected credentials. |
| #38.3 Browser token creation | Registry-specific forms/APIs using existing browser automation | Reuse dedicated profile and scoped sign-in import, select minimum publishing scope and expiration, keep token values out of output and command arguments. |
| #38.4 Store and verify token | Direct GitHub secrets calls; gh-manager adapter | Send credentials through gh-manager stdin, org selected repositories by default/repo override, expiry metadata, dispatch credential verification and wait for success. |
| #38.5 Rotate and revoke | Revoke first; provision/verify before revoke | Verify replacement before revoking previous token, verify revocation, retain safe failure state if replacement verification fails. |
| #38 publish-job bug | Depend on version-producing release job; uncached version guard | Tie proposals to release job outputs when present, check uncached exact versions before npm/cargo publication, use PyPI skip-existing; test guards and versioned checkout. |
| #39 Parent scope and PR closing | Separate PRs; one PR | Single PR with `Fixes #39`, `Fixes #36`, `Fixes #37`, `Fixes #38`, explicit status for any preexisting resolution. |

## Initial CI evidence

The initial head is `6b2e57c161541475da83702e738cc9724839ae5f` (2026-10-07 08:19:55 UTC).
Upstream Dependencies run `37593076056` started 08:20:09 UTC and failed on that head.
Its full log is preserved locally in `ci-logs/upstream-dependencies-37593076056.log`.


## Root causes and scope review

The #33 script traversal fed whole files into publishing regexes. Comments and
ordinary strings could therefore count as publishing. Discovery also retained
case-study/experiment manifests, and all manifests of one registry inherited a
single repository-level workflow result. These paths existed in Rust and JS.
Container discovery independently scanned raw YAML; it needed the same fix.

The npm bootstrap hard-coded `git fetch origin HEAD` and inspected the local
manifest. Selecting an unmerged rename was impossible without fetching and
inspecting that ref. One manifest still has one real npm identity; we chose the
explicit wrapper option rather than mutate package identity inside an existing
tarball. Per-step working directories and per-manifest coverage make that option
reviewable. Divergent wrapper versions and missing publishing coverage warn.

The publish proposal had neither a registry-version gate nor a dependency on a
version producer. A normal main push could republish an immutable version or
publish the commit before a version bump. Proposals now add exact-version gates,
concurrency, PyPI skip-existing, and dependencies/version comparison when a
`published_version` output exists. The guard fails closed on errors other than 404.

Recent related work reviewed: [PR 35](https://github.com/link-foundation/package-registry-manager/pull/35)
(workflow proposals and multi-registry batching), [PR 29](https://github.com/link-foundation/package-registry-manager/pull/29)
(uncached publication lookups), and [PR 27](https://github.com/link-foundation/package-registry-manager/pull/27)
(crates.io API bootstrap and scoped import). Existing command-vector execution,
dedicated browser profiles and token auditing are reused.

## Online research and existing components

Research checked on 2026-10-07. Sources below are registry maintainers or the
maintainers of the component, not third-party tutorials.

| Topic / existing component | Findings and effect on the solution |
| --- | --- |
| [npm publish](https://docs.npmjs.com/cli/v11/commands/npm-publish/) | Name/version pairs are immutable. Dry-run validates local publication inputs without uploading; it cannot promise acceptance of unpublished server policy. Keep prechecks before approval and handle actual E403 once. |
| [npm moniker rules](https://blog.npmjs.org/post/168978377570/new-package-moniker-rules.html) | Punctuation variants can conflict. Scoped or more descriptive names are practical alternatives. |
| [npm-name](https://github.com/sindresorhus/npm-name) | A maintained availability component with the same client-side limitations: punctuation checks are best effort, and unpublished similarity checks have no public API. A small bounded implementation in both languages avoids introducing different JS/Rust policies. |
| [RubyGems trusted publishing](https://guides.rubygems.org/trusted-publishing/) | Pending/existing publishers and `rubygems/release-gem` support OIDC. Add gemspec discovery and trusted setup, never a stored fallback token. |
| [NuGet trusted publishing](https://learn.microsoft.com/en-us/nuget/nuget-org/trusted-publishing) | A policy links owner/repository/workflow/environment. `NuGet/login` exchanges OIDC for a temporary key; the key is not a persistent secret. Reuse dotnet pack and policy setup. |
| [JSR publishing](https://jsr.io/docs/publishing-packages) | Repository-linked publishing works through GitHub Actions OIDC. Reuse deno/jsr publishing and avoid a stored JSR token fallback. |
| [Docker organization access tokens](https://docs.docker.com/security/access-tokens/organization-access-tokens/) | OATs offer per-repository image-push scopes and expiry, with account prerequisites. Personal Read & Write tokens are broader; they cannot satisfy a one-repository promise. Document that distinction. |
| [Docker Hub API](https://docs.docker.com/reference/api/hub/latest/) | Token API operations exist, but require an authenticated provider session and appropriate account permissions. An API-backed adapter is stronger than inferring revocation from a UI list. |
| [Central portal tokens](https://central.sonatype.org/publish/generate-portal-token/) | The token is a username/password pair, with expiry. A generic single-PAT adapter cannot configure both Maven settings values correctly. Do not report this as a verified end-to-end integration. |
| [Azure DevOps PATs](https://learn.microsoft.com/en-us/azure/devops/organizations/accounts/use-personal-access-tokens-to-authenticate) | VS Code publishing should use Marketplace Manage scope and finite expiry. Existing vsce is reused for local packaging. |
| [Chrome Web Store API](https://developer.chrome.com/docs/webstore/using-api) | Chrome uses OAuth client credentials plus a refresh token, not an ordinary expiring PAT. Provider-specific OAuth renewal and revocation require a different adapter. |
| [gh-manager secret API issue](https://github.com/link-foundation/gh-manager/issues/6) | The linked storage implementation is still requested, not shipped. GitHub never returns secret values; expiry needs separate registry metadata. A safe adapter must refuse unsupported storage before minting tokens. |
| Existing browser-commander / command-stream | Reuse the browser catalogue, dedicated profiles, scoped import and exact argv. Refresh all five link-foundation dependency versions together to fix the initial upstream check. |

An AST parser per script language could distinguish more dynamic execution paths,
but would need separate shell, Python, JS and Rust parsers and still cannot prove
runtime branches. The chosen lexer/command-position detection covers the reported
preflight comments, inert help text and executable forms without adding a large
parser stack. Dynamic scripts should expose explicit publishing steps.

## Tests and reproducible evidence

Before changing behavior, minimal tests reproduced comment/preflight detection,
workflow inert text, missing directory exclusions, unmerged-ref inspection,
wrapper version gaps and missing proposal guards. Local failing logs were saved
under ignored `ci-logs/` (`issue-37-before.log`, `workflow-text-before.log`,
`container-before.log`, `main-version-before.log`, `wrapper-version-before.log`,
`issue-38-proposal-before.log`, and `token-cycle-before.log`).

Automated coverage now includes:

- Both languages: comments and quotes, preflight-only scripts, executed npm calls,
  skipped snapshots/experiments, wrapper publishing coverage, safe PR refs, disposable
  worktree cleanup and remote-main name/version reporting, new registry detection.
- Both languages: registry credential decision table, absent/expiring/rejected
  credential rotation, verification-before-revocation, and retained old token on failure.
- JavaScript: real subprocess stdin transport suppresses token echo, org selected
  repositories/repo override, unsupported storage stops before browser creation,
  and nonce-correlated workflow verification rejects unrelated successful runs.
- Both proposal templates: producer dependency and nonempty version output,
  exact-version checks and PyPI skip-existing.
- [Version guard probe](../../../experiments/issue-39/version-guard-probe.mjs): run
  `node experiments/issue-39/version-guard-probe.mjs` for ten bounded cases.
  It executes the generated Python guard with mocked HTTP responses for npm and
  cargo. Existing versions skip, 404 publishes, and 403/network/version mismatch
  stop before publication. No live package publication occurs.

See [the user guide](../../registry-setup.md) and
[verification workflow example](../../../examples/registry-credentials/verify-docker-login.yml).

## Requirement status and remaining limits

#36 and #37 have implemented regressions and fixes in both ports. Cross-linking
uses the documented opt-in README/deprecation path. A bootstrap cannot make a
previously used npm version reusable; the guide explains the merge-version rule.

#38's decision table, added detection/setup plans, guarded workflow proposals,
credential-cycle state machine, stdin-only transport, organization/repo scope,
expiry checks and correlated verification are implemented and tested. **The full
live browser-to-gh-manager cycle is not complete for every token provider.**
The storage commands depend on unresolved upstream #6. Browser form helpers
currently require human creation/revocation and known value/list selectors; they
are not validated against live accounts. Central's two-part credential and
Chrome's OAuth refresh lifecycle need provider-specific adapters, while per-repo
Docker scope needs an eligible organization token. OIDC cleanup currently audits
repository secrets; organization-wide shared-secret removal still requires an
organization audit. These are material limitations, not claims of resolution or
requirements silently dropped from the inventory.

No live credentials were created, stored, revoked or published during this work.
The adapter fails safely when its storage contract, expiry, identifier or token
list cannot be verified. No follow-up PR was created or used to move this scope
elsewhere; the evidence and current limits remain in PR 40 for review.

## Local validation

All contributing-guide checks pass: Rust formatting and warning-free Clippy,
400 Rust tests, 232 passing JavaScript tests (one opt-in browser test skipped in
the default suite), 98 release-script tests and 30 root-script tests. Both opt-in
browser smoke tests also passed with installed Chrome under Xvfb. Required docs,
file size and latest-upstream dependency checks pass. The JavaScript generated
version-guard probe uses one Python interpreter for ten finite cases, keeping
parallel CI builds inexpensive without increasing its 15-second test timeout.

The new per-package filter initially dropped root reusable workflows in repositories
with several npm manifests. `reusable-before.log` reproduces that regression;
matching caller jobs are now retained in both ports, with a regression in each.
DOM helper tests execute the generated JavaScript and confirm that unoffered
scopes are not selected and a missing token list cannot prove revocation.

Remote main remains `d4540c0e8d0378653478918a917ba46cd1c7c0ff` and is already an
ancestor of this branch. The changelog fragment triggers the normal release;
package versions are intentionally left to the release workflow.

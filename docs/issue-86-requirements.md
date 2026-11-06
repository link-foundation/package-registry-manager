# Issue 86: requirements, research, implementation and verification

Scope: [#86](https://github.com/link-foundation/package-registry-manager/issues/86),
[#83](https://github.com/link-foundation/package-registry-manager/issues/83),
[#84](https://github.com/link-foundation/package-registry-manager/issues/84),
[#85](https://github.com/link-foundation/package-registry-manager/issues/85).
All issue bodies, native sub-issues and comments and PR 87 conversation, inline
comments and reviews were read (no comments/reviews at initial investigation).

## Work plan

1. Inventory requirements and inspect both ports and recent PRs 82/76/74.
2. Draft narrow NuGet policy/readback and provider identity repairs.
3. Draft authenticated Central namespace, first-release GPG, and token lifetime handling.
4. Draft exact Gradle metadata graph verification in both ports.
5. Run focused reproducing tests, lint/format and resource-bounded Rust checks.
6. Commit atomic changes, synchronize main, review the full PR diff, push only the
   prepared branch; inspect current-head CI logs, finalize PR body and mark ready.

## Requirement inventory and solution choices

| ID | Requirement | Chosen implementation and verification plan |
| --- | --- | --- |
| 86.1 | Address all three issues in one PR, including later feedback | Both ports and shared fixtures; all rows below verified before completion |
| 86.2 | Close parent and each child individually; explain already-resolved cases | Separate Fixes #86, Fixes #83, Fixes #84, Fixes #85 in PR body |
| 83.1 | Missing optional Gallery staging observable must not throw | Feature detection in both copies of the official-page adapter; representative model test |
| 83.2 | Explicitly disable unlist; keep one push operation and exact package IDs | Required scope fields remain fail-closed; optional staging disabled when available |
| 83.3 | Normalize saved machine scopes independently of display labels and pending edits | Inspect official Gallery source, read persisted policy scopes from fresh page state; translated-label and unsaved-edit tests |
| 83.4 | Reload after create or edit, check profile/owner, GitHub IDs, workflow/environment, subjects, operations, permanent activation | Effective policy comparison; create/edit/reload/idempotence regressions |
| 83.5 | Separate CI entrypoint from NuGet reusable OIDC identity; trace nested calls; reject ambiguity | Provider-specific resolver from actual publishing/login jobs; callers retained for retry; npm/PyPI unchanged |
| 83.6 | Use normal reviewed installed gh-manager dependency, update both pins/lockfile | Immutable gh-manager 1.5.0 source pin; real-library mocked HTTP integration |
| 83.7 | Preserve fallback secrets and credential privacy | No fallback deletion/rotation changes; secret write/log assertions |
| 83.8 | Distinguish configuration, OIDC exchange, accepted push, exact public package | Keep existing independent evidence/gates and test them |
| 83.9 | Detect first-match policy shadowing after rename/first publication; repair original exact subject and new-package authorization, preserve owner/creator/GitHub identity and unrelated policies | Use Gallery creation order before scope comparison; edit original in place with configured `package:push`, wait for edit, reload; older/newer regression in both ports and mocked push rejection/acceptance, never infer delivery from exchange |
| 84.1 | Authenticated ownership readback must work before first release and after indexing lag | Official authenticated portal UI readback; never infer account ownership from public artifacts |
| 84.2 | Authorized namespace registration and GitHub proof creation/submission | Portal UI plus official GitHub API/CLI; bind account/namespace/org and immutable IDs |
| 84.3 | Retain proof cleanup obligation; no token scope escalation | Persist cleanup state; delete only newly created empty proof repo via existing permission or report pending |
| 84.4 | Explicit first-release signing, reuse suitable existing material first; generation only when authorized | Official GPG in isolated temporary home; explicit config and task confirmation; never silently replace unreadable existing GitHub secrets |
| 84.5 | RSA4096, repository identity, one-year expiry, encrypted key/passphrase storage and expiry metadata | GPG generation/export; gh-manager stdin/in-memory secrets and nonsecret metadata |
| 84.6 | Detached signing round trip, public-only propagation, independent keyserver readback/fingerprint and signature verification | Separate verifier keyring; fingerprint/expiry checks; cleanup in finally |
| 84.7 | Read actual Central token expiry/lifetime/name, pass expiry metadata to gh-manager and expose to health | Exact server date only; unknown expiry explicit; retain existing pair without inventing validity or rotating |
| 84.8 | Idempotent verified setup; configuration and release evidence separate | Reuse authenticated namespace, signing material and pairs; no artifact upload; keep deployment, public signatures, variants, consumers, GitHub releases separate |
| 85.1 | Preserve/validate full available-at object with string module and group/version | Shared realistic Gradle 1.1 fixtures, both ports; no undefined coordinates |
| 85.2 | Reject/diagnose pointer version mismatch; resolve exact referenced metadata/artifacts | Exact version and URL validation; recursively traverse bounded graph with deduplication |
| 85.3 | Validate configured platforms against actual metadata and referenced files | Missing expected platform remains partial; GET platform modules and inspect inline files and redirects |
| 85.4 | POM-only presence cannot certify binaries/signatures/consumers | Explicit verification evidence fields; public files checked separately, signatures and consumer gates unclaimed |
| 85.5 | Separate absent root metadata, partial delivery, malformed metadata, transport errors | Classified results and focused cases for 404/invalid JSON/HTTP 5xx/network errors |
| 85.6 | Fixture includes repeated coordinate references, inline files, platform redirects and component.url back to root | Shared realistic root/platform JSON; owning root component identity accepted for redirected platform metadata |
| 85.7 | Do not upload immutable links-notation 0.31.0 | Read-only live public metadata check only |

## Research and alternatives

- [NuGet trusted publishing](https://learn.microsoft.com/en-us/nuget/nuget-org/trusted-publishing)
  documents exact workflows, environment, scopes and permanent activation. Use
  Gallery's existing model, not a separate API-key mechanism.
- [Official Gallery implementation](https://github.com/NuGet/NuGetGallery/blob/main/src/NuGetGallery/Scripts/gallery/page-trusted-publishing.js)
  is the authority for machine-scope readback; labels/pending edits are insufficient.
- [Gallery's policy ordering](https://github.com/NuGet/NuGetGallery/blob/main/src/NuGetGallery/Controllers/UsersController.cs)
  and [OIDC policy evaluator](https://github.com/NuGet/NuGetGallery/blob/5969489b9f4928c70dbbb645768c47a525133f9b/src/NuGetGallery.Services/Authentication/Federated/FederatedCredentialPolicyEvaluator.cs)
  sort by creation date. The evaluator returns the first successful identity match
  before applying package subjects/scopes. Use the server's ordered policy list,
  edit the effective original in place, and reject conflicting owners or unbound
  identities. Creating a newer matching policy cannot repair the earlier scope.
- [Sonatype namespace registration](https://central.sonatype.org/register/namespace/)
  documents temporary public proof repositories and deletion after verification.
  Public Maven search was rejected as an ownership source.
- [Central token generation](https://central.sonatype.org/publish/generate-portal-token/)
  exposes a selected expiry and a one-time credential dialog. Do not calculate a
  guessed timestamp from “one month”.
- [Official GnuPG manual](https://www.gnupg.org/documentation/manuals/gnupg/GPG-Options.html)
  documents isolated homedirs, agent behavior and exact command options.
- [MSYS2 path handling](https://www.msys2.org/docs/filesystem-paths/) explains
  the native/Unix path boundary. GPG subprocesses work inside their keyring with
  relative paths, supporting Git's MSYS GPG and native Windows GPG alike.
- [Central GPG requirements](https://central.sonatype.org/publish/requirements/gpg/)
  specifies detached signatures, public keyservers and primary-key signing.
  Use installed GnuPG rather than introduce a crypto implementation/library.
- [Gradle Module Metadata 1.1](https://raw.githubusercontent.com/gradle/gradle/master/platforms/documentation/docs/src/docs/design/gradle-module-metadata-latest-specification.md)
  defines available-at objects and file records. Use its existing JSON format;
  no Gradle execution or heavyweight dependency is needed for registry readback.
- Existing components: browser-commander for authenticated portal UI, gh-manager
  1.5.0 for encrypted secret storage/expiry health and CI repair, official GitHub
  REST/gh for proof repositories, GnuPG for key lifecycle and signature checks,
  built-in fetch/reqwest and JSON parsing for public metadata. OpenPGP.js and
  Maven/Gradle plugins are alternatives, but add dependencies or invoke builds
  unnecessarily; retain existing downstream publishing plugins.

## Verification evidence

- `node experiments/issue-86/reproduce.mjs`: the 0.27.0 Gallery model throws
  without `StageScopeChecked`; the repaired adapter submits. The original Gradle
  string map produces `undefined:undefined`; the repaired realistic fixture verifies.
- Initial NuGet focused suite: 35 passed, including create → reload → second setup,
  localized persisted scopes, nested workflow identity, ambiguity and the normal
  installed gh-manager 1.5.0 encryption/expiry-health path.
- Added rename/shadowing regression fails before the fix (`active` instead of
  `shadowed`). The focused Gallery/policy/setup/signing suite now passes 17 tests.
  It verifies the original key is edited once, exact new-package `package:push`
  authorization, edit completion, persisted reload, second-run idempotence,
  retained owner/creator/immutable IDs/activation, and untouched unrelated policies.
  The push adapter mock rejects the older historical subject after a successful
  OIDC exchange and accepts the same artifact after repair; live push is a separate gate.
- Central focused suite: 36 passed, including real RSA4096 generation, detached
  signing, a separate verifier keyring, proof reuse and temporary-home removal.
  External keyserver send/receive is replaced with public-key export/import in the
  GPG test. Portal and GitHub mutations use deterministic account-bound mocks.
- Additional Central lifetime/cleanup suite: 28 passed, including actual expiry
  passed to both secret writes, nonsecret token identity, idempotent second setup,
  expired/revoked token refusal, wrong organization/suspension and changed-proof
  deletion protection.
- Full JavaScript suite: 463 passed, five environment-dependent skips; syntax,
  ESLint, Prettier and duplication checks pass. Later added tests also pass focused
  checks and run in the same full CI suite.
- Read-only live links-notation 0.31.0 verification: all eight coordinates / seven
  platform variants and 19 referenced files available. Hashes, public signatures,
  consumers and GitHub release remain separate unclaimed gates.
- Shared `missing-platform.json` fixture: actual referenced JVM module returns
  404 while root metadata exists; both ports retain the required coordinate and
  expect `partial`. Root metadata absence, malformed records, wrong versions and
  transport failures have distinct regression cases.
- `cargo fmt` passes. The resource-bounded local Rust check (one compiler,
  debug info disabled) was killed while compiling upstream `chromiumoxide_cdp`;
  no repeated heavy local build. Existing CI runs Rust clippy, all unit/integration
  tests, doctests, package checks and platform builds on hosted runners.
- CI review identified a potentially slow ambiguity regex and an allowlist
  analyzer warning; direct string checks replace both. Rust doc lint corrected.
  Final current-head CI results are recorded in PR 87. Windows GPG
  diagnostics are restricted to synthetic test keys; actual signing diagnostics
  remain private. Windows CI reproduced Git's GPG treating a backslash homedir
  as relative; the real signing test now uses per-keyring subprocess working
  directories and relative proof paths, including when temp and repo drives differ.
  macOS CI exposed GPG agent startup failure in its long per-user temporary path;
  macOS signing homes now use short private directories under `/tmp`, keeping
  agent socket paths within the Unix socket limit. The same real-signing CI test
  verifies generation and cleanup across platforms.

No live private account setup or keyserver propagation is inferred from mocks.
No downstream package uploads are performed. The implementation adds the
account-dependent paths; operators still receive explicit incomplete results
when authentication, permissions or independent readback are unavailable.

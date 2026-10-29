# Issue 73: requirements, research, implementation and remaining verification

Review date: 2026-10-29. Scope: parent #73, native children #65–#72,
all issue comments, all three PR #74 comment types, and required downstream
Data.Doublets #538, C# template #80 and gh-manager #29. This document records
implementation evidence separately from live publication acceptance. PR #74
must remain unmerged while the blocked acceptance criteria below remain open.

## Requirement inventory and solution choices

Each row is a requirement, its implementation approach, and its verification.
Modules named below exist in both `js/src` and `rust/src`, with setup helpers
under `rust/src/setup`. Deterministic tests do not assert live account access.

| Issue / requirement | Approach and implementation | Verification / remaining work |
| --- | --- | --- |
| #65 classify owner-setup separately from OIDC errors | `bootstrap-recovery` extracts every exact name from the explicit preflight diagnostic; `release-runs` retains this evidence | bootstrap-recovery tests distinguish unrelated failures |
| #65 retry only after prerequisites are repaired | Batch configuration precedes recovery; fresh probes establish every missing name and publisher; current default SHA must match the failed run | bootstrap prerequisite tests; live gh-upload-log recovery was already completed in #71 |
| #65 causal failed jobs and fresh preflight equivalent | Recheck the exact missing identities and publisher tuples immediately before gh-manager repair; select only jobs containing the diagnostic | Unrelated failed jobs are not selected; success is checked after waiting |
| #66 discover each generated alias | `discovery` / `audit_discovery` expand `publishAliases`, retain source/version/generator/all names and workflow identity | audit-targets and generated-artifact tests |
| #66 independent state, selection and trust | Aliases enter the normal package inventory, organization scan, selection and separate publisher plans | account and selection regression tests |
| #66 actual artifact validation | Run the declared generator in an isolated worktree, validate generated manifest name/version, then inspect the actual pack result | generated-artifact test; refuse unresolved generator or escaped output |
| #66 verify both released names | Read successful npm publish lines to obtain actual versions, then probe each exact public version | npm-release tests; no assumption that source version equals release version |
| #67 real Gradle publications only | Require resolved coordinates and publication/component, or explicit reviewed config; skip application/build-only definitions with reasons | audit-targets tests cover Android and unresolved definitions |
| #67 examples versus executable NuGet tools | Skip unpublished examples; retain explicit publication evidence and configured exact-path opt-in | discovery tests |
| #67 historical snapshots in every ecosystem | Shared skip rule excludes `dev/log` copies unless configured in `publish` | JS/Rust audit-targets fixtures |
| #68 expected/latest/exact evidence | `publication-state` augments registry state and account findings with exact-version existence and evidence URL | publication-state tests |
| #68 missing newer release versus stale source | Compare resolved numerical versions conservatively; keep absent manifest version distinct from source older than latest | state tests; incomparable versions remain absent rather than guessed stale |
| #68 uncertainty never becomes absence | Only supported, successful metadata or explicit 404 establishes a result; authorization, transport and invalid responses remain unknown | registry-state tests; malformed NuGet versions and Go metadata remain unknown |
| #68 Maven Central | Read Maven metadata and exact POM; retain separate evidence | Maven state fixture |
| #68 Packagist | Read Composer p2 versions including normalized leading `v`; validate response schema | Packagist fixture |
| #68 Go modules | Read escaped proxy list and latest; distinguish empty tagged list with importable pseudo-version | Go fixtures validate canonical latest/list versions, uppercase path escaping and pseudo-versions |
| #68 inherited Cargo versions | Resolve explicit/nearest workspace.package.version without executing build code | Rust workspace and JS audit-targets fixtures |
| #68 unsupported registries | Emit explicit unknown/unsupported findings rather than silently omit coverage | account-state tests; C++ channels remain unverified |
| #69 full normalized collision inventory | Group all publishable targets, including aliases, before selection or mutation; PyPI punctuation and Cargo hyphen/underscore normalization | identity-conflicts tests |
| #69 complete source evidence | Keep repository, manifest, source artifact, version, public owners/repository and permissions unknown | identity/account fixtures |
| #69 deliberate resolution | Require distinct source names or identical reviewed shared-artifact mapping with full source/version inventory and one canonical publisher | mapping tests reject incomplete/noncanonical plans |
| #69 exclude copies | Apply discovery exclusions before identity grouping | audit-targets fixtures |
| #70 discover C++ distribution | Statically read root CMake project, Conan recipe and vcpkg manifest; resolve shared CMake version without running recipes | audit-targets fixtures |
| #70 distinguish build/test/examples | Use the shared exclusions and distribution discovery rules | discovery tests |
| #70 workflow/channel/remote evidence | Detect publishing workflows and required names; gh-manager reads effective Conan remote and credential health | distribution-channel fixture; evidence omits credential values |
| #70 useful setup plans | Plan CMake configuration, Conan inspection/create/test, vcpkg dry-run, CI/remote credentials; unsupported provider publication remains unverified | plan tests; no public claim from local manifest presence |
| #71 #58 integration baseline | Keep anonymous crates publisher settings and expired-log handling fixes already merged through #64; update gh-manager pin to merged #29 implementation | upstream dependency and existing gateway tests |
| #71 all tool gaps | Apply #65–#70 across both ports; retain historical audit lists below and identity prerequisites | full suites and this inventory |
| #71 controlled publication order | Resolve collisions/owners first, propose missing workflows, validate artifacts, authenticate, then normal CI releases | The historical 49/14 lists are inputs, not authorization to publish placeholders |
| #72 official trusted publishing first | SHA-pin NuGet/login; use its documented `user` input and job-level id-token write; temporary key remains in the publishing job | nuget-workflow tests and pinned action.yml inspected |
| #72 provided fallback only | Shared policy and actual embedded controller try OIDC first, ignore an invalid fallback on success, then use caller-designated custom/NUGET_TOKEN/NUGET_API_KEY only after unavailable/rejected OIDC | nuget-auth and Python controller decision tests |
| #72 missing username / both failed | Missing user skips exchange and yields explicit unavailable state; provided fallback remains eligible; both failed fails the job | auth fixtures; no credential minting |
| #72 unchanged bounded auth retry | Pack once; validate nupkg identity and SHA-256; one fallback attempt after exact public absence; never retry transport/build/probe uncertainty | Python controller tests |
| #72 report real method | Return attempts, selected method, publication status and trust-repaired separately; do not equate fallback with repaired trust | auth/CI fixtures; live evidence still required |
| #72 preserve secrets | NuGet policy, migration, credential audit and cleanup retain configured fallback names, including organization credentials | token and credential-cycle tests in both ports |
| #72 no persistent temporary keys | Key is passed job-locally to the controller; output contains fixed classifications only | generated workflow review; no cross-job output or secret write |
| #72 exact account/policy model | Distinguish login profile, package owner, immutable GitHub IDs, workflow/environment, exact IDs and one allowed push operation | wrong-owner/duplicate/inactive/temp/mismatch/multi-ID tests |
| #72 real supported management | dotnet signer trust is not publisher configuration; browser-commander drives official Gallery model when no supported policy API exists | nuget-page DOM fixtures based on Gallery source |
| #72 idempotent policy setup | Discover before create, refuse overlapping repository policy duplication, verify actual account and fresh active saved policy | policy/page tests; no live authenticated Gallery account in this workspace |
| #72 GitHub responsibilities | gh-manager ensures effective NUGET_USER; reviewed workflow code supplies permissions and OIDC/fallback wiring | variable readback checked; pending migration remains pending |
| #72 inspect all local sources | Deduplicate physical profiles; test cookie candidates against actual identity, then saved-login candidates; continue after invalid session | nuget-browser tests |
| #72 route-specific scope | Import only NuGet plus selected work/personal/GitHub provider domains | route fixture; use configured scope |
| #72 target credential preservation | Password migration always gets a fresh disposable target until browser-commander #189 supports merging | target preservation regression; controlled profiles cleaned after browser closes |
| #72 passkeys and interrupted sign-in | Export unsupported is distinct from presence unknown; unsuccessful reuse leads to interactive platform authentication; --yes fails honestly | credential diagnostics and interruption handling; upstream #119 limits discovery |
| #72 PackageVerify HTTP 400 | Preserve bounded redacted diagnostics; push-only incompatibility and derived-key subject scope do not prove authorization or failed ownership | realistic HTTP 400/derived-key tests |
| #72 settings recovery | Require actual verified settings change plus causal failed-job authentication evidence; gh-manager repairs and waits | nuget-ci fixtures; required workflow permissions supplied |
| #72 code recovery | Follow existing run at fresh default-branch SHA; never retry old code or duplicate a push run | current-main supersedes historical-failure fixture |
| #72 final evidence | Report URL, attempt, SHA, jobs/steps and actual publication method; require successful gates, exact download and consumer restore/build | skipped/log-unavailable fixtures; live downstream criteria blocked below |
| #72 both downstream repositories | Required C#/C++ and automatic/instant paths are inventoried below; this manager cannot substitute its PR tests for those main releases | OPEN external acceptance criteria below |
| #73 one PR / closing references | All local implementation and tests stay on prepared branch and PR #74; independent closing keywords for all nine issues | PR body review; never merge with incomplete criteria |

## Configuration decisions

Create `.package-registry-manager.json` only for reviewed project decisions.
No account names or package identities are inferred from an unrelated template.

```json
{
  "publish": ["examples/PublishedTool/PublishedTool.csproj"],
  "gradle": [{
    "manifest": "library/build.gradle.kts",
    "group": "com.acme",
    "artifact": "library",
    "version": "1.2.3",
    "component": "java"
  }],
  "nuget": {
    "username": "release-profile",
    "package_owner": "Acme",
    "package_ids": ["Acme.Library", "Acme.Tool"],
    "operations": ["package:pushversion"],
    "sign_in_route": "github"
  }
}
```

`publish` uses exact repository-relative manifest paths. It opts a known source
into discovery; it does not establish account authorization or publication.
NuGet sign-in routes are `work`, `personal` or `github`. Use `package:push` for
new package creation or `package:pushversion` for existing packages; policy setup
selects one operation supported by the Gallery rather than widening scope.
GitHub immutable IDs are read fresh. Workflow permissions remain reviewed code.

For intentionally shared artifacts use `shared_artifacts`, with `registry`,
`name`, `reviewed: true`, a stable `artifact` label, complete `sources` entries
(`repository`, `manifest`, `version`) and a canonical `publisher` (`repository`,
`manifest`). Every colliding source must carry the same complete mapping. Select
only the canonical publisher. A mapping never renames another package's tarball.

## Research and existing components

- [Execa's Windows support](https://github.com/sindresorhus/execa/blob/main/docs/windows.md)
  resolves `PATHEXT` and safely runs `.cmd` launchers such as npm. The existing
  command-stream Execa adapter now handles captured commands throughout the
  JavaScript port; its direct `exec` API did not resolve these launchers. Rust
  already resolves `PATHEXT` through its shared `resolve_program` helper.
- [NuGet trusted publishing](https://learn.microsoft.com/en-us/nuget/nuget-org/trusted-publishing)
  describes temporary, one-hour credentials and independent package ownership.
  [NuGet/login](https://github.com/NuGet/login) is the official exchange action;
  inspected pinned action.yml uses `user`, not `username`. Reuse it instead of
  implementing the exchange or storing an issued key.
- [NuGet versioning](https://learn.microsoft.com/en-us/nuget/concepts/package-versioning)
  documents flat-container normalization. NuGet.Versioning is the appropriate
  component for .NET clients; the embedded Python verifier implements the narrow
  URL normalization it needs and verifies the downloaded nuspec identity.
- [NuGet Gallery](https://github.com/NuGet/NuGetGallery) is the primary source for
  current policy fields, activation and PackageVerify behavior. Policy creation
  uses the actual Gallery model through existing browser-commander rather than
  inventing a dotnet publisher CLI.
- [gh-manager](https://github.com/link-foundation/gh-manager/pull/30) already
  provides effective variables, health and verified repair orchestration. Both
  ports pin merged revision `0b3ceb1590be7eb0340ae088730698b5ae9bebfc` (1.2.1).
  No second GitHub mutation framework is needed. Health currently lacks an
  environment-scope argument, which the Conan evidence reports explicitly.
- [npm trusted publishers](https://docs.npmjs.com/trusted-publishers/) and
  [npm trust](https://docs.npmjs.com/cli/v11/commands/npm-trust/) distinguish
  owner bootstrap from publisher attachment. Reuse npm's trust readback and
  repository generators; a renamed root tarball cannot validate an alias.
- [Gradle Maven Publish](https://docs.gradle.org/current/userguide/publishing_maven.html)
  requires real publication coordinates/components. Static detection plus
  explicit reviewed config avoids evaluating arbitrary Gradle just to inspect.
- [Composer repositories](https://getcomposer.org/doc/05-repositories.md) define
  p2 metadata; use public read-only data rather than introducing a new registry SDK.
- [Go module reference](https://go.dev/ref/mod) and [Go proxy](https://proxy.golang.org/)
  define uppercase escaping and list/latest behavior; an empty tagged list can
  coexist with a usable pseudo-version.
- [Conan remote commands](https://docs.conan.io/2/reference/commands/remote.html)
  and [vcpkg manifest mode](https://learn.microsoft.com/en-us/vcpkg/consume/manifest-mode)
  provide validation and remote configuration primitives. Neither a local recipe
  nor a dry-run proves an upload. CPack/CMake release channels remain provider-specific.
- [browser-commander #189](https://github.com/link-foundation/browser-commander/issues/189)
  tracks destructive saved-password refresh; fresh migration targets preserve
  existing credentials. [#119](https://github.com/link-foundation/browser-commander/issues/119)
  separates passkey presence from exportability. No unsupported credential export
  is attempted; unreadable/provider state remains unknown.

Alternatives considered: treating every Gradle/CMake file as a package produces
false identities; executing project builds during scanning is unnecessary and
unsafe; probing only latest misses unpublished manifest versions; PackageVerify
as a push gate rejects valid push-only policies; selecting a stored token first
violates the required default; broad retry/cleanup can restart unrelated failures
or remove an intentional fallback. The implementation uses existing repository
adapters and official components with explicit unknown states instead.

## External acceptance criteria still blocked

These are unfinished requirements, not follow-up scope or completed work.

1. [Data.Doublets #538](https://github.com/linksplatform/Data.Doublets/issues/538)
   remains open. The active LinksPlatform policy and successful OIDC exchange are
   existing evidence; a complete corrected **main** run must still pass C# Ubuntu
   and Windows, GCC/Clang/sanitizer C++, tests/coverage/benchmarks/static analysis,
   FFI/package smoke, API/Unicode PDF/DocFX/Doxygen docs, all security checks,
   metadata/release permissions, publication, mirrors/assets/docs deployment and
   final required gate. Verify exact NuGet versions/downloads and consumers and
   record the run/SHA/method in that issue. Latest main run
   [40928285351](https://github.com/linksplatform/Data.Doublets/actions/runs/40928285351)
   at `c1a96a1aeb577e57d3e0b459bcf26e5fcc5e3efb` (created 18:10:27 UTC)
   passes validation jobs but fails publication and the required gate.
   Its failed log lines 10–20 prepare C# 0.18.4 and C++ 0.1.1, then report
   `GH006: Protected branch update failed` and `Repository rules reject the
   release commit. Allow github-actions to push release metadata; do not
   force-push.` Publication/deployment and consumer acceptance remain unfinished.
   This PR does not change another repository's protected-branch rules.

2. [C# template #80](https://github.com/link-foundation/csharp-ai-driven-development-pipeline-template/issues/80)
   remains open. Its `MyPackage` / `Your Name` identity requires an owner decision.
   No broad or unrelated-package policy is appropriate. Both automatic and
   instant/manual release workflows, preflight/auth/push callers, generated repo
   docs/config and fork report checks need the same contract, followed by full
   updated-main builds/tests/docs/release, exact public install and GitHub release
   verification. Existing NUGET_USER readback cannot satisfy those criteria.
3. [gh-manager #29](https://github.com/link-foundation/gh-manager/issues/29)
   is closed via #30, and its merged APIs are consumed here. This resolves the
   dependency capability blocker; it does not resolve either downstream release.
4. No authenticated live NuGet Gallery profile is available in this workspace.
   Policy logic has deterministic DOM/readback tests; a real account's activation,
   live push and exact consumer results must remain unverified until executed.
5. Browser-commander does not expose complete provider passkey-presence metadata
   or safe saved-password merge. Fresh targets address preservation; unknown
   presence is reported explicitly, not interpreted as no passkey.

## Historical public audit input (#71)

The following tables preserve the complete public unresolved inventory from the
2026-10-25 audit. Counts are historical: 102 reviewed targets, 49 absent names,
14 absent manifest versions, 38 present versions, one Go pseudo-version. They
are not new network observations. Private repository identities are excluded.

| Project / source manifest | Registry | Package and expected version | Plan prerequisite |
| --- | --- | --- | --- |
| [deep](https://github.com/link-foundation/deep/blob/main/package.json) | npm | `@deep-foundation/deep@7.0.0-alpha.8` | Publishing workflow proposal required |
| [cw-deep](https://github.com/link-foundation/cw-deep/blob/main/contracts/cw-deeplinks/Cargo.toml) | crates-io | `cw-deeplinks@0.0.1` | Publishing workflow proposal required |
| [bridge](https://github.com/link-foundation/bridge/blob/main/package.json) | npm | `@link-foundation/bridge@0.1.0` | Publishing workflow proposal required |
| [relative-meta-logic](https://github.com/link-foundation/relative-meta-logic/blob/main/js/package.json) | npm | `relative-meta-logic@0.19.0` | Resolve identity conflict (#69) |
| [relative-meta-logic](https://github.com/link-foundation/relative-meta-logic/blob/main/vscode/package.json) | npm | `relative-meta-logic@0.1.0` | Resolve identity conflict (#69) |
| [relative-meta-logic](https://github.com/link-foundation/relative-meta-logic/blob/main/rust/Cargo.toml) | crates-io | `relative-meta-logic@0.19.0` | Publishing workflow proposal required |
| [meta-theory](https://github.com/link-foundation/meta-theory/blob/main/package.json) | npm | `meta-theory-experiments@1.1.0` | Publishing workflow proposal required |
| [links-client](https://github.com/link-foundation/links-client/blob/main/rust/Cargo.toml) | crates-io | `links-client@1.0.0` | Publishing workflow proposal required |
| [links-client](https://github.com/link-foundation/links-client/blob/main/python/pyproject.toml) | pypi | `links-client@1.0.0` | Setup steps exist; validate artifact and authenticate |
| [ideav-local-associative-js](https://github.com/link-foundation/ideav-local-associative-js/blob/main/package.json) | npm | `ideav-local-associative-js@0.3.0` | Setup steps exist; validate artifact and authenticate |
| [gh-uncommitted](https://github.com/link-foundation/gh-uncommitted/blob/main/package.json) | npm | `gh-uncommitted@0.1.0` | Publishing workflow proposal required |
| [meta-notation](https://github.com/link-foundation/meta-notation/blob/main/js/package.json) | npm | `meta-notation@0.1.0` | Setup steps exist; validate artifact and authenticate |
| [meta-notation](https://github.com/link-foundation/meta-notation/blob/main/rust/Cargo.toml) | crates-io | `meta-notation@0.1.0` | Setup steps exist; validate artifact and authenticate |
| [lino-objects-codec](https://github.com/link-foundation/lino-objects-codec/blob/main/python/pyproject.toml) | pypi | `lino-objects-codec@0.3.1` | Setup steps exist; validate artifact and authenticate |
| [browser-commander](https://github.com/link-foundation/browser-commander/blob/main/python/pyproject.toml) | pypi | `browser-commander@0.5.3` | Setup steps exist; validate artifact and authenticate |
| [js-ai-driven-development-pipeline-template](https://github.com/link-foundation/js-ai-driven-development-pipeline-template/blob/main/package.json) | npm | `@link-foundation/example-package-name@0.11.31` | Resolve identity conflict (#69) |
| [transformer](https://github.com/link-foundation/transformer/blob/main/Cargo.toml) | crates-io | `lino-transformer@0.1.0` | Publishing workflow proposal required |
| [grammar-expressions](https://github.com/link-foundation/grammar-expressions/blob/main/js/package.json) | npm | `grammar-expressions@0.1.0` | Publishing workflow proposal required |
| [grammar-expressions](https://github.com/link-foundation/grammar-expressions/blob/main/rust/Cargo.toml) | crates-io | `grammar-expressions@0.1.0` | Publishing workflow proposal required |
| [lino-rest-api](https://github.com/link-foundation/lino-rest-api/blob/main/python/pyproject.toml) | pypi | `lino-rest-api@0.2.2` | Setup steps exist; validate artifact and authenticate |
| [gh-download-gist](https://github.com/link-foundation/gh-download-gist/blob/main/package.json) | npm | `gh-download-gist@0.2.0` | Setup steps exist; validate artifact and authenticate |
| [lino-cache](https://github.com/link-foundation/lino-cache/blob/main/package.json) | npm | `lino-cache@0.2.0` | Setup steps exist; validate artifact and authenticate |
| [links-queue](https://github.com/link-foundation/links-queue/blob/main/js/package.json) | npm | `links-queue-js@0.15.1` | Setup steps exist; validate artifact and authenticate |
| [links-queue](https://github.com/link-foundation/links-queue/blob/main/js/packages/express/package.json) | npm | `links-queue-express@0.1.0` | Publishing workflow proposal required |
| [links-queue](https://github.com/link-foundation/links-queue/blob/main/js/packages/fastify/package.json) | npm | `links-queue-fastify@0.1.0` | Publishing workflow proposal required |
| [links-queue](https://github.com/link-foundation/links-queue/blob/main/js/packages/hono/package.json) | npm | `links-queue-hono@0.1.0` | Publishing workflow proposal required |
| [links-queue](https://github.com/link-foundation/links-queue/blob/main/js/packages/nestjs/package.json) | npm | `links-queue-nestjs@0.1.0` | Publishing workflow proposal required |
| [links-queue](https://github.com/link-foundation/links-queue/blob/main/rust/Cargo.toml) | crates-io | `links-queue@0.3.0` | Setup steps exist; validate artifact and authenticate |
| [rust-ai-driven-development-pipeline-template](https://github.com/link-foundation/rust-ai-driven-development-pipeline-template/blob/main/Cargo.toml) | crates-io | `example-sum-package-name@0.20.2` | Resolve identity conflict (#69) |
| [java-ai-driven-development-pipeline-template](https://github.com/link-foundation/java-ai-driven-development-pipeline-template/blob/main/pom.xml) | maven-central | `com.linkfoundation:my-package@0.2.0` | Choose/configure publishing workflow |
| [gh-upload-image](https://github.com/link-foundation/gh-upload-image/blob/main/package.json) | npm | `gh-upload-image@0.2.0` | Setup steps exist; validate artifact and authenticate |
| [box](https://github.com/link-foundation/box/blob/main/Dockerfile) | docker-hub | `link-foundation/box` | Confirm image namespace; configure CI |
| [pdf-to-markdown](https://github.com/link-foundation/pdf-to-markdown/blob/main/package.json) | npm | `pd2md@0.6.0` | Setup steps exist; validate artifact and authenticate |
| [set-process-name](https://github.com/link-foundation/set-process-name/blob/main/package.json) | npm | `set-process-name@0.6.0` | Setup steps exist; validate artifact and authenticate |
| [wiki-referencify](https://github.com/link-foundation/wiki-referencify/blob/main/package.json) | npm | `wiki-referencify@1.0.0` | Setup steps exist; validate artifact and authenticate |
| [meta-ontology](https://github.com/link-foundation/meta-ontology/blob/main/Cargo.toml) | crates-io | `meta-ontology@0.9.0` | Setup steps exist; validate artifact and authenticate |
| [rust-web-box](https://github.com/link-foundation/rust-web-box/blob/main/web/extensions/rust-analyzer-web/package.json) | npm | `rust-analyzer-web@0.1.0` | Publishing workflow proposal required |
| [rust-web-box](https://github.com/link-foundation/rust-web-box/blob/main/web/extensions/webvm-host/package.json) | npm | `webvm-host@0.1.0` | Publishing workflow proposal required |
| [rust-web-box](https://github.com/link-foundation/rust-web-box/blob/main/Cargo.toml) | crates-io | `example-sum-package-name@0.19.0` | Resolve identity conflict (#69) |
| [code-changes-collector](https://github.com/link-foundation/code-changes-collector/blob/main/Cargo.toml) | crates-io | `example-sum-package-name@0.7.0` | Resolve identity conflict (#69) |
| [grammar-inference](https://github.com/link-foundation/grammar-inference/blob/main/Cargo.toml) | crates-io | `example-sum-package-name@0.7.0` | Resolve identity conflict (#69) |
| [rules-inference](https://github.com/link-foundation/rules-inference/blob/main/Cargo.toml) | crates-io | `example-sum-package-name@0.7.0` | Resolve identity conflict (#69) |
| [dedup-log](https://github.com/link-foundation/dedup-log/blob/main/package.json) | npm | `dedup-log@0.10.0` | Setup steps exist; validate artifact and authenticate |
| [linksql](https://github.com/link-foundation/linksql/blob/main/js/package.json) | npm | `@link-foundation/linksql@0.1.0` | Setup steps exist; validate artifact and authenticate |
| [linksql](https://github.com/link-foundation/linksql/blob/main/python/pyproject.toml) | pypi | `linksql@0.1.0` | Setup steps exist; validate artifact and authenticate |
| [php-ai-driven-development-pipeline-template](https://github.com/link-foundation/php-ai-driven-development-pipeline-template/blob/main/composer.json) | packagist | `link-foundation/example-package-name@0.2.1` | Setup steps exist; validate artifact and authenticate |
| [gh-manager](https://github.com/link-foundation/gh-manager/blob/main/package.json) | npm | `@link-foundation/gh-manager@1.2.0` | Setup steps exist; validate artifact and authenticate |
| [watermarks-remover](https://github.com/link-foundation/watermarks-remover/blob/main/package.json) | npm | `@link-foundation/watermarks-remover@0.11.31` | Setup steps exist; validate artifact and authenticate |
| [universal-translator](https://github.com/link-foundation/universal-translator/blob/main/package.json) | npm | `@link-foundation/example-package-name@0.11.31` | Resolve identity conflict (#69) |

| Project / source manifest | Registry | Expected package version | Observed latest | Review note |
| --- | --- | --- | --- | --- |
| [si-units](https://github.com/link-foundation/si-units/blob/main/si-units/package.json) | npm | `si-units@1.0.0` | 0.2.3 | Exact manifest version absent |
| [deduplino](https://github.com/link-foundation/deduplino/blob/main/package.json) | npm | `deduplino@0.1.0` | 0.0.9 | Exact manifest version absent |
| [log-lazy](https://github.com/link-foundation/log-lazy/blob/main/js/package.json) | npm | `log-lazy@1.1.0` | 1.0.4 | Exact manifest version absent |
| [lino-env](https://github.com/link-foundation/lino-env/blob/main/js/package.json) | npm | `lino-env@1.0.1` | 0.2.8 | Exact manifest version absent |
| [gh-load-issue](https://github.com/link-foundation/gh-load-issue/blob/main/package.json) | npm | `gh-load-issue@0.4.0` | 0.3.2 | Exact manifest version absent |
| [links-client](https://github.com/link-foundation/links-client/blob/main/js/package.json) | npm | `@link-foundation/links-client@1.2.0` | 1.0.0 | Exact manifest version absent |
| [gh-tree](https://github.com/link-foundation/gh-tree/blob/main/package.json) | npm | `gh-tree@0.0.1` | 0.4.1 | Manifest older than latest; review source metadata |
| [lino-rest-api](https://github.com/link-foundation/lino-rest-api/blob/main/js/package.json) | npm | `lino-rest-api@0.2.2` | 0.2.1 | Exact manifest version absent |
| [python-ai-driven-development-pipeline-template](https://github.com/link-foundation/python-ai-driven-development-pipeline-template/blob/main/pyproject.toml) | pypi | `my-package@0.1.0` | 0.0.0 | Exact manifest version absent |
| [csharp-ai-driven-development-pipeline-template](https://github.com/link-foundation/csharp-ai-driven-development-pipeline-template/blob/main/src/MyPackage/MyPackage.csproj) | nuget | `MyPackage@0.3.16` | Other versions exist | Exact manifest version absent |
| [anygram](https://github.com/link-foundation/anygram/blob/main/package.json) | npm | `my-package@0.5.0` | 0.0.0 | Shared placeholder identity (#69) |
| [lino-schema](https://github.com/link-foundation/lino-schema/blob/main/package.json) | npm | `my-package@0.6.0` | 0.0.0 | Shared placeholder identity (#69) |
| [lino-document-markup-language](https://github.com/link-foundation/lino-document-markup-language/blob/main/package.json) | npm | `my-package@0.7.3` | 0.0.0 | Shared placeholder identity (#69) |
| [meta-language](https://github.com/link-foundation/meta-language/blob/main/js/package.json) | npm | `meta-language@1.0.0` | 0.46.0 | Exact manifest version absent |

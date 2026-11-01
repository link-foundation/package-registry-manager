# Issue 75: links-notation integration gaps — requirements and evidence

Review date: 2026-11-01. Scope: issue #75 (no comments at review time), PR #76
(no comments or reviews at review time), and
[links-notation](https://github.com/link-foundation/links-notation) main
`06ee7ea5` as the reproduction source. Modules named below exist in both
`js/src` and `rust/src`. Deterministic tests use
`tests/fixtures/links-notation`, a copy of that repository's manifests,
version files and workflows. They never touch live registries or settings.

Reproduce the original report with
`node experiments/issue-75/reproduce.mjs <links-notation checkout>`. Print the
compact identity summary with `node experiments/issue-75/summary.mjs <dir>`.

## Requirement inventory

| Requirement | Implementation | Verification |
| --- | --- | --- |
| Nested `js.yml -> release.yml -> publish-*.yml` callers are found | `publishers` follows local reusable calls, keeps the outer caller as the OIDC identity and records `workflow_callees` chains | `links-notation-fixture` summary: every target maps to `js.yml`/`release` plus its exact callee |
| Rust shell function and `./mvnw -B deploy` | `source-code` accepts commands after `if/then/else/do/...`; the Maven pattern accepts `./mvnw` with flags | crates (both crates) and Maven map to `publish-rust`/`publish-java` |
| Package scoping keeps reachable callers | `package-coverage` keeps reusable caller jobs, matches ancestor-directory steps only when they name the package or manifest, and keeps action-only steps (`crates-io-auth-action`) | fixture contract |
| NuGet `VersionPrefix` | `discovery` reads `PackageVersion`/`Version`/`VersionPrefix[-VersionSuffix]` and ignores MSBuild expressions | NuGet `0.27.1` |
| C++ `cpp/VERSION` read with CMake `file(READ)` | `cpp-distribution` resolves `project(... VERSION ${VAR})` from a sibling `file(READ ...)` or `set(VAR ...)`. CMake is never executed | cmake `links_notation` `0.27.1` with `version_source` |
| Go `go/VERSION` | `go.mod` versions come from a sibling `VERSION` file (`v`-prefixed) | Go `v0.27.1` |
| Root vs `php/composer.json` | Root `composer.json` is canonical. The subdirectory copy is skipped and its version source and scope are inherited | single Packagist target |
| Internal benchmark workloads | A `bench`/`benchmark`/`perf` manifest with no publishing workflow is skipped unless listed in `publish`. The benchmark crate stays `publish = false` | no benchmark is publishable |
| No false "add publishing job" | Tag, hook and release-asset registries (Go, Packagist, CMake) are scoped by release evidence, and the warning is suppressed for them | `no false add-publishing-job request` test |
| NuGet multiline guarded push | `nuget-migration` recognizes only the guard, `${A:-$B}` key selection, presence check, notices, `published=` outputs and one nuget.org push. It emits OIDC login and the controller push of the same artifacts with the `NUGET_API_KEY \|\| NUGET_TOKEN` fallback, then re-emits the original `id: publish` outputs. Pack is not repeated. Any other command raises `nuget-prerequisite-edit` with the exact edit | `nuget-guarded-migration` and fixture tests: pack kept once, guard kept, fallback kept, `id-token: write` in all three workflows |
| NuGet CI verifier applicable jobs | `nuget-ci` `requiredJobs`/`unmetJobs`: failures always block. Skips block only for required release jobs, or when the required set is unknown | `nuget-ci` test with dependency-review/benchmark skips |
| Maven pair, namespace, signing | `credential-requirements` resolves `CENTRAL_USERNAME`+`CENTRAL_TOKEN`, `GPG_PRIVATE_KEY`+`GPG_PASSPHRASE` from the workflows. The plan has `verify-namespace` (with groupId), `ensure-central-credential-pair` and `ensure-signing-key`, with `oidc: unsupported`. Values stay in memory or encrypted GitHub secrets, and nothing is rotated or revoked | fixture plan test |
| Packagist read-back, stable tag, hook, update API | `publication-state` records the exact `stable_tag` from `p2` (stable-only) metadata. The plan has `verify-packagist-package`, `packagist-github-hook` (`manual-prerequisite`) and `packagist-update-api` (`configured` or `unsupported-without-credentials`). It makes no OIDC claim and keeps Private Packagist distinct | fixture plan test |
| Go exact proxy version | `publication-state` reads `@v/<version>.info` | publication-state test |
| Stale "NuGet has no OIDC" | NuGet stays `oidc-with-token-fallback` in `credential-cycle`. The migration replaces the stale comment block's push with NuGet/login | fixture migration test |
| PyPI always-token expression | `authenticationFacts` detects `&& '' \|\| secrets.X` password selection. The policy is `token-only` with a warning that the flag is ineffective | PyPI `token-only` in the fixture |
| Variable-gated crates OIDC | New policy `variable-gated-oidc-with-token-fallback` with `authentication_gate`. A migrate plan asks to register trust and flip the variable, not rewrite the workflow | fixture plan test |
| No rotation, deletion or republication | The new steps only ensure missing secrets. Migration controllers probe the exact version before pushing | tests assert no rotate/revoke steps |

## Not verifiable from this repository

The issue's last completion item needs live access to the downstream
links-notation repository, registry accounts and publisher settings:

- exact registry versions after each repair
- GitHub releases and assets
- final required CI
- meaningful consumers

This covers creating the Maven namespace and credentials, Packagist
registration, the NuGet policy, and merging the generated workflow change
downstream (tracked in links-notation#192). The manager now produces the
reviewable change and the structured prerequisites. Executing them belongs to
the downstream setup run and is not claimed here.

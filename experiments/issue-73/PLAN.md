# Issue 73 implementation and verification plan

Objective: implement all requirements of #65–#72 in PR #74, with both Rust and
JavaScript behavior covered. Never equate plans or configuration with publication.

## Inventory and implemented tool behavior

Checked items below describe local implementation and deterministic regression
coverage. They do not mark the external publication acceptance criteria complete.

- [x] Read parent, eight issues, every comment, PR conversation, reviews and inline comments.
- [x] Read related downstream issues (#538 Data.Doublets, #80 C# template, #29 gh-manager), #58 and recent merged PRs; inventory all additional acceptance criteria.
- [x] Research official APIs/libraries; record links, alternatives, limitations and decisions.
- [x] #65: classify owner-setup/bootstrap preflight separately from OIDC; extract exact missing names, revalidate existence/publishers and present preflight before bounded failed-job retry; keep unrelated failures distinct; include diagnostic in release scan evidence.
- [x] #66: expand generated npm publishAliases with source/version/generator/workflow metadata; independent selection, state, trust and post-release verification; validate actual generated artifact before bootstrap, refuse assumed renamed tarballs.
- [x] #67: require real Gradle coordinates and publication/component; unresolved/application/build-only definitions skipped with reasons, never fabricated publishable identities; exclude unpublished .NET examples without excluding explicitly published executable tools; exclude dev/log snapshots across all ecosystems unless explicitly opted in.
- [x] #68: retain expected/latest/exact-version state and URL; distinguish absent version from source older than latest, failures from absence; Maven/Packagist/Go read-only coverage with pseudo-version/tag distinction; unsupported registries explicitly unknown; resolve Cargo workspace inherited versions, preserve unresolved versions.
- [x] #69: normalize/group all registry identities (including aliases) before mutation across repositories and within a repository; report every source/manifest/version; block until distinct names or reviewed shared-artifact mapping; expose public repository/owner evidence without asserting permission; exclude copied snapshots.
- [x] #70: discover CMake, Conan and vcpkg, shared versions, skip build/test/example files; identify workflow channels and remote requirements; no public-presence inference; concrete validation/CI/credential plans for supported channels and explicit unknown/unsupported for others, use gh-manager metadata.
- [x] #71: incorporate #58 integration/log expiry/anonymous crates settings fixes; cover complete audit capability gaps and public unresolved target list; preserve identity decisions and publication safety, exact npm checks despite unpublish root documents; identify external project decisions and live evidence honestly.
- [x] #72 policy: NuGet/login OIDC first; provided/custom token only after unavailable/rejected OIDC; invalid fallback ignored on OIDC success; no new persistent credential; structured methods/reasons/authentication vs authorization vs publication; same artifact bounded auth retry; unrelated/transport/probe failures distinct.
- [x] #72 policy setup: login profile vs package owner vs GitHub IDs/workflow/environment/exact IDs/operations; reject placeholders/ambiguity; discover equivalent policy before create, fresh readback and activation state; browser-commander supported fallback, never dotnet signer trust; gh-manager effective username variable and fallback preservation.
- [x] #72 all surfaces: update shared policy, plans, workflow proposals/migration, cleanup and credential checks in both ports; preserve repository and shared org fallback secrets; temporary OIDC keys job-local only; pending code changes remain pending.
- [x] #72 browser: inspect/deduplicate every local profile, candidate session identity verification then saved-login fallback; selected-route nuget/Microsoft personal/GitHub domains; preserve successful/target credentials, fresh target for unsafe password reimports; passkey presence separate from export support, unknown/unreadable/unsupported explicit; sign-in interruption.
- [x] #72 diagnostics: realistic PackageVerify HTTP 400 and derived verification-key limitations, never claim scope authorization or rotate/widen from incompatible probe.
- [x] #72 recovery/evidence: settings-only repair affected failed CI only, code repair follow new main SHA without duplicate dispatch; report URL/attempt/SHA/jobs/steps/method; all required downstream gates plus exact public versions/download/consumer checks; placeholders and skipped stages explicitly incomplete.

## Work sequence

1. Inventory code paths and add minimal failing regressions before each behavior fix.
2. Implement discovery and package state in both ports, then identity preflight/recovery.
3. Implement NuGet policy/workflow/browser/cleanup and recovery contracts in both ports.
4. Run focused tests then all JS/Rust checks and script tests sequentially under 2.92 GiB. Rust CARGO_BUILD_JOBS=1 and debug info disabled. Preserve logs; no unbounded memory/stack experiments.
5. Add changelog fragment (release trigger; no direct version bump per contributing guide). Commit atomic validated changes and push only issue-73-a033a43fcd85.
6. Merge current main if needed. Review gh pr diff for scope, regressions and secret material.
7. Update PR title/body with reproduction/tests/limitations and Fixes #65 through #73, one keyword each.
8. Inspect fresh CI timestamps/SHA, download each failed run log to ci-logs, diagnose with exact lines, fix and recheck. Finish background work, clean tree, mark ready only with accurate completion status; never merge.

## External completion evidence

Maintain explicit unresolved criteria where required external identity/account/main-revision
state cannot be established. Such criteria remain unfinished regardless of passing PR CI.


The exhaustive requirement-to-code/test matrix, official-source research and
historical public target lists are in [the requirement inventory](../../docs/issue-73-requirements.md).

- [ ] Data.Doublets #538: corrected protected-main full pipeline, publication,
  mirrors/assets/docs deployment, exact public versions and consumer evidence.
  Latest main run 40928285351 at c1a96a1aeb577e57d3e0b459bcf26e5fcc5e3efb
  passes validation but fails publish on GH006 protected-main rules; exact new
  versions and final required gate remain unfinished.
- [ ] C# template #80: owner-approved replacement for placeholder package identity,
  matching policy, both automatic/instant main releases and exact consumer proof.
- [ ] Authenticated live NuGet Gallery policy activation, actual push and consumer
  readback for these external repositories. No matching authenticated profile is
  available in this workspace.
- [x] gh-manager #29 capability dependency is merged and consumed in both ports.

Keep PR #74 draft and unmerged while these acceptance criteria remain unfinished.

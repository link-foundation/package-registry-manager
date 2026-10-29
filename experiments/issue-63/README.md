# Regression verification for issue 63

The smallest reproductions live in the normal test suites. They use injected
registry/GitHub responses and temporary repositories; they require no real
registry credentials and never publish a package.

| Requirement | Reproduction and verification |
| --- | --- |
| #58 public inspection | `js/test/scan-reliability.test.mjs` and `rust/tests/integration/scan_reliability.rs`: public lookup succeeds while owner-only publisher settings would refuse anonymous access. Exactly one public request preserves existence/provenance and an authenticated-verification reason. |
| #58 rate limiting | The same suites exercise 403/429, Retry-After, concurrent shared pacing and unknown HTTP evidence. Organization scan tests cover expired logs and special default branches; the JS account test verifies clean repositories still execute after another repository fails. |
| #59 quiet sessions | `js/test/quiet-crates-session.test.mjs` tests saved-session versus interactive escalation. Browser parity smoke tests launch real Chrome, verify saved cookies across a headed/headless restart, and exercise the Rust headless launcher. Existing batch tests verify shared browser lifetime. |
| #60 workflow shapes | `js/test/workflow-migration.test.mjs` and `rust/tests/unit/workflow_migration.rs` cover inline tokens, substitutions, executed string-built commands, preflights, distinct guards, inherited environment/secrets, mixed auth and preserving already converted jobs. CLI tests verify all crates are selected without `--package`; batch tests verify shared proposals and per-package execution. |
| #62 policy and retention | Migration tests and the shared token-rotation golden files verify OIDC-first fallback, optional reusable inputs and inspection. Cleanup tests cover default retention, explicit deletion and publication/non-use proofs. Existing token-rotation tests cover CI evidence and organization/repository fallback. |

Run the regular checks described in `CONTRIBUTING.md`. On a memory-constrained
machine use `CARGO_BUILD_JOBS=1`, `CARGO_PROFILE_DEV_DEBUG=0` and
`CARGO_PROFILE_TEST_DEBUG=0`. Browser checks need Chrome and a display:

```sh
PRM_BROWSER_SMOKE=1 xvfb-run -a node --test js/test/browser-parity.test.mjs js/test/publisher-dom.test.mjs
PRM_BROWSER_SMOKE=1 xvfb-run -a cargo test --manifest-path rust/Cargo.toml --test integration browser_
```

The first focused runs reproduced the old failures before implementation:
public inspection made an extra private request; unknown crate responses lost
HTTP evidence; saved sessions requested a headed browser; cleanup deleted
fallback credentials; inline-token/preflight migration rejected supported
workflows. Additional review regressions reproduced duplicate npm fallback
steps and variable-gated OIDC being incorrectly reported as complete.

The inherited failing CI ran at `c0847d5` on 2026-10-23. The upstream dependency
log identified stale browser-commander versions at lines 177 and 179. The
security log identified six TLS dependency advisories at lines 3815–3869.
Merging current main supplies the browser update; this change updates
command-stream in both languages and patches rustls, rustls-webpki and
webpki-root-certs. Local diagnostic logs are kept in the ignored `ci-logs/`
directory.

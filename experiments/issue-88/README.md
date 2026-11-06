# Issue 88 work plan

Requirements and verification checklist:

- [x] Recover stale OAuth using fresh same-profile login; reuse approved grants;
      identify authentication, registration and unapproved grant boundaries.
- [x] Validate root Composer descriptor; discover before submission; verify exact
      repository; never rename or duplicate an existing package.
- [x] Read Safe token in memory; main token only for package creation; redact
      errors and never put credentials in URLs, argv or reports.
- [x] Encrypt selected update secrets through installed gh-manager; read presence
      back and independently test authenticated acceptance.
- [x] Inspect/create/repair only Packagist hooks; verify active push JSON settings;
      request test delivery and inspect its fresh result.
- [x] Retry only causal publisher/dependent jobs after configuration verification.
      Report registration, queue acceptance, exact version/source, Composer
      consumer and GitHub release as separate evidence.
- [x] Cover both runtimes, including installed gh-manager encryption/transport,
      missing/existing accounts/packages, stale OAuth, bad hooks, API rejection,
      storage failure, transport uncertainty, delayed indexing and SHA mismatch.
- [x] Add release fragment, run focused checks and review the diff. Track final
      main ancestry, clean branch, current-commit CI and ready state on PR 89.

No required sub-issues or feedback existed when the issue and all PR comment
endpoints were read on 2026-11-06. Packagist's documented public VCS ingestion
is used; no Private Packagist artifact publishing is introduced.

Verification evidence:

- 66 focused tests pass in `ci-logs/packagist-test.log`; the hook regression failed
  before the fix (`ci-logs/issue-88-before.log`).
- `npm run check`, Rust library check and Rust library Clippy pass locally.
- Initial CI exposed a Node 22 file timeout (manager readback waits) and Windows
  npm shim resolution. Tests now use the installed manager's zero-wait fixture
  option; production retains polling. The native loader handles npm shims, and
  test programs use temporary source files to respect Windows argv limits.
- Cross-platform/full validation is tracked by the checks on
  <https://github.com/link-foundation/package-registry-manager/pull/89>.

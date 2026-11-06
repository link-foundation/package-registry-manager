# Issue 88 work plan

Requirements and verification checklist:

- [ ] Recover stale OAuth using fresh same-profile login; reuse approved grants;
      identify authentication, registration and unapproved grant boundaries.
- [ ] Validate root Composer descriptor; discover before submission; verify exact
      repository; never rename or duplicate an existing package.
- [ ] Read Safe token in memory; main token only for package creation; redact
      errors and never put credentials in URLs, argv or reports.
- [ ] Encrypt selected update secrets through installed gh-manager; read presence
      back and independently test authenticated acceptance.
- [ ] Inspect/create/repair only Packagist hooks; verify active push JSON settings;
      request test delivery and inspect its fresh result.
- [ ] Retry only causal publisher/dependent jobs after configuration verification.
      Report registration, queue acceptance, exact version/source, Composer
      consumer and GitHub release as separate evidence.
- [ ] Cover both runtimes, including installed gh-manager encryption/transport,
      missing/existing accounts/packages, stale OAuth, bad hooks, API rejection,
      storage failure, transport uncertainty, delayed indexing and SHA mismatch.
- [ ] Release fragment, focused checks, final diff review, clean branch, updated
      PR 89, current main ancestry and passing current-commit CI; mark ready.

No required sub-issues or feedback existed when the issue and all PR comment
endpoints were read on 2026-11-06. Packagist's documented public VCS ingestion
is used; no Private Packagist artifact publishing is introduced.

---
bump: minor
---

### Added
- Explicit legacy crates.io token rotation with preserved scopes, organization-first secret storage, expiry companions, CI verification before revocation, and migration target reporting in both CLIs.
- Private crates.io token expiry warnings during account scans with `--token-expiry-days`.
- `setup --migrate` for crates.io, npm and PyPI: reviewed conversions of existing publishing workflows and reusable callers, repository changelog fragments, and legacy secret cleanup after a verified OIDC upload. Organization secrets remain until a fresh complete scan finds no remaining consumers.

### Fixed
- Detect scripted Cargo publishing with variable executables and multiline argument arrays through local reusable workflow callers, including account scan snapshots.
- Preserve the publishing callee's environment while registering the top-level caller workflow, including nested reusable calls.
- Preserve multiline Cargo publishing conditions in authentication steps and stop conversions when publishing steps require different authentication conditions.
- Include token readers without publishable manifests in shared-secret CI verification, and group explicit custom rotation secret names consistently in both CLIs.

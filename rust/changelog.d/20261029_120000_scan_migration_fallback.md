---
bump: minor
---

### Added
- Default OIDC-first publishing with token fallback for crates.io, npm and PyPI, optional reusable secrets, policy inspection and explicit `--delete-token-secrets` cleanup.

### Fixed
- Pace public crates.io scans and retry rate-limited registry/GitHub reads while retaining unknown HTTP evidence and separating private publisher settings. Update gh-manager to handle expired CI logs; continue migration for clean repositories in partial account scans.
- Reuse saved crates.io sessions headlessly across batch API tasks, escalating to a visible browser for sign-in or approval.
- Convert inline Cargo tokens, command substitutions, string-built publishing commands, preflights, inherited reusable secrets, mixed authentication and multi-crate jobs while preserving release guards and explaining preview/execution source differences.
- Update command-stream and TLS dependencies to resolve stale dependency and security checks.

### Added

- Inspect and plan publishing across an organization or user through gh-manager's repository/file APIs; report unpublished packages, packages without trusted publishing, publishers pointing at another repository, and release failures with CI evidence.
- Set up account findings together, sharing browser sessions and npm approvals. Registry token setup now uses gh-manager CI health and test evidence, organization secrets with repository fallback, and custom secret naming templates.

### Fixed

- Resolve gh-manager from a pinned source release while its scoped npm package remains unpublished, rather than requiring an unavailable executable on PATH.

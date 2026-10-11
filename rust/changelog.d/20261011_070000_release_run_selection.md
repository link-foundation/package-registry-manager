---
bump: patch
---

### Fixed
- Select failed default-branch push or workflow_dispatch releases with failed publish/preflight jobs during trusted-publisher setup and repository repair, excluding newer pull request and lint runs. Classify OIDC preflight rejections and show the selected run URL before retrying its failed jobs (#44).

### Changed
- Require current browser-commander and command-stream releases to satisfy the upstream dependency checks.

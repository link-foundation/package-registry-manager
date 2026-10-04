---
bump: patch
---

### Fixed
- The npm setup relays `npm trust list`'s 2FA approval link: the trust checks ran it with `--json`, which makes npm hold the link back until it exits, so the approval expired and "Verify trusted publishing" failed with E404 although `npm trust github` had just created the publisher. `npm trust list` now runs with `--browser=false` and its readable output is parsed, and npm's `Trust configuration created successfully` answer counts as the verification, so no second approval is needed (#24)
- Signing npm out and removing the temporary worktree run even when an earlier setup step fails; before, a failing step skipped every cleanup step after it, leaving the npm session token and a stale worktree behind (#24)

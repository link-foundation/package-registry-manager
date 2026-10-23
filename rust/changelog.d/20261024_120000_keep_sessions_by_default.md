---
bump: minor
---

### Changed
- `setup` stays signed in by default: the npm session token and the crates.io
  sign-in in the automated profile are kept, so the next run (npm: within two
  hours) skips the sign-in. `--sign-out` signs out at the end instead;
  `--keep-session` is still accepted and is now the default.

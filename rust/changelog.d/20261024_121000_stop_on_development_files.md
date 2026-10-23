---
bump: minor
---

### Added
- An npm first publish stops before the sign-in when the packed tarball
  contains repository development directories such as `.github/` or `tests/`,
  which a package.json without a `files` allowlist lets in.

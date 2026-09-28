---
bump: minor
---

### Fixed
- `plan` no longer emits setup steps or a trusted publisher for unpublishable packages; they appear with `"steps": []` and a `skipped_reason`, and `setup` picks the only publishable package when a registry also has private ones (#4)

---
bump: patch
---

### Fixed
- The release workflow no longer fails after a successful `npm publish` because the new version was not visible yet: every `npm view` lookup uses `--prefer-online` instead of npm's cached pre-publish packument, the wait grows to about 11 minutes with backoff, and if the version is still not visible the job ends with a warning and `published=true`. The crates.io wait sends `Cache-Control: no-cache` with each probe (#28)

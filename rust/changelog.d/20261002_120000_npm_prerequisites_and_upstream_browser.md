---
bump: minor
---

### Added
- `plan` and `setup` list the prerequisites before the steps: the Node.js and npm versions, the npm that runs `npm trust` and the version it resolves to, the npm account's 2FA state, `gh` sign-in and scopes, and where browser pages open (#13)
- npm bootstrap checks account-level 2FA with `npm profile get --json` after signing in, opens https://www.npmjs.com/settings/~/tfa when it is off, and stops before publishing until it is on (#13)
- npm attach reads the latest release run; when npm rejected it with E404 or `invalid-publisher`, setup says so and, once trust is attached, re-runs its failed jobs with `gh run rerun <id> --failed`, which `--verify-release` then watches instead of dispatching a new run (#13)
- The packed tarball is installed into a scratch directory without lifecycle scripts, its bin entries are compared with package.json, and each installed bin is run with `--version`; npm's pack warnings are reported, without ever suggesting or running `npm pkg fix` (#13)

### Changed
- `npm trust` runs through `npx -y npm@^11.10`, or `npm@^12` when the local Node.js satisfies npm 12's engines (`^22.22.2 || ^24.15.0 || >=26.0.0`), instead of `npm@latest`, which fails on older Node.js (#13)

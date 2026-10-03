---
bump: minor
---

### Added
- npm sign-in and approval links print their deadline ("Sign in within about 5 minutes (until 19:05)."); when npm's web login expires, setup asks for a fresh link up to 3 times instead of failing (#17)
- Setup names the default browser it opens links in, `--open-with <app>` opens them in another browser application, and `--keep-session` keeps the npm sign-in after a first publish so the next package needs no new sign-in (#17)
- After the first publish, setup says that future releases publish from the trusted workflow without a login, and before approvals it suggests npm's "skip two-factor checks for 5 minutes" option so the approvals that follow need no new confirmation (#17)
- Every plan for a repository whose workflow deploys with `actions/deploy-pages` checks the Pages site with `gh api repos/{owner}/{repo}/pages` and, after a confirmation, enables it (`POST`) or switches it (`PUT`) to GitHub Actions as its source; without administrator rights it prints the settings link instead of failing (#16)
- `--workflow <file>` and `--environment <name>` override the detected trusted publisher, and `plan --package <name>` plans only that package in both CLIs (#16)
- Manifests in `tests`, `test`, `fixtures`, `__fixtures__`, `__tests__`, and `examples` directories are skipped unless a workflow mentions them, more paths can be skipped through an `ignore` list in `.package-registry-manager.json`, and `--verbose` lists the skipped manifests with their reasons (#16)
- Setup lists every registry's leftover token secrets (`NPM_TOKEN`, `NPM_AUTH_TOKEN`, `CARGO_TOKEN`, `CARGO_REGISTRY_TOKEN`, `CRATES_IO_TOKEN`, `CRATES_TOKEN`) and deletes each unused one after a confirmation; after the crates.io first publish it checks with `GET /api/v1/me/tokens` that the one-time token was revoked (#16)

### Changed
- The trusted-publisher workflow is the one whose job actually runs `npm publish` (directly or through a package script), not a workflow that merely mentions npm (#16)
- npm bootstrap packs and checks the bins first and signs in right before publishing, so the 5-minute sign-in window is not spent on local checks (#17)
- A package that already publishes through trusted publishing still runs the repository checks instead of stopping early (#16)
- The release workflow mints the crates.io token with `rust-lang/crates-io-auth-action` (OIDC), no longer reads `CARGO_TOKEN`, `CARGO_REGISTRY_TOKEN`, or `NPM_TOKEN`, and skips each registry separately when its package is not bootstrapped yet (#16)

### Fixed
- Self-publishing no longer picks the wrong trusted-publisher workflow or plans test fixtures as packages, and the documentation deployment explains how to enable GitHub Pages instead of failing with "Get Pages site failed ... Not Found" (#16)
- The JavaScript CLI finds the publishing job when the repository is reached through a symlinked path (macOS's `/var`, Windows short names), and names the macOS default browser instead of printing none (#16, #17)

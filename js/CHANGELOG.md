# Changelog

All notable changes to the JavaScript package are documented here. The package
currently shares its release version with the Rust implementation.

## [Unreleased]

### Added

- `status` reports the newest release workflow run (including tag-dispatched
  publishes), its active and failed jobs, the release version, and each
  package's expected and latest registry version. It supports `--run`, `--wait`,
  `--require-published`, `--no-release`, JSON output, and account scans, and
  recognizes crates.io's default-on OIDC exchange with an opt-out token
  fallback.
- PyPI setup submits a prefilled sign-in form automatically, retries once after
  an invalid login or an expired two-factor login, never reads the credential
  values, and tells the user when passkey/2FA or unrecognized-device email
  confirmation is required. A 2FA page gets "Remember this device for 30 days"
  checked automatically, including immediately after the initial sign-in, and
  the confirmation page is detected in the current tab or another tab of the
  automated browser.
- Automated Chromium launches suppress Chrome product-promo tabs and disable
  split-view drag-and-drop by default, so browser tips do not cover the slot
  where sign-in and approval pages open.
- npm web login, crates.io GitHub login, NuGet, credential pages and repository
  repair submit a prefilled username/password form automatically. Autofilled
  values trigger the input events sites need to enable their submit button, so
  a prefilled npm "Sign In" button no longer waits for a manual click.
- A failed `gh workflow run` re-reads the workflow YAML and retries with
  declared `workflow_dispatch` defaults or the first choice option, so required
  inputs such as `bump_type` no longer stop a first release.
- The automated browser closes tabs restored from an earlier profile session
  and keeps only the working tab, so repeated runs no longer fill the window
  with `about:blank` pages. It matches the working page by identity and URL, so
  a Playwright wrapper is never mistaken for an extra tab. Extension mode closes
  the tab it opened.
- npm security-key approvals check the 5-minute cooldown box and click "Use
  security key" automatically; Touch ID or the physical key remains the only
  manual confirmation.
- Visible automated browsers start maximized on Chromium (including profiles
  that saved a small window) and Firefox, while headless launches keep their
  default size.
- `setup --registry npm --execute` takes a never-published package to OIDC
  trusted publishing in one run: web login, a confirmed first publish from a
  temporary worktree of the default branch, `npm trust github`, and cleanup of
  a leftover `NPM_TOKEN` secret. It never reads, creates, or asks for an npm
  token (#3).
- `inspect` and `plan` look packages up on npm, crates.io, PyPI, and Docker Hub
  and plan a `bootstrap`, `attach`, or `complete` mode. `--offline` skips the
  lookups, `--dry-run` prints the whole flow, and `--verify-release` watches the
  next release for provenance (#3).
- PyPI bootstraps through a pending publisher and a first OIDC publish from CI;
  crates.io publishes once with a revocable token and then opens the trusted
  publisher settings (#3).
- Docker Hub and GHCR images are detected from a `Dockerfile` and the release
  workflow, with plans for the repository, a scoped token, `DOCKERHUB_*`
  variables and secret, `packages: write`, and package linking (#5).
- `setup --browser default|automated` chooses where sign-in and approval URLs
  open; `default`, the default, uses the user's default browser (#9).
- An npm first publish stops before the sign-in when the packed tarball
  contains repository development directories such as `.github/` or `tests/`,
  which a package.json without a `files` allowlist lets in.
- SwiftPM packages (`--registry swift`) are discovered from `Package.swift`
  and `swift/VERSION` and verified by exact bare tag, commit, tagged manifest,
  public release and consumer (#77). Kotlin Multiplatform Gradle publications
  are discovered with their platform variants and verified by root POM,
  `.module` metadata and every variant (#78).
- Generated Central, Packagist and SwiftPM setup steps run their own handlers;
  manual prerequisites stay incomplete without read-back evidence (#79).

### Changed

- `setup` stays signed in by default: the npm session token and the crates.io
  sign-in in the automated profile are kept, so the next run (npm: within two
  hours) skips the sign-in. `--sign-out` signs out at the end instead;
  `--keep-session` is still accepted and is now the default.
- npm web-auth URLs (`npm login`, `npm publish` 2FA, `npm trust`) open in the
  user's default browser (`open`, `xdg-open`, or the Windows URL handler),
  where they are usually already signed in. The automated browser profile is
  only used to fill the trusted-publisher form (#9).
- The automation browser profile now defaults to a per-user state directory
  outside any repository (`~/Library/Application Support` on macOS,
  `$XDG_STATE_HOME` or `~/.local/state` on Linux, `%LOCALAPPDATA%` on Windows,
  each followed by `package-registry-manager/browser-profile`) (#8).
- `browser-commander` is required at `^0.34.0`, its latest release (#28). A
  launch still names its channel (`chrome` by default), so the upstream switch
  of channel-less launches to Chrome for Testing does not change which browser
  opens.

### Fixed

- A browser profile inside a Git work tree gets a `.gitignore` containing `*`
  before the browser starts, and setup refuses to continue if Git still does
  not ignore it or already tracks files in it. A profile left in
  `.package-registry-manager/` by an earlier release is protected the same way,
  with a warning to delete it (#8).
- When a web login is not completed and npm falls back to its legacy
  `Username:` prompt, setup stops npm and says to re-run for a fresh login link
  instead of failing with `npm exited with status 1` (#9).

- `plan` no longer emits setup steps or a trusted publisher for unpublishable
  packages; they appear with `"steps": []` and a `skipped_reason`, and `setup`
  picks the only publishable package when a registry also has private ones
  (#4).
- Central and Packagist steps no longer reach the crates.io session (#79).
  Account scans fetch referenced version sidecars at the scanned commit, so
  they agree with local inspection (#80).

## [0.19.35] - 2026-09-21

### Added

- Initial JavaScript implementation of repository inspection and registry
  setup planning.

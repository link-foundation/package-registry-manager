# Changelog

All notable changes to the JavaScript package are documented here. The package
currently shares its release version with the Rust implementation.

## [Unreleased]

### Added

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

### Changed

- npm web-auth URLs (`npm login`, `npm publish` 2FA, `npm trust`) open in the
  user's default browser (`open`, `xdg-open`, or the Windows URL handler),
  where they are usually already signed in. The automated browser profile is
  only used to fill the trusted-publisher form (#9).
- The automation browser profile now defaults to a per-user state directory
  outside any repository (`~/Library/Application Support` on macOS,
  `$XDG_STATE_HOME` or `~/.local/state` on Linux, `%LOCALAPPDATA%` on Windows,
  each followed by `package-registry-manager/browser-profile`) (#8).
- `browser-commander` is required at `^0.22.0`, its latest release (#28).

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

## [0.19.35] - 2026-09-21

### Added

- Initial JavaScript implementation of repository inspection and registry
  setup planning.

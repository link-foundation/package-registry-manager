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

### Fixed

- `plan` no longer emits setup steps or a trusted publisher for unpublishable
  packages; they appear with `"steps": []` and a `skipped_reason`, and `setup`
  picks the only publishable package when a registry also has private ones
  (#4).

## [0.19.35] - 2026-09-21

### Added

- Initial JavaScript implementation of repository inspection and registry
  setup planning.

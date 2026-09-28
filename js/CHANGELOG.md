# Changelog

All notable changes to the JavaScript package are documented here. The package
currently shares its release version with the Rust implementation.

## [Unreleased]

### Fixed

- `plan` no longer emits setup steps or a trusted publisher for unpublishable
  packages; they appear with `"steps": []` and a `skipped_reason`, and `setup`
  picks the only publishable package when a registry also has private ones
  (#4).

## [0.19.35] - 2026-09-21

### Added

- Initial JavaScript implementation of repository inspection and registry
  setup planning.

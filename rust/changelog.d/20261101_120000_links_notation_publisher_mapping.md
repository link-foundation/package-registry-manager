---
bump: minor
---

### Added

- Model Maven Central (Central user-token pair, verified namespace, GPG signing
  key and passphrase) and public Packagist (stable-tag read-back, GitHub hook,
  `PACKAGIST_USERNAME`/`PACKAGIST_TOKEN` update API) as structured credential
  requirements and plan states, without claiming OIDC for either registry.
- Migrate guarded multiline `dotnet nuget push` steps to NuGet OIDC first with
  the existing `NUGET_API_KEY`/`NUGET_TOKEN` fallback, keeping pack, version
  guard and release outputs; anything else returns the exact prerequisite edit.
- Verify exact Go module versions through the proxy `@v/<version>.info`.

### Fixed

- Map nested reusable publishers (`js.yml -> release.yml -> publish-*.yml`),
  shell-function publish commands and `./mvnw -B deploy`, and report callee jobs.
- Read NuGet `VersionPrefix`, CMake `file(READ VERSION)` and Go `VERSION` files;
  treat root `composer.json` as canonical over a `php/composer.json` copy and
  benchmark manifests as internal, so no false "add publishing job" request.
- Report the PyPI `vars.X == 'true' && '' || secrets.PYPI_TOKEN` expression as
  token-only and variable-gated crates.io OIDC as its own policy.
- NuGet CI verification accepts expected skipped jobs (PR-only review,
  unchanged benchmarks) but still rejects a skipped required release job.

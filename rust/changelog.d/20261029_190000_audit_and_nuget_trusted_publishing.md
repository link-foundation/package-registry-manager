### Added

- Discover generated npm aliases and CMake, Conan and vcpkg distribution targets;
  report exact public manifest-version evidence and conflicting package identities.
- Add exact NuGet publisher policy discovery/readback and job-local OIDC-first
  workflow proposals with a retained provided-token fallback and bounded retries.

### Fixed

- Recover npm owner-bootstrap preflight failures only after fresh package and
  publisher verification; verify every generated name at its actual release version.
- Resolve Cargo workspace versions and exclude Android build/application files,
  unpublished examples and historical development snapshots from artifact audits.
- Preserve existing browser credentials with fresh sign-in migration targets and
  distinguish uncertain provider/publication evidence from verified completion.

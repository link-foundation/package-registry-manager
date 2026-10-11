### Fixed

- Keep valid packages and continue organization/user scans after repository,
  manifest, registry lookup, and release-log failures. Stream completed text
  results with progress, include failures in JSON, and exit nonzero at the end.
- Check only the latest default-branch publishing workflow run, download logs
  only for failed releases, and retain failed run evidence when logs expired.
- Let account setup plan and execute usable packages despite scan failures.
- Report the Node.js 22.12 minimum before loading GitHub dependencies and list
  it in setup prerequisites.

### Changed

- Default online commands to the available organization, then signed-in user,
  with local fallback. Explicit repository/ref and offline local scopes remain
  supported. Account plan/setup JSON includes scan details and a plans array.

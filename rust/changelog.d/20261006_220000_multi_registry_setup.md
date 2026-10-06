### Added

- `setup --all` processes every publishable package with one browser session,
  deferred registry sign-out and a per-package completion summary.
- Missing npm, crates.io and PyPI publishing jobs can be proposed together in
  a reviewed draft pull request on a new branch, using the existing shared
  release workflow and OIDC authentication. `--add-publish-job --workflow`
  chooses an explicit proposal target.

### Fixed

- Detect PyPI publication through repository scripts using `python -m twine
  upload`, and report the workflow job invoking the script.
- Warn when CI does not publish a package and prevent trusted-publisher
  attachment without a detected job or explicit workflow override.
- Read `requires-python` and choose a compatible installed interpreter for
  local PyPI builds, with an installation hint when none is available.

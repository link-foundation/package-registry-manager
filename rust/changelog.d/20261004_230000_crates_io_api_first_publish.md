---
bump: minor
---

### Added
- crates.io first publish through the crates.io API: after one browser sign-in (reused when the automated profile already has the session) and one confirmation, the CLI creates a 1-hour token limited to the crate with the `publish-new` and `trusted-publishing` scopes from inside the crates.io page, runs `cargo publish` with `CARGO_REGISTRY_TOKEN` set only for that child process, attaches the trusted publisher with `POST /api/v1/trusted_publishing/github_configs`, and revokes the token with `DELETE /api/v1/tokens/current` even when the publish fails, failing the run while crates.io still accepts it. No `cargo login`, no credentials file, and the token is never printed. `--manual` keeps the previous checklist (#26)
- `--browser-import default|auto` and `--browser-import-scope full|domains`; without `--browser-import`, a sign-in step that finds no session offers once to import only the registry's sign-in cookies (`crates.io` + `github.com`, `npmjs.com`, `pypi.org` + `github.com`) from an installed browser (#26)

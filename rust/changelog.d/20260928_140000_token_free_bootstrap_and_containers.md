---
bump: minor
---

### Added
- `setup --registry npm --execute` takes a never-published package to OIDC trusted publishing in one run: web login, a confirmed first publish from a temporary worktree of the default branch, `npm trust github`, and cleanup of a leftover `NPM_TOKEN` secret. It never reads, creates, or asks for an npm token (#3)
- `inspect` and `plan` look packages up on npm, crates.io, PyPI, and Docker Hub and plan a `bootstrap`, `attach`, or `complete` mode; `--offline` skips the lookups, `--dry-run` prints the whole flow, and `--verify-release` watches the next release for provenance (#3)
- PyPI bootstraps through a pending publisher and a first OIDC publish from CI; crates.io publishes once with a revocable token and then opens the trusted publisher settings (#3)
- Docker Hub and GHCR images are detected from a `Dockerfile` and the release workflow, with plans for the repository, a scoped token, `DOCKERHUB_*` variables and secret, `packages: write`, and package linking (#5)
- The Rust CLI now matches the JavaScript CLI's plan JSON and text output (#6)

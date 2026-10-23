---
bump: patch
---

CI/CD: crates.io and npm publishing try trusted publishing (OIDC) first and fall back to a token secret when the OIDC exchange fails: `CARGO_REGISTRY_TOKEN` / `CARGO_TOKEN` for crates.io, `NPM_TOKEN` for npm (issue #62). Only the publish steps read the secrets, and `publish-crate.rs` warns when it publishes with the fallback secret.

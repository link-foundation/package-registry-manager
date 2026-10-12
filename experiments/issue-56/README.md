# Legacy token rotation and trusted publishing migration

The shared `tests/fixtures/token-rotation` repository reproduces `start`'s
publishing structure: `js.yml` calls `rust.yml`, which invokes a JavaScript
script using a variable Cargo executable and a multiline `publish` argument
array. Before the fix, inspection did not select the caller workflow and
account snapshots omitted the referenced script. A healthy secret also caused
the credential cycle to return without replacing it, even for explicit rotation.

Run the deterministic regression tests:

```sh
node --test js/test/token-rotation.test.mjs js/test/crates-token-rotation.test.mjs
CARGO_BUILD_JOBS=1 cargo test --manifest-path rust/Cargo.toml --test unit token_rotation
CARGO_BUILD_JOBS=1 cargo test --manifest-path rust/Cargo.toml --test unit ci_credential_cycle
CARGO_BUILD_JOBS=1 cargo test --manifest-path rust/Cargo.toml --test integration account_snapshot_follows_referenced_script
node --test js/test/workflow-migration.test.mjs js/test/migration-cleanup.test.mjs
CARGO_BUILD_JOBS=1 cargo test --manifest-path rust/Cargo.toml --test unit workflow_migration
node experiments/issue-56/migration-preview.mjs
```

The browser mock executes the generated fetch scripts and records token creation,
scope-preserving storage with expiry, CI tests for every consumer, and revocation
in that order. Ambiguous identity, incomplete scans, and uncertain or failing CI
leave existing tokens available. Broad organization visibility and repository
fallback are covered without accessing a live account or changing real secrets.

The fixture also reproduces loss of the callee's `release-main` environment.
`expected.json` contains the converted caller and callee, checked by both CLIs.
Use `migration-preview.mjs --record` to update this golden output intentionally.
Cleanup mocks cover real OIDC uploads versus successful no-op runs, stale commits,
changed default branches, shared consumers, dynamic secret forwarding and incomplete
scans. Workflow tests preserve permissions, conditions, commands and unrelated jobs.
Multiline publishing guards are copied in full; different publishing guards in one
Cargo job stop conversion before a PR can leave a publisher without a token.

For a read-only GitHub API probe against the current `link-foundation/start`, run
`node examples/inspect-start-publisher.mjs` after authenticating `gh` and installing
JavaScript dependencies. It reports `js.yml` as the caller. The live repository
may migrate independently; the fixed fixture retains its legacy token references.

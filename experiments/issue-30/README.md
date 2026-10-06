# Browser catalogue investigation (#30)

The original JavaScript import code checked `SUPPORTED_COOKIE_BROWSERS`, an export absent
from the published package, and silently used a five-browser fallback. Rust
already accepted additional cookie sources but kept a separate five-browser
name map. Both ports maintained separate default-browser identifiers and
collapsed most snapshot channels to Chrome or stable Edge.

The regression suites were added before implementation:

```bash
node --test js/test/browser-catalogue.test.mjs
cargo test --manifest-path rust/Cargo.toml --test unit browser_catalogue
```

The original JavaScript implementation failed all five initial cases (imports,
names, help, snapshot selection, and diagnostics). Rust failed the Vivaldi name
assertion: actual `vivaldi`, expected `Vivaldi`. The suites now also cover every
upstream id/alias, operating-system identifiers, legacy identifiers, unsupported
launch engines, and mocked executable/profile discovery on all three platforms.
The Windows discovery regression also verifies case-insensitive `ProgramFiles`
and `Path` keys in environment snapshots; both ports missed those installations
before key normalization.

Initial CI run `37422894841`, created 2026-10-06T06:17:14Z for
`9bf1c9fc838292011c4aad569d285beb73849caf`, failed the upstream dependency check.
Its log lines 193–196 identified Browser Commander JS 0.22.0 / Rust 0.15.0 and
command-stream JS 1.3.0 / Rust 1.2.0 as outdated. The updated releases are
Browser Commander 0.25.0 / 0.18.0 and command-stream 1.5.0 / 1.4.1. ESLint and
Tokio were also updated after auditing every direct dependency:

```bash
mkdir -p ci-logs
node experiments/issue-30/dependency-releases.mjs
node scripts/check-upstream-dependencies.mjs
```

The audit records current registry metadata in the ignored `ci-logs/` directory.
The regression and CI logs are kept there as well. On a 3 GiB development
container, use two Cargo build jobs and disable debug information to compile
the large Chromium protocol crate:

```bash
CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 \
  cargo test --manifest-path rust/Cargo.toml --all-targets
```

CI run `37426694996`, created 2026-10-06T06:57:13Z for `bed3903`, also
exposed test portability issues. Its Windows JavaScript log line 231 showed
`D:\D:\...` from using a file URL's pathname instead of `fileURLToPath`.
The macOS log line 440 reported a missing opener entry: the helper could delete
the log between its final snapshot and a detached opener's append. Clearing
the previous log before starting children preserves late entries for polling.
Clippy 1.99's `assert_is_empty` error appears in lint log line 781 and fresh-merge
log line 1176; equality with an empty vector gives useful failure diagnostics.
Completed job logs can be downloaded while the workflow is still running with
`gh api repos/link-foundation/package-registry-manager/actions/jobs/JOB_ID/logs
--allow-escape-sequences > ci-logs/JOB_NAME.log`.

The public catalogue exposes 31 sources in these releases. It supplies
identities, aliases, profile roots, executable templates, default-browser
identifiers, and control protocols. JavaScript reaches entries through
`BROWSER_IDS` and `findBrowserSource`; Rust uses `browser_sources` and the public
root resolver. The published packages keep their executable resolver private,
so discovery expands the public executable templates and PATH names locally.
Readable names come from application paths and aliases.

Chromium targets have CDP control and are accepted by the real launcher used by
this project. Firefox-family entries advertise BiDi but the real launcher still
rejects them. The upstream migration matrix also marks Firefox/WebKit target
writers unsupported; DuckDuckGo is detection-only. Source recognition does not
imply support for every data class or for launching that source as a target.
These remaining requirements were
[reported upstream](https://github.com/link-foundation/browser-commander/issues/114#issuecomment-6010684004).

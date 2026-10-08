# Repository transfers and trusted publishing (#41)

The issue reports that moving `konard/disk-space-saviour` to
`link-foundation/disk-space-saviour` left npm's trusted publisher and
`package.json` repository URL pointing to the old owner. npm rejected the first
release with E404. Inspection inferred completion from the historical version's
trusted-publishing flag, so setup omitted every repair.

## Reproduction and evidence

The shared `tests/fixtures/repository-transfer/inspection.json` reproduces that
state: a published package with historical trusted publishing, but repository
findings naming the old owner. The initial JavaScript and Rust regression tests
failed because the resulting mode was `complete` instead of `repair`.
`evidence.json` supplies npm SLSA provenance, PyPI provenance and configured
publisher identities separately. Tests also obtain findings from actual
manifest files and mocked registry responses rather than only preset findings.

Run the focused regression suites:

```sh
node --test --test-timeout=30000 js/test/repository-transfer.test.mjs
node --test --test-timeout=30000 --test-name-pattern='repository transfer' js/test/npm-bootstrap.test.mjs
cargo test --manifest-path rust/Cargo.toml repository_transfer
# Requires an installed Chrome; all browser requests are intercepted:
node experiments/issue-41/pypi-settings.mjs
```

## Resulting behavior

Online inspection resolves GitHub redirects with `gh api repos/<slug> --jq
.full_name`. Local manifest URLs are also checked offline. Latest npm/PyPI
attestations and latest Cargo version metadata are compared separately from
configured publishers. Each disagreement has its evidence source and a warning.
An unavailable authenticated settings lookup stays unverified; historical
provenance cannot turn that state into a completed setup.

The repair plan checks/attaches the current repository's publisher, verifies the
repository, workflow and environment, removes only publishers for other
repositories, then verifies both that the replacement survives and old trust is
gone. Other workflows of the current repository survive. npm uses the interactive
trust CLI; crates.io uses the authenticated, paginated settings API; PyPI reads
the publisher table and submits the selected removal form with its CSRF fields.
Failures stop release retries and preserve old trust until replacement is proven.

Manifest repairs use a separate worktree from the fetched remote default branch,
commit only the affected manifest, and offer a draft PR. The caller's checkout
is not edited. npm repository.directory and Python URL path suffixes survive.
Creating or declining the PR pauses setup. A later execution checks the failed
run's SHA before rerunning it. If only the default branch has corrected metadata,
setup dispatches a fresh workflow instead of replaying the old manifest.

## Practical boundaries

Registry settings may require an authenticated browser session or npm approval.
Inspection reports an unverified publisher when it cannot read those settings;
setup must verify them before removal. A full registry publisher quota can reject
attachment: setup stops and keeps existing trust rather than removing it first.
The fresh-run path requires workflow_dispatch; workflows restricted to release
tags may require a new release after the reviewed manifest fix is merged.
The changes were tested with deterministic registry/browser fixtures, without
changing a live account's publishers or publishing package artifacts.

## Registry contracts consulted

- [npm trust commands](https://docs.npmjs.com/cli/v11/commands/npm-trust/)
- [npm trusted publishing](https://docs.npmjs.com/trusted-publishers/)
- [PyPI integrity API](https://docs.pypi.org/api/integrity/)
- [Warehouse publisher table and removal form](https://github.com/pypi/warehouse/blob/main/warehouse/templates/manage/partials/_manage_base.html)
- [crates.io publisher-list endpoint](https://github.com/rust-lang/crates.io/blob/main/src/controllers/trustpub/github_configs/list.rs)

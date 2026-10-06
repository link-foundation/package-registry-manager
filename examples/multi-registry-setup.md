# Set up an npm, Rust and Python repository

A repository with `js/package.json`, `rust/Cargo.toml` and
`python/pyproject.toml` can be configured in one run:

```bash
package-registry-manager inspect --repository /path/to/repo
package-registry-manager setup --repository /path/to/repo --all
package-registry-manager setup --repository /path/to/repo --all --execute
```

The JavaScript CLI accepts the same arguments. Inspection follows publishing
scripts called by workflow jobs. If only PyPI has a publishing job in
`release.yml`, setup previews npm and crates.io OIDC jobs in that same file
and offers a draft pull request on a new branch. Review the versioning,
triggers and build dependencies, then merge the workflow and run setup again.
An ambiguous workflow target can be selected with
`--add-publish-job --workflow release.yml`.

The second run reuses a single automated browser across packages and signs
out at the end. npm performs its web approvals without another login for each
package. Python's local build uses an interpreter satisfying `requires-python`;
if the default Python is 3.12 and the project requires `>=3.13`, an installed
`python3.13` or `python3.14` is selected. The final summary reports configured,
complete, failed, blocked or unrun packages.

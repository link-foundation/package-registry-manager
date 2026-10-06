# Multi-registry release detection reproduction

Run `node experiments/issue-33/reproduce.mjs` from the repository root. The
script creates and removes a temporary repository with npm, Rust and Python
packages and a shared release job invoking a Node.js script containing
`python -m twine upload`.

Before the fix, PyPI had no workflow metadata and npm/crates.io plans included
trusted-publisher attachment despite having no CI publishing jobs. After the
fix, PyPI reports `release.yml` and job `release`; the other two packages warn
that CI releases will not reach them and offer a workflow pull request instead
of registry attachment. The Python package also retains `requires_python`.

The automated regression tests live in both languages' publisher and Python
suites. Workflow proposal tests verify OIDC templates and preservation of other
jobs; setup tests verify shared sessions, final logout and cleanup on failure.

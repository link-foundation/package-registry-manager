# package-registry-manager (Rust)

Rust implementation of
[package-registry-manager](https://github.com/link-foundation/package-registry-manager).
It discovers package manifests, emits registry setup plans, validates packages
with exact argument vectors, and guides authenticated setup in the default
browser, filling forms in a dedicated visible browser profile.

```bash
cargo install package-registry-manager
package-registry-manager inspect --repository /path/to/repository
package-registry-manager plan --repository /path/to/repository --format json
package-registry-manager setup --repository /path/to/repository --registry npm
package-registry-manager setup --repository /path/to/repository --all --execute
```

Setup is a dry run unless `--execute` is present. It can publish the first
version after confirmation; later versions publish through CI. Missing CI
publishing jobs are offered in a draft pull request, and PyPI builds select an
interpreter satisfying `requires-python`. See the [repository README](../README.md)
for the registry walkthroughs and safety details.
See the [bootstrap, wrapper and credential guide](https://github.com/link-foundation/package-registry-manager/blob/main/docs/registry-setup.md) for branch/PR setup, dual npm names, registry policies and integration limits.

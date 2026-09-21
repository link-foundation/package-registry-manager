# package-registry-manager (Rust)

Rust implementation of
[package-registry-manager](https://github.com/link-foundation/package-registry-manager).
It discovers package manifests, emits registry setup plans, validates packages
with exact argument vectors, and guides authenticated setup in a dedicated
visible browser.

```bash
cargo install package-registry-manager
package-registry-manager inspect --repository /path/to/repository
package-registry-manager plan --repository /path/to/repository --format json
package-registry-manager setup --repository /path/to/repository --registry npm
```

Setup is a dry run unless `--execute` is present, and it never publishes an
artifact. See the [repository README](../README.md) for all registry
walkthroughs and safety details.

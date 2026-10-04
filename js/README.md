# package-registry-manager (JavaScript)

Node.js implementation of
[package-registry-manager](https://github.com/link-foundation/package-registry-manager).
It discovers package manifests, emits registry setup plans, validates packages
with exact argument vectors, and guides authenticated setup in the default
browser, filling forms in a dedicated visible browser profile.

```bash
npx package-registry-manager inspect --repository /path/to/repository
npx package-registry-manager plan --repository /path/to/repository --format json
npx package-registry-manager setup --repository /path/to/repository --registry npm
```

The package installs the command as both `package-registry-manager` and
`package-registry-manager-js`; use the second name when the Rust command of the
same name is also on your `PATH`.

Setup is a dry run unless `--execute` is present, and it never publishes an
artifact. See the repository README for all registry walkthroughs and safety
details.

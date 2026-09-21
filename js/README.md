# package-registry-manager (JavaScript)

Node.js implementation of
[package-registry-manager](https://github.com/link-foundation/package-registry-manager).
It discovers package manifests, emits registry setup plans, validates packages
with exact argument vectors, and guides authenticated setup in a dedicated
visible browser.

```bash
npm install package-registry-manager
npx package-registry-manager-js inspect --repository /path/to/repository
npx package-registry-manager-js plan --repository /path/to/repository --format json
npx package-registry-manager-js setup --repository /path/to/repository --registry npm
```

Setup is a dry run unless `--execute` is present, and it never publishes an
artifact. See the repository README for all registry walkthroughs and safety
details.

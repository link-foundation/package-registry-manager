#!/usr/bin/env bash
# Reproduces #22: packs js/, installs the tarball, and runs the bins through
# the node_modules/.bin symlinks npm creates. Before the fix every command
# exited 0 without output.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
(cd "$root/js" && npm pack --ignore-scripts --pack-destination "$work" >/dev/null 2>&1)
npm install --no-save --no-package-lock --no-audit --no-fund --ignore-scripts \
  --prefer-offline --prefix "$work/install" "$work"/package-registry-manager-*.tgz >/dev/null
for bin in "$work"/install/node_modules/.bin/package-registry-manager*; do
  echo "== $(basename "$bin") --version"
  "$bin" --version
  echo "== $(basename "$bin") inspect"
  "$bin" inspect --offline --repository "$root/tests/fixtures/polyglot" | sed -n 1,3p
done

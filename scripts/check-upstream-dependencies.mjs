#!/usr/bin/env node

/**
 * Fail when a link-foundation dependency falls behind its latest release.
 *
 * For a 0.x version a caret requirement never reaches the next minor release
 * (`^0.19.0` stays below 0.20.0), so a dependency can silently freeze. This
 * check compares the lowest version each manifest accepts with the latest
 * release on npm or crates.io.
 *
 * Usage:
 *   node scripts/check-upstream-dependencies.mjs
 *
 * Exit codes:
 *   - 0: every link-foundation dependency requires its latest release
 *   - 1: a dependency is behind, missing from its manifest, or not found
 */

import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

/** The link-foundation packages each manifest must keep current. */
export const UPSTREAM = [
  { registry: 'npm', manifest: 'js/package.json', name: 'browser-commander' },
  { registry: 'npm', manifest: 'js/package.json', name: 'command-stream' },
  {
    registry: 'crates',
    manifest: 'rust/Cargo.toml',
    name: 'browser-commander',
  },
  { registry: 'crates', manifest: 'rust/Cargo.toml', name: 'command-stream' },
  { registry: 'crates', manifest: 'rust/Cargo.toml', name: 'lino-arguments' },
];

/** The requirement `package.json` declares for `name`, or null. */
export function npmRequirement(text, name) {
  const manifest = JSON.parse(text);
  return (
    manifest.dependencies?.[name] ?? manifest.devDependencies?.[name] ?? null
  );
}

/**
 * The requirement `Cargo.toml` declares for `name` in `[dependencies]`, as
 * `name = "1.2"` or `name = { version = "1.2", ... }`, or null.
 */
export function cargoRequirement(text, name) {
  const section =
    text.split(/^\[dependencies\]\s*$/m)[1]?.split(/^\[/m)[0] ?? '';
  const escaped = name.replace(/[.*+?^${}()|[\]\\-]/g, '\\$&');
  const match = section.match(
    new RegExp(
      `^${escaped}\\s*=\\s*(?:"([^"]+)"|\\{[^}\\n]*\\bversion\\s*=\\s*"([^"]+)")`,
      'm'
    )
  );
  return match ? (match[1] ?? match[2]) : null;
}

/** The lowest version a caret, tilde, or exact requirement accepts. */
export function minimumVersion(requirement) {
  const match = String(requirement).match(
    /^\s*[\^~=]?\s*v?(\d+)(?:\.(\d+))?(?:\.(\d+))?/
  );
  if (!match) {
    return null;
  }
  return [match[1], match[2] ?? '0', match[3] ?? '0'].map(Number);
}

/** Negative, zero, or positive as version `left` is below, equal to, or above `right`. */
export function compareVersions(left, right) {
  for (let index = 0; index < 3; index += 1) {
    if (left[index] !== right[index]) {
      return left[index] - right[index];
    }
  }
  return 0;
}

async function latestVersion(registry, name, fetchJson) {
  if (registry === 'npm') {
    return (await fetchJson(`https://registry.npmjs.org/${name}/latest`))
      .version;
  }
  const crate = await fetchJson(`https://crates.io/api/v1/crates/${name}`);
  return crate.crate.max_stable_version ?? crate.crate.max_version;
}

async function fetchJsonFromRegistry(url) {
  const response = await fetch(url, {
    headers: {
      // crates.io rejects requests without an identifying user agent.
      'User-Agent':
        'package-registry-manager dependency check (https://github.com/link-foundation/package-registry-manager)',
      Accept: 'application/json',
    },
  });
  if (!response.ok) {
    throw new Error(`${url} answered ${response.status}`);
  }
  return response.json();
}

/** Checks every upstream dependency and returns one finding per problem. */
export async function checkUpstreamDependencies({
  upstream = UPSTREAM,
  readManifest = (manifest) => readFileSync(path.join(ROOT, manifest), 'utf8'),
  fetchJson = fetchJsonFromRegistry,
} = {}) {
  const results = [];
  for (const { registry, manifest, name } of upstream) {
    const text = readManifest(manifest);
    const requirement =
      registry === 'npm'
        ? npmRequirement(text, name)
        : cargoRequirement(text, name);
    if (!requirement) {
      results.push({
        manifest,
        name,
        problem: `${name} is not a dependency in ${manifest}`,
      });
      continue;
    }
    const latest = await latestVersion(registry, name, fetchJson);
    const minimum = minimumVersion(requirement);
    const behind =
      !minimum || compareVersions(minimum, minimumVersion(latest)) < 0;
    results.push({
      manifest,
      name,
      requirement,
      latest,
      problem: behind
        ? `${manifest} requires ${name} ${requirement}, but ${latest} is the latest release`
        : null,
    });
  }
  return results;
}

async function main() {
  const results = await checkUpstreamDependencies();
  for (const result of results) {
    console.log(
      result.problem
        ? `✗ ${result.problem}`
        : `✓ ${result.manifest}: ${result.name} ${result.requirement} (latest ${result.latest})`
    );
  }
  if (results.some((result) => result.problem)) {
    console.log(
      '\nBump the requirement to the latest release (a 0.x caret never reaches the next minor).'
    );
    process.exit(1);
  }
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  await main();
}

import assert from 'node:assert/strict';
import test from 'node:test';

import {
  cargoRequirement,
  checkUpstreamDependencies,
  compareVersions,
  minimumVersion,
  npmRequirement,
} from './check-upstream-dependencies.mjs';

const CARGO = `[package]
name = "demo"
version = "0.1.0"

[dependencies]
browser-commander = "0.14.1"
command-stream = { version = "1.2", default-features = false }
lino-arguments-extra = "9.9.9"

[dev-dependencies]
lino-arguments = "0.3.0"
`;

test('reads requirements from package.json and Cargo.toml', () => {
  const npm = JSON.stringify({
    dependencies: { 'browser-commander': '^0.21.2' },
  });
  assert.equal(npmRequirement(npm, 'browser-commander'), '^0.21.2');
  assert.equal(npmRequirement(npm, 'command-stream'), null);
  assert.equal(cargoRequirement(CARGO, 'browser-commander'), '0.14.1');
  assert.equal(cargoRequirement(CARGO, 'command-stream'), '1.2');
  // Only [dependencies] counts, and a longer name is not a match.
  assert.equal(cargoRequirement(CARGO, 'lino-arguments'), null);
});

test('compares the lowest accepted version with the latest release', () => {
  assert.deepEqual(minimumVersion('^0.19.0'), [0, 19, 0]);
  assert.deepEqual(minimumVersion('0.13'), [0, 13, 0]);
  assert.deepEqual(minimumVersion('~1.2.3'), [1, 2, 3]);
  assert.deepEqual(minimumVersion('=2'), [2, 0, 0]);
  assert.equal(minimumVersion('*'), null);
  assert.ok(compareVersions([0, 19, 0], [0, 21, 0]) < 0);
  assert.ok(compareVersions([1, 3, 0], [1, 2, 9]) > 0);
  assert.equal(compareVersions([1, 2, 0], [1, 2, 0]), 0);
});

test('flags a dependency frozen behind its latest release', async () => {
  const manifests = {
    'js/package.json': JSON.stringify({
      dependencies: {
        'browser-commander': '^0.19.0',
        'command-stream': '^1.3.0',
      },
    }),
    'rust/Cargo.toml': CARGO,
  };
  const releases = {
    'https://registry.npmjs.org/browser-commander/latest': {
      version: '0.21.0',
    },
    'https://registry.npmjs.org/command-stream/latest': { version: '1.3.0' },
    'https://crates.io/api/v1/crates/browser-commander': {
      crate: { max_stable_version: '0.14.1', max_version: '0.15.0-beta.1' },
    },
  };
  const results = await checkUpstreamDependencies({
    upstream: [
      {
        registry: 'npm',
        manifest: 'js/package.json',
        name: 'browser-commander',
      },
      { registry: 'npm', manifest: 'js/package.json', name: 'command-stream' },
      {
        registry: 'crates',
        manifest: 'rust/Cargo.toml',
        name: 'browser-commander',
      },
      {
        registry: 'crates',
        manifest: 'rust/Cargo.toml',
        name: 'lino-arguments',
      },
    ],
    readManifest: (manifest) => manifests[manifest],
    fetchJson: async (url) => releases[url],
  });
  assert.deepEqual(
    results.map((result) => result.problem),
    [
      'js/package.json requires browser-commander ^0.19.0, but 0.21.0 is the latest release',
      null,
      null,
      'lino-arguments is not a dependency in rust/Cargo.toml',
    ]
  );
});

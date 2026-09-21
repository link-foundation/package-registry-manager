import assert from 'node:assert/strict';
import { cp, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { after, before, test } from 'node:test';
import { fileURLToPath } from 'node:url';

import { npmPrefillScript } from '../src/browser.mjs';
import { inspectRepository } from '../src/discovery.mjs';
import { REGISTRIES } from '../src/model.mjs';
import { buildPlans } from '../src/plan.mjs';

let temporary;
let repository;

before(async () => {
  temporary = await mkdtemp(path.join(os.tmpdir(), 'package-registry-manager-'));
  repository = path.join(temporary, 'polyglot');
  const here = path.dirname(fileURLToPath(import.meta.url));
  await cp(path.resolve(here, '../../tests/fixtures/polyglot'), repository, {
    recursive: true,
  });
  await mkdir(path.join(repository, '.git'), { recursive: true });
  await writeFile(
    path.join(repository, '.git/config'),
    '[remote "origin"]\n\turl = https://github.com/acme/polyglot.git\n'
  );
});

after(async () => {
  await rm(temporary, { recursive: true, force: true });
});

test('discovers each maintained package registry', async () => {
  const inspection = await inspectRepository(repository);
  assert.deepEqual(
    [...new Set(inspection.packages.map((item) => item.registry))].sort(),
    [...REGISTRIES].sort()
  );
  assert.equal(inspection.packages.length, 7);
  assert.equal(inspection.repository.github_owner, 'acme');
  assert.equal(inspection.repository.github_repository, 'polyglot');
  assert.equal(inspection.repository.release_workflow, 'publish.yml');

  const actual = structuredClone(inspection);
  actual.repository.root = '<ROOT>';
  const expected = JSON.parse(
    await readFile(path.join(repository, 'expected-inspection.json'), 'utf8')
  );
  assert.deepEqual(actual, expected, 'JavaScript must honor the shared language contract');
});

test('prefills npm trusted-publisher identity', async () => {
  const inspection = await inspectRepository(repository);
  const npm = buildPlans(inspection).find((plan) => plan.registry === 'npm');
  assert.deepEqual(npm.trusted_publisher, {
    provider: 'github-actions',
    organization: 'acme',
    repository: 'polyglot',
    workflow: 'publish.yml',
  });
  assert.equal(
    npm.steps.find((step) => step.kind === 'browser').url,
    'https://www.npmjs.com/package/@acme%2Fwidgets/access'
  );
  const repeatedAt = structuredClone(inspection);
  repeatedAt.packages.find((item) => item.registry === 'npm').name = '@acme/@widgets';
  assert.equal(
    buildPlans(repeatedAt).find((plan) => plan.registry === 'npm').steps.at(-1).url,
    'https://www.npmjs.com/package/@acme%2F@widgets/access'
  );
  const script = npmPrefillScript(npm.trusted_publisher);
  assert.match(script, /publish\.yml/);
  assert.doesNotThrow(() => new Function(`return ${script}`));
});

test('never plans an artifact upload', async () => {
  const inspection = await inspectRepository(repository);
  for (const plan of buildPlans(inspection)) {
    for (const step of plan.steps) {
      if (!step.command) continue;
      const rendered = `${step.command.program} ${step.command.args.join(' ')}`;
      assert.notEqual(rendered, 'npm publish');
      assert.notEqual(rendered, 'cargo publish');
    }
  }
});

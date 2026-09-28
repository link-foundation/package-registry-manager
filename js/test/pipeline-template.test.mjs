import assert from "node:assert/strict";
import { cp, mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";

import { inspectRepository } from "../src/discovery.mjs";
import { buildPlans } from "../src/plan.mjs";

let temporary;
let repository;

before(async () => {
  temporary = await mkdtemp(
    path.join(os.tmpdir(), "package-registry-manager-template-"),
  );
  repository = path.join(temporary, "pipeline-template");
  const here = path.dirname(fileURLToPath(import.meta.url));
  await cp(
    path.resolve(here, "../../tests/fixtures/pipeline-template"),
    repository,
    { recursive: true },
  );
  await mkdir(path.join(repository, ".git"), { recursive: true });
  await writeFile(
    path.join(repository, ".git/config"),
    '[remote "origin"]\n\turl = https://github.com/acme/pipeline-app.git\n',
  );
});

after(async () => {
  await rm(temporary, { recursive: true, force: true });
});

test("plans no actionable steps for unpublishable packages", async () => {
  const inspection = await inspectRepository(repository);
  const plans = buildPlans(inspection, ["npm"]);
  const privatePlan = plans.find(
    (plan) => plan.package.name === "universal-example-app",
  );
  assert.equal(privatePlan.package.publishable, false);
  assert.deepEqual(privatePlan.steps, []);
  assert.equal(privatePlan.trusted_publisher, undefined);
  assert.equal(
    privatePlan.skipped_reason,
    "package.json marks this package as private",
  );

  const publicPlan = plans.find((plan) => plan.package.name === "pipeline-app");
  assert.equal(publicPlan.skipped_reason, undefined);
  assert.ok(publicPlan.steps.length > 0);
  assert.equal(publicPlan.trusted_publisher.workflow, "release.yml");
});

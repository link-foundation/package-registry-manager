import assert from "node:assert/strict";
import { cp, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
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

test("skips the example app, listing it only when asked", async () => {
  const inspection = await inspectRepository(repository);
  assert.equal(inspection.skipped, undefined);
  assert.equal(
    inspection.packages.some((item) => item.manifest.startsWith("examples/")),
    false,
  );
  const verbose = await inspectRepository(repository, {
    includeSkipped: true,
  });
  assert.deepEqual(verbose.skipped, [
    {
      manifest: "examples/universal-app/package.json",
      reason:
        "under examples/, a test or example directory, and no workflow publishes it",
    },
  ]);
  assert.deepEqual(verbose.packages, inspection.packages);
});

test("plans no actionable steps for unpublishable packages", async () => {
  const inspection = await inspectRepository(repository);
  inspection.packages.unshift({
    registry: "npm",
    name: "universal-example-app",
    version: "0.0.0",
    manifest: "apps/universal-app/package.json",
    publishable: false,
    problems: ["package.json marks this package as private"],
  });
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

test("detects Docker Hub and GHCR images from the Dockerfile and release workflow", async () => {
  const inspection = await inspectRepository(repository);
  const actual = structuredClone(inspection);
  actual.repository.root = "<ROOT>";
  const expected = JSON.parse(
    await readFile(path.join(repository, "expected-inspection.json"), "utf8"),
  );
  assert.deepEqual(actual, expected);
  const containers = inspection.packages.filter((item) =>
    ["docker-hub", "ghcr"].includes(item.registry),
  );
  assert.deepEqual(
    containers.map((item) => [item.registry, item.name, item.manifest]),
    [
      ["docker-hub", "acme/pipeline-app", "Dockerfile"],
      ["ghcr", "acme/pipeline-app", "Dockerfile"],
    ],
  );
});

test("plans Docker Hub repository, token, variables, and secret", async () => {
  const inspection = await inspectRepository(repository);
  const [plan] = buildPlans(inspection, ["docker-hub"]);
  assert.deepEqual(
    plan.steps.map((step) => step.id),
    [
      "check-registry",
      "create-repository",
      "check-github-cli",
      "set-image-variable",
      "set-username-variable",
      "manage-registry-token",
    ],
  );
  const argv = Object.fromEntries(
    plan.steps
      .filter((step) => step.command)
      .map((step) => [step.id, [step.command.program, ...step.command.args]]),
  );
  assert.deepEqual(argv["set-image-variable"], [
    "gh",
    "variable",
    "set",
    "DOCKERHUB_IMAGE",
    "--body",
    "acme/pipeline-app",
    "--repo",
    "acme/pipeline-app",
  ]);
  assert.deepEqual(argv["set-username-variable"], [
    "gh",
    "variable",
    "set",
    "DOCKERHUB_USERNAME",
    "--body",
    "acme",
    "--repo",
    "acme/pipeline-app",
  ]);
  assert.equal(
    plan.steps.find((step) => step.id === "manage-registry-token").kind,
    "api",
  );
  assert.equal(
    plan.steps.find((step) => step.id === "create-repository").url,
    "https://hub.docker.com/repository/create?namespace=acme",
  );

  const existing = structuredClone(inspection);
  existing.packages.find(
    (item) => item.registry === "docker-hub",
  ).exists_on_registry = true;
  const [attach] = buildPlans(existing, ["docker-hub"]);
  assert.equal(attach.mode, "attach");
  assert.equal(
    attach.steps.some((step) => step.id === "create-repository"),
    false,
  );
});

test("checks GHCR packages: write and links the package after the first push", async () => {
  const inspection = await inspectRepository(repository);
  const [plan] = buildPlans(inspection, ["ghcr"]);
  assert.equal(plan.package.warnings, undefined);
  assert.deepEqual(
    plan.steps.map((step) => step.id),
    ["link-package"],
  );
  assert.equal(
    plan.steps[0].url,
    "https://github.com/users/acme/packages/container/package/pipeline-app",
  );

  const workflow = path.join(repository, ".github/workflows/release.yml");
  const original = await readFile(workflow, "utf8");
  try {
    await writeFile(
      workflow,
      original.replace("packages: write", "packages: read"),
    );
    const [withoutPermission] = buildPlans(
      await inspectRepository(repository),
      ["ghcr"],
    );
    assert.match(withoutPermission.package.warnings[0], /packages: write/);
    assert.equal(withoutPermission.steps[0].id, "grant-packages-write");
  } finally {
    await writeFile(workflow, original);
  }
});

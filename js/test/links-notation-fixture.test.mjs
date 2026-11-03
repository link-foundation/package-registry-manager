import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { readdirSync, readFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { inspectRepository } from "../src/discovery.mjs";
import { buildPlans } from "../src/plan.mjs";
import { workflowMigrationProposal } from "../src/workflow-migration.mjs";

const root = fileURLToPath(
  new URL("../../tests/fixtures/links-notation", import.meta.url),
);

const summary = (p) => ({
  registry: p.registry,
  name: p.name,
  version: p.version ?? null,
  manifest: p.manifest,
  publishable: p.publishable,
  workflow: p.workflow ?? null,
  workflow_jobs: p.workflow_jobs ?? [],
  workflow_callees: p.workflow_callees ?? [],
  publishing_policy: p.publishing_policy ?? null,
});

test("links-notation: eight distributions plus the macro crate with exact identities", async () => {
  const inspection = await inspectRepository(root);
  const expected = JSON.parse(
    await readFile(`${root}/expected-packages.json`, "utf8"),
  );
  assert.deepEqual(inspection.packages.map(summary), expected);
  for (const item of inspection.packages.filter((p) => p.publishable)) {
    assert.equal(item.workflow, "js.yml", item.name);
    assert.ok(item.workflow_callees?.length || item.registry !== "cmake");
  }
  const skipped = (inspection.skipped ?? []).map(
    (s) => s.manifest ?? s.path ?? s,
  );
  assert.ok(
    JSON.stringify(skipped).includes("php/composer.json") ||
      !inspection.packages.some((p) => p.manifest === "php/composer.json"),
  );
  assert.ok(
    !inspection.packages.some(
      (p) => /bench/i.test(p.manifest) && p.publishable,
    ),
  );
});

test("links-notation: no false add-publishing-job request", async () => {
  const inspection = await inspectRepository(root);
  for (const plan of buildPlans(inspection, [], { addPublishJob: true })) {
    assert.ok(
      !(plan.skipped_reason ?? "").includes("no workflow publishes"),
      `${plan.registry}: ${plan.skipped_reason}`,
    );
  }
});

test("links-notation: Maven and Packagist plans expose structured credential states", async () => {
  const inspection = await inspectRepository(root);
  const plans = buildPlans(inspection, ["maven-central", "packagist"]);
  const maven = plans.find((p) => p.registry === "maven-central");
  const pair = maven.steps.find(
    (s) => s.id === "ensure-central-credential-pair",
  );
  assert.deepEqual(
    pair.secrets.map((s) => s.secret),
    ["CENTRAL_USERNAME", "CENTRAL_TOKEN"],
  );
  assert.equal(pair.oidc, "unsupported");
  assert.deepEqual(
    maven.steps
      .find((s) => s.id === "ensure-signing-key")
      .secrets.map((s) => s.secret),
    ["GPG_PRIVATE_KEY", "GPG_PASSPHRASE"],
  );
  assert.equal(
    maven.steps.find((s) => s.id === "verify-namespace").namespace,
    "io.github.link-foundation",
  );
  assert.ok(!maven.steps.some((s) => /rotate|revoke/i.test(s.id)));
  const packagist = plans.find((p) => p.registry === "packagist");
  assert.equal(
    packagist.steps.find((s) => s.id === "packagist-update-api").state,
    "configured",
  );
  assert.equal(
    packagist.steps.find((s) => s.id === "packagist-github-hook").state,
    "manual-prerequisite",
  );
});

test("links-notation: NuGet migration keeps pack, guard, fallback and id-token across callers", async () => {
  // Upstream now uses NuGet trusted publishing; the API-key workflows it
  // replaced are frozen next to the fixture.
  const dir = `${root}-legacy-nuget/.github/workflows`;
  const workflows = readdirSync(dir).map((name) => ({
    name,
    contents: readFileSync(`${dir}/${name}`, "utf8"),
  }));
  const inspection = await inspectRepository(root);
  const nuget = inspection.packages.find((p) => p.registry === "nuget");
  const proposal = await workflowMigrationProposal(root, [nuget], workflows);
  const csharp = proposal.files["publish-csharp.yml"];
  assert.match(
    csharp,
    /- name: Pack\n\s+if: steps.version-check.outputs.should_publish == 'true'/,
  );
  assert.match(csharp, /NuGet\/login@/);
  assert.match(csharp, /secrets.NUGET_API_KEY \|\| secrets.NUGET_TOKEN/);
  assert.match(csharp, /published=skipped/);
  assert.equal(csharp.match(/dotnet pack/g).length, 1);
  for (const name of ["js.yml", "release.yml", "publish-csharp.yml"]) {
    assert.match(proposal.files[name], /id-token: write/, name);
  }
});

test("links-notation: variable-gated crates migration asks to flip the variable, not rewrite", async () => {
  const inspection = await inspectRepository(root);
  for (const plan of buildPlans(inspection, ["crates-io"], {
    migrate: true,
  }).filter((p) => p.package.publishable)) {
    assert.deepEqual(
      plan.steps.map((s) => s.id),
      ["enable-oidc-variable"],
    );
    assert.match(plan.steps[0].title, /CRATES_IO_TRUSTED_PUBLISHING/);
  }
});

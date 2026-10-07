import assert from "node:assert/strict";
import { mkdtemp, mkdir, writeFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { inspectRepository } from "../src/discovery.mjs";
import { buildPlans } from "../src/plan.mjs";
import { credentialPolicy } from "../src/credential-cycle.mjs";

test("discovers OIDC registries and extension token publishers (#38)", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "prm-policy-"));
  try {
    const files = {
      "tool.gemspec":
        'Gem::Specification.new do |s|\n s.name = "tool"\n s.version = "1.0.0"\nend',
      "jsr.json": '{"name":"@acme/tool","version":"1.0.0"}',
      "extension/package.json":
        '{"name":"tool","version":"1.0.0","publisher":"acme","engines":{"vscode":"^1.80.0"}}',
      ".github/workflows/release.yml": `on: workflow_dispatch
jobs:
  release:
    permissions: {id-token: write}
    steps:
      - run: gem push tool.gem
      - run: deno publish
      - run: vsce publish
        env: {VSCE_PAT: '\${{ secrets.VSCE_PAT }}'}
      - uses: acme/chrome-webstore-upload@v1
      - run: ovsx publish
        env: {OVSX_PAT: '\${{ secrets.OVSX_PAT }}'}
`,
    };
    for (const [name, contents] of Object.entries(files)) {
      await mkdir(path.dirname(path.join(root, name)), { recursive: true });
      await writeFile(path.join(root, name), contents);
    }
    const inspection = await inspectRepository(root);
    for (const registry of [
      "rubygems",
      "jsr",
      "vscode-marketplace",
      "open-vsx",
      "chrome-web-store",
    ]) {
      const item = inspection.packages.find(
        (item) => item.registry === registry,
      );
      assert.ok(item, registry);
      assert.equal(item.workflow, "release.yml");
      const plan = buildPlans(inspection, [registry])[0];
      assert.ok(
        plan.steps.some(
          (step) =>
            step.id ===
            (credentialPolicy(registry).mode === "token"
              ? "manage-registry-token"
              : "configure-trusted-publisher"),
        ),
        registry,
      );
    }
    assert.deepEqual(
      inspection.packages.find((item) => item.registry === "vscode-marketplace")
        .token_secrets,
      ["VSCE_PAT"],
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

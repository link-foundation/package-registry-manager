import assert from "node:assert/strict";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { inspectRepository } from "../src/discovery.mjs";
import { readWorkflows } from "../src/workflows.mjs";
import { readFile } from "node:fs/promises";
import { buildPlans } from "../src/plan.mjs";
import { detectPublisher } from "../src/publishers.mjs";

const root = fileURLToPath(
  new URL("../../tests/fixtures/token-rotation", import.meta.url),
);

test("nested reusable callers register the outer workflow and convert every caller", async () => {
  const { workflowMigrationProposal } =
    await import("../src/workflow-migration.mjs");
  const workflows = [
    {
      name: "outer.yml",
      contents:
        "on: push\njobs:\n  call:\n    uses: ./.github/workflows/middle.yml\n    secrets:\n      CARGO_TOKEN: ${{ secrets.CARGO_TOKEN }}\n",
    },
    {
      name: "middle.yml",
      contents:
        "on: workflow_call\njobs:\n  call:\n    uses: ./.github/workflows/inner.yml\n    secrets:\n      CARGO_TOKEN: ${{ secrets.CARGO_TOKEN }}\n",
    },
    {
      name: "inner.yml",
      contents:
        "on: workflow_call\njobs:\n  release:\n    environment: release-main\n    steps:\n      - run: cargo publish\n        env:\n          CARGO_TOKEN: ${{ secrets.CARGO_TOKEN }}\n",
    },
  ];
  const publisher = await detectPublisher(root, workflows, "crates-io");
  assert.equal(publisher.workflow, "outer.yml");
  assert.equal(publisher.environment, "release-main");
  const proposal = await workflowMigrationProposal(
    root,
    [{ registry: "crates-io", workflow: publisher.workflow }],
    workflows,
  );
  assert.equal(Object.keys(proposal.files).length, 3);
  for (const contents of Object.values(proposal.files)) {
    assert.match(contents, /id-token: write/);
    assert.doesNotMatch(contents, /secrets.CARGO_TOKEN/);
  }
});

test("reusable publisher keeps caller filename and callee environment", async () => {
  const inspection = await inspectRepository(root);
  assert.equal(inspection.packages[0].workflow, "js.yml");
  assert.equal(inspection.packages[0].environment, "release-main");
});

test("conversion preserves inherited permissions, publishing conditions, and unrelated jobs", async () => {
  const { workflowMigrationProposal } =
    await import("../src/workflow-migration.mjs");
  const contents =
    "on: push\npermissions:\n  contents: write\n  actions: read\njobs:\n  build:\n    steps:\n      - run: cargo test\n  release:\n    steps:\n      - name: Publish crate\n        if: steps.version.outputs.changed == 'true'\n        run: cargo publish\n        env:\n          CARGO_TOKEN: ${{ secrets.CARGO_TOKEN }}\n";
  const proposal = await workflowMigrationProposal(
    root,
    [{ registry: "crates-io", workflow: "release.yml" }],
    [{ name: "release.yml", contents }],
  );
  assert.match(
    proposal.files["release.yml"],
    /contents: write\n {6}actions: read/,
  );
  assert.equal(
    proposal.files["release.yml"].match(/if: steps.version.outputs.changed/g)
      .length,
    2,
  );
  assert.match(
    proposal.files["release.yml"],
    /build:\n {4}steps:\n {6}- run: cargo test/,
  );
});

test("npm and PyPI conversions remove publishing credentials and retain build steps", async () => {
  const { workflowMigrationProposal } =
    await import("../src/workflow-migration.mjs");
  for (const [registry, command, secret, expected] of [
    ["npm", "npm publish", "NPM_TOKEN", /NPM_CONFIG_PROVENANCE: 'true'/],
    [
      "pypi",
      "twine upload dist/*",
      "PYPI_TOKEN",
      /pypa\/gh-action-pypi-publish/,
    ],
  ]) {
    const contents = `on: push\njobs:\n  publish:\n    steps:\n      - run: make build\n      - run: ${command}\n        env:\n          ${secret}: \${{ secrets.${secret} }}\n`;
    const result = await workflowMigrationProposal(
      root,
      [{ registry }],
      [{ name: "release.yml", contents }],
    );
    assert.match(result.files["release.yml"], expected);
    assert.match(result.files["release.yml"], /make build/);
    assert.doesNotMatch(
      result.files["release.yml"],
      new RegExp(`secrets\\.${secret}`),
    );
  }
});

test("crates authentication preserves multiline guards and refuses different publishing guards", async () => {
  const { workflowMigrationProposal } =
    await import("../src/workflow-migration.mjs");
  const contents =
    "on: push\njobs:\n  release:\n    steps:\n      - name: Publish first crate\n        if: >-\n          steps.first.outputs.changed == 'true'\n        run: cargo publish -p first\n";
  const propose = (text) =>
    workflowMigrationProposal(
      root,
      [{ registry: "crates-io" }],
      [{ name: "release.yml", contents: text }],
    );
  const converted = await propose(contents);
  assert.equal(
    converted.files["release.yml"].match(/steps.first.outputs.changed/g).length,
    2,
  );
  await assert.rejects(
    propose(
      `${contents}      - name: Publish second crate\n        if: steps.second.outputs.changed == 'true'\n        run: cargo publish -p second\n`,
    ),
    /different publishing conditions/,
  );
});

test("PyPI conversion retains step identity, condition, package directory and unrelated environment", async () => {
  const { workflowMigrationProposal } =
    await import("../src/workflow-migration.mjs");
  const contents =
    "on: push\njobs:\n  publish:\n    steps:\n      - name: Upload Python package\n        id: publish\n        if: steps.release.outputs.version != ''\n        run: python -m twine upload python/dist/*\n        env:\n          TWINE_PASSWORD: ${{ secrets.PYPI_TOKEN }}\n          OTHER: retained\n";
  const result = await workflowMigrationProposal(
    root,
    [{ registry: "pypi" }],
    [{ name: "release.yml", contents }],
  );
  assert.match(result.files["release.yml"], /id: publish/);
  assert.match(
    result.files["release.yml"],
    /if: steps.release.outputs.version/,
  );
  assert.match(result.files["release.yml"], /packages-dir: 'python\/dist\/'/);
  assert.match(result.files["release.yml"], /OTHER: retained/);
  const action = contents.replace(
    "run: python -m twine upload python/dist/*",
    "uses: pypa/gh-action-pypi-publish@release/v1\n        with:\n          password: ${{ secrets.PYPI_TOKEN }}\n          packages-dir: python/dist/",
  );
  const converted = await workflowMigrationProposal(
    root,
    [{ registry: "pypi" }],
    [{ name: "release.yml", contents: action }],
  );
  assert.doesNotMatch(
    converted.files["release.yml"],
    /password:|secrets.PYPI_TOKEN/,
  );
  assert.match(converted.files["release.yml"], /packages-dir: python\/dist\//);
  await assert.rejects(
    workflowMigrationProposal(
      root,
      [{ registry: "pypi" }],
      [
        {
          name: "release.yml",
          contents: contents.replace(
            "twine upload python/dist/*",
            "twine upload --skip-existing python/dist/*",
          ),
        },
      ],
    ),
    /simple twine/,
  );
});

test("migration never removes another token consumer and bounds reusable cycles", async () => {
  const { workflowMigrationProposal } =
    await import("../src/workflow-migration.mjs");
  const contents =
    "on: push\njobs:\n  release:\n    steps:\n      - run: cargo publish\n  other:\n    steps:\n      - run: echo ready\n        env:\n          OTHER: ${{ secrets.CARGO_TOKEN }}\n";
  await assert.rejects(
    workflowMigrationProposal(
      root,
      [{ registry: "crates-io" }],
      [{ name: "release.yml", contents }],
    ),
    /non-publishing/,
  );
  const cycle =
    "on: workflow_call\njobs:\n  recurse:\n    uses: ./.github/workflows/cycle.yml\n";
  await assert.rejects(
    workflowMigrationProposal(
      root,
      [{ registry: "crates-io" }],
      [{ name: "cycle.yml", contents: cycle }],
    ),
    /cyclic/,
  );
});

test("migrate and add-publish-job convert existing token publication before attaching trust", async () => {
  const inspection = await inspectRepository(root);
  for (const options of [{ migrate: true }, { addPublishJob: true }]) {
    const [plan] = buildPlans(inspection, ["crates-io"], options);
    assert.equal(plan.steps[0].id, "add-publishing-workflow");
    assert.match(plan.steps[0].description, /convert/i);
  }
});

test("migration rewrites both workflows, preserves release commands, pins auth and drops token wiring", async () => {
  const { workflowMigrationProposal } =
    await import("../src/workflow-migration.mjs");
  const inspection = await inspectRepository(root);
  const proposal = await workflowMigrationProposal(
    root,
    inspection.packages,
    await readWorkflows(root),
  );
  assert.deepEqual(
    proposal.files,
    JSON.parse(await readFile(`${root}/expected.json`, "utf8")),
  );
  assert.deepEqual(Object.keys(proposal.files).sort(), ["js.yml", "rust.yml"]);
  const caller = proposal.files["js.yml"];
  const callee = proposal.files["rust.yml"];
  assert.match(caller, /id-token: write/);
  assert.match(callee, /id-token: write/);
  assert.match(callee, /rust-lang\/crates-io-auth-action@[a-f0-9]{40}/);
  assert.match(
    callee,
    /CARGO_REGISTRY_TOKEN: \$\{\{ steps\.prm_auth\.outputs\.token \}\}/,
  );
  assert.match(callee, /environment: release-main/);
  assert.match(callee, /run: node scripts\/publish-to-crates.mjs/);
  assert.doesNotMatch(caller + callee, /secrets\.CARGO_TOKEN|CARGO_TOKEN:/);
});

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
    assert.match(contents, /secrets.CARGO_TOKEN/);
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

test("npm and PyPI conversions isolate OIDC credentials and retain fallback and build steps", async () => {
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
    assert.match(
      result.files["release.yml"],
      new RegExp(`secrets\\.${secret}`),
    );
    assert.match(result.files["release.yml"], /outcome == 'failure'/);
  }
});

test("crates authentication preserves multiline guards and authenticates distinct publishing guards", async () => {
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
  const distinct = await propose(
    `${contents}      - name: Publish second crate\n        if: steps.second.outputs.changed == 'true'\n        run: cargo publish -p second\n`,
  );
  assert.match(distinct.files["release.yml"], /prm_auth_2.outputs.token/);
  assert.equal(
    distinct.files["release.yml"].match(/steps.second.outputs.changed/g).length,
    2,
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
  assert.equal(converted.files["release.yml"].match(/password:/g).length, 1);
  assert.match(
    converted.files["release.yml"],
    /password:.*secrets.PYPI_API_TOKEN/,
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
  const kept = await workflowMigrationProposal(
    root,
    [{ registry: "crates-io" }],
    [{ name: "release.yml", contents }],
  );
  assert.match(kept.files["release.yml"], /OTHER: \$\{\{ secrets.CARGO_TOKEN/);
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

test("migration rewrites both workflows, preserves release commands, pins auth and retains fallback wiring", async () => {
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
    /CARGO_REGISTRY_TOKEN: \$\{\{ steps\.prm_auth\.outputs\.token \|\| secrets.CARGO_REGISTRY_TOKEN \|\| secrets.CARGO_TOKEN \}\}/,
  );
  assert.match(callee, /environment: release-main/);
  assert.match(callee, /run: node scripts\/publish-to-crates.mjs/);
  assert.match(caller + callee, /secrets\.CARGO_TOKEN/);
});

test("OIDC-first migration preserves fallback secrets, inline tokens, inherit and preflights (#60/#62)", async () => {
  const { workflowMigrationProposal } =
    await import("../src/workflow-migration.mjs");
  const workflows = [
    {
      name: "ci.yml",
      contents:
        "on: push\nenv:\n  CARGO_TOKEN: ${{ secrets.CARGO_TOKEN }}\njobs:\n  check:\n    steps:\n      - run: node scripts/publish-preflight.mjs crates\n        env:\n          CARGO_TOKEN: ${{ secrets.CARGO_TOKEN }}\n  call:\n    uses: ./.github/workflows/rust.yml\n    secrets: inherit\n",
    },
    {
      name: "rust.yml",
      contents:
        "on:\n  workflow_call:\n    secrets:\n      CARGO_TOKEN:\n        required: true\njobs:\n  release:\n    steps:\n      - run: bash scripts/preflight-publish.sh crates\n        env:\n          CARGO_TOKEN: ${{ secrets.CARGO_TOKEN }}\n      - run: cargo publish --token '${{ secrets.CARGO_TOKEN }}'\n",
    },
  ];
  const result = await workflowMigrationProposal(
    root,
    [{ registry: "crates-io", workflow: "ci.yml" }],
    workflows,
  );
  assert.match(result.files["ci.yml"], /secrets: inherit/);
  const rust = result.files["rust.yml"];
  assert.match(rust, /required: false/);
  assert.match(rust, /continue-on-error: true/);
  assert.match(
    rust,
    /steps.prm_auth.outputs.token \|\| secrets.CARGO_REGISTRY_TOKEN \|\| secrets.CARGO_TOKEN/,
  );
  assert.doesNotMatch(rust, /--token/);
  assert.ok(
    rust.indexOf("uses: rust-lang") <
      rust.indexOf("run: bash scripts/preflight"),
  );
  assert.match(rust, /# v1.0.5/);
});

test("distinct guards receive separate auth and existing mixed auth remains convertible (#60)", async () => {
  const { workflowMigrationProposal } =
    await import("../src/workflow-migration.mjs");
  const contents =
    "on: push\njobs:\n  release:\n    steps:\n      - uses: rust-lang/crates-io-auth-action@v1\n        id: auth\n        if: vars.CRATES_IO_TRUSTED_PUBLISHING == 'true'\n        continue-on-error: true\n      - run: cargo publish -p first\n        if: steps.first.outputs.changed == 'true'\n        env:\n          CARGO_REGISTRY_TOKEN: ${{ steps.auth.outputs.token || secrets.CARGO_TOKEN }}\n      - run: cargo publish -p second\n        if: steps.second.outputs.changed == 'true'\n";
  const result = await workflowMigrationProposal(
    root,
    [{ registry: "crates-io" }],
    [{ name: "release.yml", contents }],
  );
  assert.match(result.files["release.yml"], /CARGO_TOKEN/);
  assert.match(result.files["release.yml"], /prm_auth_2.outputs.token/);
});

test("shell substitutions and executed string-built scripts are detected (#60)", async () => {
  const { mkdtemp, mkdir, writeFile, rm } = await import("node:fs/promises");
  const os = await import("node:os");
  const temporary = await mkdtemp(`${os.tmpdir()}/prm-script-`);
  try {
    await mkdir(`${temporary}/scripts`);
    await writeFile(
      `${temporary}/scripts/publish.mjs`,
      "let command = 'cargo publish --allow-dirty';\ncommand += ' --verbose';\nexecSync(command);\n",
    );
    for (const command of [
      "if ! OUT=$(cargo publish 2>&1); then exit 1; fi",
      "OUT=`cargo publish`",
      "node scripts/publish.mjs",
    ]) {
      const result = await detectPublisher(
        temporary,
        [
          {
            name: "release.yml",
            contents: `jobs:\n  release:\n    steps:\n      - run: ${command}\n`,
          },
        ],
        "crates-io",
      );
      assert.equal(result.workflow, "release.yml", command);
    }
    for (const quote of ["'", "`"]) {
      await writeFile(
        `${temporary}/scripts/publish.mjs`,
        `const command = ${quote}cargo publish${quote};\nexecSync(command);\n`,
      );
      const executed = await detectPublisher(
        temporary,
        [
          {
            name: "release.yml",
            contents:
              "jobs:\n  release:\n    steps:\n      - run: node scripts/publish.mjs\n",
          },
        ],
        "crates-io",
      );
      assert.equal(executed.workflow, "release.yml", quote);
      await writeFile(
        `${temporary}/scripts/publish.mjs`,
        `const message = ${quote}cargo publish${quote};\nconsole.log(message);\n`,
      );
      const result = await detectPublisher(
        temporary,
        [
          {
            name: "release.yml",
            contents:
              "jobs:\n  release:\n    steps:\n      - run: node scripts/publish.mjs\n",
          },
        ],
        "crates-io",
      );
      assert.equal(result.workflow, null, quote);
    }
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
});

test("declares reusable fallback secrets and preserves quoted names and step ids (#62)", async () => {
  const { workflowMigrationProposal } =
    await import("../src/workflow-migration.mjs");
  {
    const contents =
      "on:\n  workflow_call:\n    secrets:\n      CARGO_TOKEN:\n        required: true\njobs:\n  publish:\n    steps:\n      - run: cargo publish\n";
    const result = await workflowMigrationProposal(
      root,
      [{ registry: "crates-io", workflow: "rust.yml" }],
      [{ name: "rust.yml", contents }],
    );
    assert.match(
      result.files["rust.yml"],
      /CARGO_REGISTRY_TOKEN:\n\s+required: false/,
    );
    const npm = await workflowMigrationProposal(
      root,
      [{ registry: "npm", workflow: "npm.yml" }],
      [
        {
          name: "npm.yml",
          contents:
            "on: push\njobs:\n  publish:\n    steps:\n      - id: upload\n        name: 'Upload package'\n        run: npm publish\n",
        },
      ],
    );
    assert.equal((npm.files["npm.yml"].match(/id: upload\n/g) ?? []).length, 1);
    assert.match(npm.files["npm.yml"], /name: 'Upload package \(OIDC\)'/);
  }
});

test("inspection does not combine OIDC and fallback evidence from unrelated jobs (#62)", async () => {
  const { detectPublisher } = await import("../src/publishers.mjs");
  const result = await detectPublisher(
    process.cwd(),
    [
      {
        name: "release.yml",
        contents:
          "on: push\njobs:\n  publish:\n    steps:\n      - run: cargo publish\n        env:\n          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_TOKEN }}\n  unrelated:\n    permissions:\n      id-token: write\n    steps:\n      - uses: rust-lang/crates-io-auth-action@v1\n        continue-on-error: true\n        env:\n          TOKEN: ${{ steps.auth.outputs.token || secrets.CARGO_TOKEN }}\n",
      },
    ],
    "crates-io",
  );
  assert.equal(result.policy, "token-only");
});

test("workflow-level Cargo credentials are overridden on every consumer (#60)", async () => {
  const { workflowMigrationProposal } =
    await import("../src/workflow-migration.mjs");
  const contents =
    "on: push\nenv:\n  CARGO_TOKEN: ${{ secrets.CARGO_TOKEN }}\njobs:\n  publish:\n    steps:\n      - run: cargo publish\n";
  const result = await workflowMigrationProposal(
    root,
    [{ registry: "crates-io" }],
    [{ name: "release.yml", contents }],
  );
  assert.match(
    result.files["release.yml"],
    /run: cargo publish\n\s+env:\n\s+CARGO_TOKEN: \$\{\{ steps.prm_auth.outputs.token/,
  );
});

test("migration preserves an already converted npm job while converting another", async () => {
  const { workflowMigrationProposal } =
    await import("../src/workflow-migration.mjs");
  const packages = [{ registry: "npm", workflow: "release.yml" }];
  const contents =
    "on: push\njobs:\n  first:\n    steps:\n      - run: npm publish\n        env:\n          NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}\n";
  const first = (
    await workflowMigrationProposal(root, packages, [
      { name: "release.yml", contents },
    ])
  ).files["release.yml"];
  const second = (
    await workflowMigrationProposal(root, packages, [
      {
        name: "release.yml",
        contents: `${first}  second:\n    steps:\n      - run: npm publish\n        env:\n          NODE_AUTH_TOKEN: \${{ secrets.NPM_TOKEN }}\n`,
      },
    ])
  ).files["release.yml"];
  assert.equal(second.split("  second:")[0], first);
  assert.equal((second.match(/outcome == 'failure'/g) ?? []).length, 2);
});

test("inspection flags variable-gated OIDC so migration can normalize it", async () => {
  const { detectPublisher } = await import("../src/publishers.mjs");
  const contents =
    "on: push\njobs:\n  release:\n    permissions:\n      id-token: write\n    steps:\n      - uses: rust-lang/crates-io-auth-action@v1\n        id: auth\n        if: vars.OIDC_ENABLED == 'true'\n        continue-on-error: true\n      - run: cargo publish\n        env:\n          CARGO_REGISTRY_TOKEN: ${{ steps.auth.outputs.token || secrets.CARGO_TOKEN }}\n";
  assert.notEqual(
    (
      await detectPublisher(
        root,
        [{ name: "release.yml", contents }],
        "crates-io",
      )
    ).policy,
    "oidc-with-token-fallback",
  );
});

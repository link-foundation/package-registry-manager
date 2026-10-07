import assert from "node:assert/strict";
import { mkdir, mkdtemp, rm, symlink, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { after, test } from "node:test";

import { inspectRepository } from "../src/discovery.mjs";
import { buildPlans } from "../src/plan.mjs";
import { detectPublisher, parseWorkflow } from "../src/publishers.mjs";
import { readWorkflows } from "../src/workflows.mjs";

const temporaries = [];

after(async () => {
  await Promise.all(
    temporaries.map((item) => rm(item, { recursive: true, force: true })),
  );
});

async function repository(files) {
  const root = await mkdtemp(path.join(os.tmpdir(), "prm-publishers-"));
  temporaries.push(root);
  const all = {
    ".git/config":
      '[remote "origin"]\n\turl = https://github.com/acme/tool.git\n',
    ...files,
  };
  for (const [name, contents] of Object.entries(all)) {
    await mkdir(path.dirname(path.join(root, name)), { recursive: true });
    await writeFile(path.join(root, name), contents);
  }
  return root;
}

// The layout of package-registry-manager itself (issue #16): npm publishes
// from release.yml through a script, and desktop-release.yml sorts first.
const SELF_LAYOUT = {
  "js/package.json": '{"name": "tool", "version": "1.0.0"}\n',
  "rust/Cargo.toml": '[package]\nname = "tool"\nversion = "1.0.0"\n',
  "js/scripts/publish-to-npm.mjs": [
    'import { spawnSync } from "node:child_process";',
    "function npm(args) {",
    '  return spawnSync("npm", args, { stdio: "inherit" });',
    "}",
    'npm(["publish", "--access", "public", "--provenance"]);',
    "",
  ].join("\n"),
  "rust/scripts/publish-crate.rs": [
    "// `cargo publish` is mentioned here only in a comment.",
    'let mut cmd = Command::new("cargo");',
    'cmd.arg("publish").arg("--allow-dirty");',
    "",
  ].join("\n"),
  "rust/scripts/preflight.sh": [
    "#!/usr/bin/env bash",
    "# cargo publish would fail without a token",
    "echo 'no credential -- cargo publish would fail with 401'",
    "",
  ].join("\n"),
  ".github/workflows/desktop-release.yml": [
    "name: Desktop release",
    "on: workflow_dispatch",
    "jobs:",
    "  finalize:",
    "    runs-on: ubuntu-latest",
    "    permissions:",
    "      id-token: write",
    "    steps:",
    "      - run: gh release upload v1 app.dmg",
    "",
  ].join("\n"),
  ".github/workflows/release.yml": [
    "name: Release",
    "on:",
    "  push:",
    "    branches: [main]",
    "permissions:",
    "  contents: read",
    "jobs:",
    "  # npm publish is not run by the preflight",
    "  release-preflight:",
    "    runs-on: ubuntu-latest",
    "    steps:",
    "      - run: bash rust/scripts/preflight.sh",
    "  auto-release:",
    "    runs-on: ubuntu-latest",
    "    permissions:",
    "      contents: write",
    "      id-token: write",
    "    steps:",
    "      - run: rust-script rust/scripts/publish-crate.rs",
    "  javascript-release:",
    "    runs-on: ubuntu-latest",
    "    environment: npm",
    "    permissions: { contents: read, id-token: write }",
    "    steps:",
    "      - name: Publish and verify npm package",
    "        run: node js/scripts/publish-to-npm.mjs",
    "",
  ].join("\n"),
};

test("finds the job that publishes to npm through a script (#16)", async () => {
  const root = await repository(SELF_LAYOUT);
  const inspection = await inspectRepository(root);
  const npm = inspection.packages.find((item) => item.registry === "npm");
  assert.equal(npm.workflow, "release.yml");
  assert.deepEqual(npm.workflow_jobs, ["javascript-release"]);
  assert.equal(npm.environment, "npm");
  const crate = inspection.packages.find(
    (item) => item.registry === "crates-io",
  );
  assert.equal(crate.workflow, "release.yml");
  assert.deepEqual(crate.workflow_jobs, ["auto-release"]);
  assert.equal(inspection.repository.release_workflow, "release.yml");

  const [plan] = buildPlans(inspection, ["npm"]);
  assert.deepEqual(plan.trusted_publisher, {
    provider: "github-actions",
    organization: "acme",
    repository: "tool",
    workflow: "release.yml",
    environment: "npm",
  });
  const attach = plan.steps.find(
    (step) => step.id === "attach-trusted-publisher",
  );
  assert.deepEqual(
    attach.command.args.slice(
      attach.command.args.indexOf("--file"),
      attach.command.args.indexOf("--allow-publish"),
    ),
    ["--file", "release.yml", "--env", "npm"],
  );
});

test("never falls back to a workflow named like a release", async () => {
  const root = await repository({
    "package.json": '{"name": "tool", "version": "1.0.0"}\n',
    ".github/workflows/desktop-release.yml":
      SELF_LAYOUT[".github/workflows/desktop-release.yml"],
  });
  const inspection = await inspectRepository(root);
  assert.equal(inspection.repository.release_workflow, null);
  assert.equal(inspection.packages[0].workflow, undefined);
  const [plan] = buildPlans(inspection, ["npm"]);
  assert.equal(plan.trusted_publisher, undefined);
});

test("recognizes the common npm publish commands", async () => {
  const commands = [
    "npm publish --provenance",
    "npm stage publish",
    "pnpm -r publish --no-git-checks",
    "yarn npm publish",
    "npx changeset publish",
  ];
  for (const command of commands) {
    const workflows = [
      {
        name: "ci.yml",
        contents: `on: push\npermissions:\n  id-token: write\njobs:\n  ship:\n    runs-on: ubuntu-latest\n    steps:\n      - run: ${command}\n`,
      },
    ];
    const result = await detectPublisher("/nonexistent", workflows, "npm");
    assert.equal(result.workflow, "ci.yml", command);
    assert.deepEqual(result.jobs, ["ship"], command);
  }
  const dryRun = await detectPublisher(
    "/nonexistent",
    [
      {
        name: "ci.yml",
        contents:
          "on: push\njobs:\n  check:\n    steps:\n      - run: npm publish --dry-run\n",
      },
    ],
    "npm",
  );
  assert.equal(dryRun.workflow, null);
});

test("follows changesets/action and package scripts", async () => {
  const root = await repository({
    "package.json":
      '{"name": "tool", "version": "1.0.0", "scripts": {"release": "changeset publish"}}\n',
    ".github/workflows/version.yml": [
      "on: push",
      "jobs:",
      "  version:",
      "    permissions:",
      "      id-token: write",
      "    steps:",
      "      - uses: changesets/action@v1",
      "        with:",
      "          publish: pnpm release",
      "",
    ].join("\n"),
  });
  const result = await detectPublisher(root, await readWorkflows(root), "npm");
  assert.equal(result.workflow, "version.yml");
  assert.deepEqual(result.jobs, ["version"]);
});

test("follows a package script that a release script runs", async () => {
  // The layout of the link-foundation pipeline templates, e.g. browser-commander.
  const root = await repository({
    "js/package.json":
      '{"name": "tool", "version": "1.0.0", "scripts": {"changeset:publish": "changeset publish"}}\n',
    "js/scripts/publish-to-npm.mjs": [
      'import { $ } from "command-stream";',
      "// npm run changeset:publish --dry-run is not run",
      "await $`npm run changeset:publish`;",
      "",
    ].join("\n"),
    ".github/workflows/js.yml": [
      "on: push",
      "defaults:",
      "  run:",
      "    working-directory: js",
      "jobs:",
      "  release:",
      "    permissions:",
      "      id-token: write",
      "    steps:",
      "      - run: node scripts/publish-to-npm.mjs --should-pull",
      "",
    ].join("\n"),
  });
  const result = await detectPublisher(root, await readWorkflows(root), "npm");
  assert.equal(result.workflow, "js.yml");
  assert.deepEqual(result.jobs, ["release"]);
});

test("follows scripts when the checkout is reached through a symlink", async () => {
  // macOS's temporary directory is /var -> /private/var, and Windows may
  // name it with an 8.3 short name; realpath differs from the path given.
  const real = await repository({
    "package.json": '{"name": "tool", "version": "1.0.0"}\n',
    "scripts/publish.mjs": "await $`npm publish`;\n",
    ".github/workflows/release.yml": [
      "on: push",
      "jobs:",
      "  release:",
      "    permissions:",
      "      id-token: write",
      "    steps:",
      "      - run: node scripts/publish.mjs",
      "",
    ].join("\n"),
  });
  const root = `${real}-link`;
  temporaries.push(root);
  await symlink(real, root, "junction");
  const result = await detectPublisher(root, await readWorkflows(root), "npm");
  assert.equal(result.workflow, "release.yml");
});

test("recognizes the changesets/action publish sub-action", async () => {
  const workflows = [
    {
      name: "publish.yml",
      contents:
        "on: push\njobs:\n  publish:\n    permissions:\n      id-token: write\n    steps:\n      - uses: changesets/action/publish@ae32849d5ba541f9ae29e40e22a623bc13562f51 # v2.1.2\n",
    },
  ];
  const result = await detectPublisher("/nonexistent", workflows, "npm");
  assert.equal(result.workflow, "publish.yml");
  assert.deepEqual(result.jobs, ["publish"]);
});

test("names the caller of a reusable publishing workflow", async () => {
  const root = await repository({
    "package.json": '{"name": "tool", "version": "1.0.0"}\n',
    ".github/workflows/publish.yml": [
      "on:",
      "  workflow_call:",
      "jobs:",
      "  publish:",
      "    permissions:",
      "      id-token: write",
      "    steps:",
      "      - run: npm publish",
      "",
    ].join("\n"),
    ".github/workflows/release.yml": [
      "on:",
      "  push:",
      "jobs:",
      "  npm:",
      "    permissions:",
      "      id-token: write",
      "    uses: ./.github/workflows/publish.yml",
      "",
    ].join("\n"),
  });
  const result = await detectPublisher(root, await readWorkflows(root), "npm");
  assert.equal(result.workflow, "release.yml");
  assert.deepEqual(result.jobs, ["npm"]);
});

test("stops and asks when several workflows publish", async () => {
  const publish = (job) =>
    `on: push\njobs:\n  ${job}:\n    permissions:\n      id-token: write\n    steps:\n      - run: npm publish\n`;
  const root = await repository({
    "package.json": '{"name": "tool", "version": "1.0.0"}\n',
    ".github/workflows/a.yml": publish("one"),
    ".github/workflows/b.yml": publish("two"),
  });
  const inspection = await inspectRepository(root);
  const [npm] = inspection.packages;
  assert.equal(npm.workflow, undefined);
  assert.deepEqual(npm.workflow_candidates, ["a.yml", "b.yml"]);
  assert.match(npm.warnings[0], /pass --workflow <file>/);
  assert.equal(inspection.repository.release_workflow, null);

  const [skipped] = buildPlans(inspection, ["npm"]);
  assert.deepEqual(skipped.steps, []);
  assert.match(skipped.skipped_reason, /several workflows publish to npm/);

  const [chosen] = buildPlans(inspection, ["npm"], {
    workflow: "b.yml",
    publisherEnvironment: "release",
  });
  assert.equal(chosen.skipped_reason, undefined);
  assert.equal(chosen.trusted_publisher.workflow, "b.yml");
  assert.equal(chosen.trusted_publisher.environment, "release");
});

test("warns when the publishing job cannot mint an OIDC token", async () => {
  const workflows = [
    {
      name: "release.yml",
      contents:
        "on: push\npermissions:\n  contents: write\njobs:\n  publish:\n    steps:\n      - run: npm publish\n",
    },
  ];
  const result = await detectPublisher("/nonexistent", workflows, "npm");
  assert.equal(result.workflow, "release.yml");
  assert.match(result.warnings[0], /without `id-token: write`/);
});

test("warns about long-lived registry token secrets", async () => {
  const root = await repository({
    ...SELF_LAYOUT,
    ".github/workflows/token.yml": [
      "on: push",
      "jobs:",
      "  publish:",
      "    steps:",
      "      - run: cargo publish --dry-run",
      "        env:",
      "          CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_TOKEN }}",
      "",
    ].join("\n"),
  });
  const inspection = await inspectRepository(root);
  const crate = inspection.packages.find(
    (item) => item.registry === "crates-io",
  );
  assert.deepEqual(crate.token_secrets, ["CARGO_TOKEN"]);
  assert.match(
    crate.warnings.join("\n"),
    /token\.yml reads secrets\.CARGO_TOKEN; publish with crates\.io trusted publishing/,
  );
});

test("splits jobs by indentation and ignores comments", () => {
  const parsed = parseWorkflow(
    "on: push\njobs:\n    # a comment\n    first:\n        steps: []\n    second-job:\n        runs-on: x\nenv:\n  A: b\n",
  );
  assert.deepEqual(
    parsed.jobs.map((job) => job.name),
    ["first", "second-job"],
  );
  assert.deepEqual(parsed.header, ["on: push", "env:", "  A: b", ""]);
});

test("follows node, bash and Python scripts running python -m twine (#33)", async () => {
  for (const [runner, script, contents] of [
    [
      "node",
      "scripts/publish-to-pypi.mjs",
      "await $`cd python && python -m twine upload dist/*`;",
    ],
    ["bash", "scripts/publish.sh", "python -m twine upload dist/*"],
    [
      "python",
      "scripts/publish.py",
      'subprocess.run(["python", "-m", "twine", "upload", "dist/*"])',
    ],
  ]) {
    const root = await repository({
      "python/pyproject.toml": '[project]\nname="tool"\nversion="1.0.0"\n',
      [script]: contents,
      ".github/workflows/release.yml": `on: workflow_dispatch\njobs:\n  python-release:\n    permissions: {id-token: write}\n    steps:\n      - run: ${runner} ${script}\n`,
    });
    const inspection = await inspectRepository(root);
    assert.equal(inspection.packages[0].workflow, "release.yml", runner);
    assert.deepEqual(inspection.packages[0].workflow_jobs, ["python-release"]);
  }
});

test("warns and blocks trusted-publisher setup without a publishing job (#33)", async () => {
  const root = await repository({ "package.json": '{"name":"tool"}' });
  const inspection = await inspectRepository(root);
  assert.match(
    inspection.packages[0].warnings.join("\n"),
    /no workflow publishes tool to npm; CI releases will not reach it/,
  );
  const [plan] = buildPlans(inspection);
  assert.match(plan.skipped_reason, /publishing job/);
  assert.equal(plan.trusted_publisher, undefined);
  assert.deepEqual(
    plan.steps.map((step) => step.id),
    ["add-publishing-workflow"],
  );
  const [override] = buildPlans(inspection, [], { workflow: "release.yml" });
  assert.equal(override.trusted_publisher.workflow, "release.yml");
});

test("credential preflight comments and inert strings are not publishers (#37)", async () => {
  for (const [script, contents] of [
    ["preflight.sh", "echo ready # npm publish\n"],
    ["preflight.py", "print('ready') # npm publish\n"],
    [
      "preflight.mjs",
      "/* credentials only\nnpm publish\n*/\nconst help = 'npm publish';\nconsole.log(help);\n",
    ],
  ]) {
    const root = await repository({
      [`scripts/${script}`]: contents,
      ".github/workflows/python.yml": `on: push\njobs:\n  preflight:\n    permissions: {id-token: write}\n    steps:\n      - run: ${script.endsWith("sh") ? "bash" : script.endsWith("py") ? "python" : "node"} scripts/${script}\n`,
    });
    assert.equal(
      (await detectPublisher(root, await readWorkflows(root), "npm")).workflow,
      null,
      script,
    );
  }
});

test("quoted URL and executed commands survive comment stripping (#37)", async () => {
  const root = await repository({
    "scripts/publish.mjs":
      'const url = "https://example.org/#anchor";\nawait exec("npm", ["publish", url]); // publish\n',
    ".github/workflows/js.yml":
      "on: push\njobs:\n  publish:\n    permissions: {id-token: write}\n    steps:\n      - run: node scripts/publish.mjs\n",
  });
  assert.equal(
    (await detectPublisher(root, await readWorkflows(root), "npm")).workflow,
    "js.yml",
  );
});

test("publishing coverage is checked for every wrapper manifest (#36, #37)", async () => {
  const root = await repository({
    "package.json": '{"name":"gh-upload","version":"1.0.0"}',
    "packages/gh-upload-log/package.json":
      '{"name":"gh-upload-log","version":"1.0.0","dependencies":{"gh-upload":"1.0.0"}}',
    "packages/orphan/package.json": '{"name":"orphan","version":"1.0.0"}',
    ".github/workflows/release.yml":
      "on: push\njobs:\n  release:\n    permissions: {id-token: write}\n    steps:\n      - run: npm publish\n      - run: npm publish\n        working-directory: packages/gh-upload-log\n",
  });
  const inspection = await inspectRepository(root);
  assert.equal(
    inspection.packages.find((item) => item.name === "gh-upload-log").workflow,
    "release.yml",
  );
  assert.equal(
    inspection.packages.find((item) => item.name === "gh-upload").workflow,
    "release.yml",
  );
  const orphan = inspection.packages.find((item) => item.name === "orphan");
  assert.equal(orphan.workflow, undefined);
  assert.match(
    buildPlans(inspection).find((plan) => plan.package.name === "orphan")
      .skipped_reason,
    /no workflow publishes/,
  );
});

test("inert workflow text and commented secret reads are not publishing (#37)", async () => {
  const root = await repository({});
  const workflows = [
    {
      name: "preflight.yml",
      contents: `on: workflow_dispatch
jobs:
  preflight:
    permissions: {id-token: write}
    steps:
      - name: npm publish
        env:
          HELP: npm publish
        run: |
          echo 'npm publish would fail'
          npm whoami # npm publish
`,
    },
  ];
  const detected = await detectPublisher(root, workflows, "npm");
  assert.equal(detected.workflow, null);
  const { tokenSecrets } = await import("../src/publishers.mjs");
  assert.deepEqual(
    tokenSecrets(
      [
        {
          name: "ci.yml",
          contents: "run: npm whoami # ${{ secrets.NPM_TOKEN }}",
        },
      ],
      "npm",
    ),
    [],
  );
});

test("warns when an npm wrapper stops tracking its main package version (#36)", async () => {
  const root = await repository({
    "package.json": '{"name":"gh-upload","version":"2.0.0"}',
    "packages/old/package.json":
      '{"name":"gh-upload-log","version":"1.0.0","dependencies":{"gh-upload":"1.0.0"}}',
    ".github/workflows/release.yml":
      "on: push\njobs:\n  release:\n    permissions: {id-token: write}\n    steps:\n      - run: npm publish\n      - run: npm publish\n        working-directory: packages/old\n",
  });
  const inspected = await inspectRepository(root);
  assert.ok(
    inspected.packages
      .find((item) => item.name === "gh-upload-log")
      .warnings.some(
        (warning) => warning.includes("wrapper") && warning.includes("2.0.0"),
      ),
  );
});

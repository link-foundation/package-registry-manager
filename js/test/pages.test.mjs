import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  chmod,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  writeFile,
} from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { after, test } from "node:test";

import { inspectRepository } from "../src/discovery.mjs";
import { pagesState, pagesWorkflow } from "../src/pages.mjs";
import { buildPlans } from "../src/plan.mjs";
import { executePlan } from "../src/setup.mjs";

const FAKE_TOOL = fileURLToPath(
  new URL("../../tests/fixtures/fake-tools/fake-tool.cjs", import.meta.url),
);
const temporary = await mkdtemp(path.join(os.tmpdir(), "prm-pages-"));

after(() => rm(temporary, { recursive: true, force: true }));

const DOCS_WORKFLOW = `name: Docs
on: push
jobs:
  deploy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/configure-pages@v6
      - uses: actions/upload-pages-artifact@v5
      - uses: actions/deploy-pages@v5
`;
const published = {
  registry: "npm",
  name: "demo",
  manifest: "package.json",
  publishable: true,
  exists_on_registry: true,
  trusted_publishing: true,
  workflow: "release.yml",
};
const repository = {
  github_owner: "acme",
  github_repository: "demo",
  pages_workflow: "docs.yml",
};
const ids = (plan) => plan.steps.map((step) => step.id);

test("finds the workflow that deploys to GitHub Pages (#16)", () => {
  assert.equal(
    pagesWorkflow([
      { name: "ci.yml", contents: "steps:\n  - uses: actions/checkout@v6\n" },
      { name: "docs.yml", contents: DOCS_WORKFLOW },
    ]),
    "docs.yml",
  );
  assert.equal(
    pagesWorkflow([
      { name: "ci.yml", contents: "# actions/deploy-pages is not used\n" },
    ]),
    null,
  );
});

test("reads the Pages site from gh api (#16)", () => {
  const site = (build_type) => ({
    code: 0,
    stdout: JSON.stringify({ build_type }),
  });
  assert.equal(pagesState(site("workflow")), "workflow");
  assert.equal(pagesState(site("legacy")), "legacy");
  assert.equal(
    pagesState({
      code: 1,
      stdout: '{"message":"Not Found","status":"404"}',
      stderr: "gh: Not Found (HTTP 404)",
    }),
    "missing",
  );
  assert.equal(
    pagesState({ code: 1, stdout: "", stderr: "gh: Forbidden (HTTP 403)" }),
    undefined,
  );
});

test("records the Pages workflow in the inspection (#16)", async () => {
  const root = await mkdtemp(path.join(temporary, "inspect-"));
  await mkdir(path.join(root, ".github/workflows"), { recursive: true });
  await writeFile(path.join(root, ".github/workflows/docs.yml"), DOCS_WORKFLOW);
  await writeFile(
    path.join(root, "package.json"),
    JSON.stringify({ name: "demo", version: "1.0.0" }),
  );
  const inspection = await inspectRepository(root);
  assert.equal(inspection.repository.pages_workflow, "docs.yml");
  await rm(path.join(root, ".github/workflows/docs.yml"));
  const without = await inspectRepository(root);
  assert.equal("pages_workflow" in without.repository, false);
});

test("checks Pages in every plan mode once a workflow deploys there (#16)", () => {
  const [complete] = buildPlans({ repository, packages: [published] });
  assert.equal(complete.mode, "complete");
  assert.deepEqual(ids(complete), [
    "check-pages",
    "enable-pages",
    "use-pages-workflow",
  ]);
  const [enable, switchSource] = complete.steps.slice(1);
  assert.deepEqual(enable.command.args, [
    "api",
    "-X",
    "POST",
    "repos/acme/demo/pages",
    "-f",
    "build_type=workflow",
  ]);
  assert.equal(enable.when, "pages-missing");
  assert.equal(enable.confirm, true);
  assert.equal(switchSource.command.args[2], "PUT");
  assert.equal(switchSource.when, "pages-legacy");

  const [attach] = buildPlans({
    repository,
    packages: [{ ...published, trusted_publishing: false }],
  });
  assert.deepEqual(ids(attach).slice(-3), ids(complete));
  const [noPages] = buildPlans({
    repository: { ...repository, pages_workflow: undefined },
    packages: [published],
  });
  assert.deepEqual(ids(noPages), []);
  const [noSlug] = buildPlans({
    repository: { ...repository, github_owner: null },
    packages: [published],
  });
  assert.deepEqual(ids(noSlug), []);
});

const POSIX_ONLY = {
  skip: process.platform === "win32" && "fake tools are POSIX scripts",
};

async function runPagesSetup(site, answer) {
  const state = await mkdtemp(path.join(temporary, "state-"));
  const bin = path.join(state, "bin");
  await mkdir(bin);
  const file = path.join(bin, "gh");
  await writeFile(
    file,
    `#!${process.execPath}\n${await readFile(FAKE_TOOL, "utf8")}`,
  );
  await chmod(file, 0o755);
  const [plan] = buildPlans({ repository, packages: [published] });
  const saved = { ...process.env };
  Object.assign(process.env, {
    PATH: `${bin}${path.delimiter}${process.env.PATH}`,
    FAKE_STATE: state,
    FAKE_PAGES: site,
  });
  const lines = [];
  const prompts = [];
  const originalLog = console.log;
  console.log = (...values) => lines.push(values.join(" "));
  try {
    await executePlan(plan, {
      repository: state,
      execute: true,
      noBrowser: true,
      prompt: async (message) => {
        prompts.push(message);
        return answer;
      },
    });
  } finally {
    console.log = originalLog;
    process.env = saved;
  }
  const commands = readFileSync(path.join(state, "log.jsonl"), "utf8")
    .trim()
    .split("\n")
    .map((line) => JSON.parse(line).argv.join(" "));
  return { commands, lines, prompts };
}

test(
  "enables GitHub Pages after a confirmation when it is missing (#16)",
  POSIX_ONLY,
  async () => {
    const { commands, lines, prompts } = await runPagesSetup("missing", "y");
    assert.deepEqual(commands, [
      "gh api repos/acme/demo/pages",
      "gh api -X POST repos/acme/demo/pages -f build_type=workflow",
    ]);
    assert.deepEqual(prompts, [
      "Run `gh api -X POST repos/acme/demo/pages -f build_type=workflow`? [y/N] ",
    ]);
    assert.ok(
      lines.includes(
        "demo already publishes through trusted publishing; only the repository checks remain.",
      ),
      lines.join("\n"),
    );
    assert.ok(
      lines.includes(
        "  GitHub Pages is not enabled, so the workflow's deployment fails with Not Found.",
      ),
    );
  },
);

test(
  "switches a branch-built Pages site to GitHub Actions (#16)",
  POSIX_ONLY,
  async () => {
    const { commands } = await runPagesSetup("legacy", "y");
    assert.deepEqual(commands, [
      "gh api repos/acme/demo/pages",
      "gh api -X PUT repos/acme/demo/pages -f build_type=workflow",
    ]);
  },
);

test(
  "leaves Pages alone when it already deploys from GitHub Actions (#16)",
  POSIX_ONLY,
  async () => {
    const { commands, lines, prompts } = await runPagesSetup("workflow", "y");
    assert.deepEqual(commands, ["gh api repos/acme/demo/pages"]);
    assert.deepEqual(prompts, []);
    assert.ok(
      lines.includes(
        "  GitHub Pages is enabled with GitHub Actions as its source.",
      ),
    );
  },
);

test(
  "changes nothing when enabling Pages is declined (#16)",
  POSIX_ONLY,
  async () => {
    const { commands, lines } = await runPagesSetup("missing", "n");
    assert.deepEqual(commands, ["gh api repos/acme/demo/pages"]);
    assert.ok(lines.includes("  skipped enable-pages"));
  },
);

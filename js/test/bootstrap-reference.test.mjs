import assert from "node:assert/strict";
import { test } from "node:test";
import { buildPlans } from "../src/plan.mjs";

test("bootstrap fetches a PR head before its manifest is merged (#36)", () => {
  const [plan] = buildPlans(
    {
      repository: {},
      packages: [
        {
          registry: "npm",
          name: "gh-upload",
          version: "1.0.0",
          manifest: "package.json",
          publishable: true,
          workflow: "release.yml",
          exists_on_registry: false,
        },
      ],
    },
    [],
    { ref: "https://github.com/acme/tool/pull/37" },
  );
  assert.deepEqual(
    plan.steps.find((step) => step.id === "fetch-default-branch").command.args,
    ["fetch", "origin", "refs/pull/37/head"],
  );
  assert.ok(
    plan.steps.findIndex((step) => step.id === "check-name-policy") <
      plan.steps.findIndex((step) => step.id === "sign-in"),
  );
  assert.ok(
    plan.steps.findIndex((step) => step.id === "publish-dry-run") <
      plan.steps.findIndex((step) => step.id === "first-publish"),
  );
});

test("unsafe bootstrap refs are rejected (#36)", () => {
  const inspection = {
    repository: {},
    packages: [
      {
        registry: "npm",
        name: "tool",
        manifest: "package.json",
        publishable: true,
        workflow: "release.yml",
      },
    ],
  };
  for (const ref of [
    "--upload-pack=bad",
    "branch:destination",
    "a b",
    "../bad",
  ]) {
    assert.throws(() => buildPlans(inspection, [], { ref }), /ref/);
  }
});

test("inspects an unmerged branch and reports the main version without changing HEAD (#36)", async () => {
  const { mkdtemp, writeFile, mkdir, rm } = await import("node:fs/promises");
  const { spawnSync } = await import("node:child_process");
  const os = await import("node:os");
  const path = await import("node:path");
  const { inspectReference } = await import("../src/bootstrap-reference.mjs");
  const temp = await mkdtemp(path.join(os.tmpdir(), "prm-ref-test-"));
  const root = path.join(temp, "source");
  await mkdir(root);
  const git = (...args) => {
    const result = spawnSync("git", args, { cwd: root, encoding: "utf8" });
    assert.equal(result.status, 0, result.stderr);
    return result.stdout.trim();
  };
  try {
    git("init", "-b", "main");
    git("config", "user.email", "test@example.test");
    git("config", "user.name", "Test");
    await writeFile(
      path.join(root, "package.json"),
      '{"name":"gh-upload-log","version":"1.0.0"}',
    );
    git("add", ".");
    git("commit", "-m", "main version");
    const main = git("rev-parse", "HEAD");
    git("checkout", "-b", "rename");
    await writeFile(
      path.join(root, "package.json"),
      '{"name":"gh-upload","version":"1.1.0"}',
    );
    git("add", ".");
    git("commit", "-m", "unmerged renamed package");
    git("checkout", "main");
    git("clone", "--bare", root, path.join(temp, "origin.git"));
    git("remote", "add", "origin", path.join(temp, "origin.git"));
    const inspection = await inspectReference(root, "rename", {});
    assert.equal(inspection.packages[0].name, "gh-upload");
    assert.ok(
      inspection.packages[0].warnings.some((warning) =>
        warning.includes("gh-upload-log@1.0.0"),
      ),
    );
    assert.equal(git("rev-parse", "HEAD"), main);
    assert.equal(
      git("worktree", "list", "--porcelain").split("worktree ").length - 1,
      1,
    );
  } finally {
    await rm(temp, { recursive: true, force: true });
  }
});

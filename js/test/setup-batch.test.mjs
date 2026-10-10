import assert from "node:assert/strict";
import { test } from "node:test";
import { executePlans } from "../src/setup.mjs";

const plan = (registry, name) => ({
  registry,
  package: { name, publishable: true, manifest: "package.json" },
  repository: {},
  steps: [
    {
      id: "visit",
      title: "Visit",
      kind: "browser",
      url: `https://${registry}.example/`,
    },
  ],
});

test("account setup reports blocked plans and still runs other repositories (#43)", async () => {
  const blocked = plan("npm", "blocked");
  blocked.skipped_reason = "publisher belongs to another repository";
  const calls = [];
  await assert.rejects(
    executePlans(
      [blocked, plan("pypi", "ready")],
      {
        accountScan: true,
        execute: true,
        repository: process.cwd(),
        browser: "automated",
        automation: {
          goto: async (url) => calls.push(url),
          close: async () => calls.push("close"),
        },
        prompt: async () => "",
      },
      false,
    ),
    /publisher belongs/,
  );
  assert.deepEqual(calls, ["https://pypi.example/", "close"]);
});

test("all packages share one browser and close it once (#33)", async () => {
  const calls = [];
  const outcomes = await executePlans(
    [plan("npm", "one"), plan("pypi", "two")],
    {
      execute: true,
      repository: process.cwd(),
      browser: "automated",
      automation: {
        goto: async (url) => calls.push(url),
        close: async () => calls.push("close"),
      },
      prompt: async () => "",
    },
    false,
  );
  assert.deepEqual(calls, [
    "https://npm.example/",
    "https://pypi.example/",
    "close",
  ]);
  assert.deepEqual(
    outcomes.map((item) => item.status),
    ["configured", "configured"],
  );
});

test("a failed package closes the shared browser and leaves later packages unrun (#33)", async () => {
  const calls = [];
  await assert.rejects(
    executePlans(
      [plan("npm", "one"), plan("pypi", "two")],
      {
        execute: true,
        repository: process.cwd(),
        browser: "automated",
        automation: {
          goto: async () => {
            calls.push("goto");
            throw new Error("navigation failed");
          },
          close: async () => calls.push("close"),
        },
        prompt: async () => "",
      },
      false,
    ),
    /navigation failed/,
  );
  assert.deepEqual(calls, ["goto", "close"]);
});

test("account workflow proposals target each repository and remaining plans share one browser (#43)", async () => {
  const missing = ["one", "two"].map((name) => ({
    ...plan("npm", name),
    repository: {
      root: `/snapshots/${name}`,
      github_owner: "team",
      github_repository: name,
    },
    steps: [{ id: "add-publishing-workflow" }],
  }));
  const ready = plan("pypi", "ready");
  ready.repository = {
    root: process.cwd(),
    github_owner: "team",
    github_repository: "ready",
  };
  const calls = [];
  const outcomes = await executePlans(
    [...missing, ready],
    {
      execute: true,
      accountScan: true,
      browser: "automated",
      offerWorkflow: async (plans, options) => {
        assert.equal(plans.length, 1);
        assert.equal(options.repository, plans[0].repository.root);
        calls.push(options.repository);
        return { status: "workflow-pr" };
      },
      automation: {
        goto: async (url) => calls.push(url),
        close: async () => calls.push("close"),
      },
      prompt: async () => "",
    },
    false,
  );
  assert.deepEqual(calls, [
    "/snapshots/one",
    "/snapshots/two",
    "https://pypi.example/",
    "close",
  ]);
  assert.deepEqual(
    outcomes.map((item) => item.status),
    ["workflow-pr", "workflow-pr", "configured"],
  );
});

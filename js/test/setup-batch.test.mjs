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

import assert from "node:assert/strict";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { after, test } from "node:test";

import { inspectRepository } from "../src/discovery.mjs";
import { ignoredBy, testDirectory } from "../src/skips.mjs";

const temporaries = [];

after(async () => {
  await Promise.all(
    temporaries.map((item) => rm(item, { recursive: true, force: true })),
  );
});

async function repository(files) {
  const root = await mkdtemp(path.join(os.tmpdir(), "prm-skips-"));
  temporaries.push(root);
  for (const [name, contents] of Object.entries(files)) {
    await mkdir(path.dirname(path.join(root, name)), { recursive: true });
    await writeFile(path.join(root, name), contents);
  }
  return root;
}

const npmPackage = (name) => `{"name": "${name}", "version": "1.0.0"}\n`;

test("skips test, fixture, and example manifests (#16)", async () => {
  const root = await repository({
    "package.json": npmPackage("tool"),
    "tests/fixtures/app/package.json": npmPackage("fixture-app"),
    "test/package.json": npmPackage("test-app"),
    "src/__fixtures__/package.json": npmPackage("snapshot"),
    "examples/demo/package.json": npmPackage("demo"),
    "examples/broken/package.json": "{ not json",
    "tests/fixtures/app/Dockerfile": "FROM scratch\n",
  });
  const inspection = await inspectRepository(root);
  assert.deepEqual(
    inspection.packages.map((item) => item.name),
    ["tool"],
  );
  assert.equal(inspection.skipped, undefined);

  const verbose = await inspectRepository(root, { includeSkipped: true });
  assert.deepEqual(
    verbose.skipped.map((item) => item.manifest),
    [
      "examples/broken/package.json",
      "examples/demo/package.json",
      "src/__fixtures__/package.json",
      "test/package.json",
      "tests/fixtures/app/Dockerfile",
      "tests/fixtures/app/package.json",
    ],
  );
  assert.match(verbose.skipped[1].reason, /^under examples\//);
});

test("keeps an example that a workflow publishes", async () => {
  const root = await repository({
    "examples/demo/package.json": npmPackage("demo"),
    ".github/workflows/release.yml": [
      "on: push",
      "jobs:",
      "  publish:",
      "    permissions:",
      "      id-token: write",
      "    steps:",
      "      - run: npm publish",
      "        working-directory: examples/demo",
      "",
    ].join("\n"),
  });
  const inspection = await inspectRepository(root);
  assert.deepEqual(
    inspection.packages.map((item) => [item.name, item.workflow]),
    [["demo", "release.yml"]],
  );
});

test("honors the ignore list in .package-registry-manager.json", async () => {
  const root = await repository({
    "package.json": npmPackage("tool"),
    "packages/legacy/package.json": npmPackage("legacy"),
    "packages/kept/package.json": npmPackage("kept"),
    "docs/site/package.json": npmPackage("site"),
    ".package-registry-manager.json": JSON.stringify({
      ignore: ["packages/legacy", "docs/**"],
    }),
  });
  const inspection = await inspectRepository(root, { includeSkipped: true });
  assert.deepEqual(
    inspection.packages.map((item) => item.name),
    ["tool", "kept"],
  );
  assert.deepEqual(inspection.skipped, [
    {
      manifest: "docs/site/package.json",
      reason: 'ignored by "docs/**" in .package-registry-manager.json',
    },
    {
      manifest: "packages/legacy/package.json",
      reason: 'ignored by "packages/legacy" in .package-registry-manager.json',
    },
  ]);
});

test("rejects a malformed ignore list", async () => {
  const root = await repository({
    ".package-registry-manager.json": '{"ignore": "tests"}',
  });
  await assert.rejects(
    inspectRepository(root),
    /"ignore" must be an array of strings/,
  );
});

test("matches globs against a path and its parents", () => {
  assert.equal(
    ignoredBy(["*/legacy"], "packages/legacy/package.json"),
    "*/legacy",
  );
  assert.equal(
    ignoredBy(["**/package.json"], "a/b/package.json"),
    "**/package.json",
  );
  assert.equal(
    ignoredBy(["**/legacy/**"], "legacy/x/Cargo.toml"),
    "**/legacy/**",
  );
  assert.equal(ignoredBy(["pack?ges"], "packages/a/package.json"), "pack?ges");
  assert.equal(ignoredBy(["packages"], "packages-extra/package.json"), null);
  assert.equal(ignoredBy(["*.json"], "a/package.json"), null);
  assert.equal(testDirectory("tests/package.json"), "tests");
  assert.equal(testDirectory("package.json"), null);
  assert.equal(testDirectory("src/testing/package.json"), null);
});

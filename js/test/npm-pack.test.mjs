import assert from "node:assert/strict";
import { performance } from "node:perf_hooks";
import { test } from "node:test";
import { registryEndpoint } from "../src/registry-state.mjs";

test("names the development directories a tarball would publish", async () => {
  const { developmentDirectories } = await import("../src/setup.mjs");
  const files = [
    "package.json",
    "js/src/index.js",
    "tests/cli.test.js",
    ".github/workflows/release.yml",
    "tests/unit/a.test.js",
    "test.js",
  ].map((path) => ({ path }));
  assert.deepEqual(developmentDirectories({ files }), [".github/", "tests/"]);
  assert.deepEqual(
    developmentDirectories({ files: [{ path: "lib/test/a.js" }] }),
    [],
  );
});

test("reads npm pack output before and since npm 12 (#22)", async () => {
  const { packedEntry } = await import("../src/setup.mjs");
  const packed = { filename: "a-1.0.0.tgz", version: "1.0.0" };
  assert.deepEqual(packedEntry(JSON.stringify([packed])), packed);
  assert.deepEqual(packedEntry(JSON.stringify({ a: packed })), packed);
  assert.throws(() => packedEntry("{}"), /npm pack printed no package/);
});

test("trims trailing slashes from endpoint overrides in linear time", () => {
  const variable = "PACKAGE_REGISTRY_MANAGER_NPM_REGISTRY";
  assert.equal(
    registryEndpoint("npm", { [variable]: "http://mirror.test/npm///" }),
    "http://mirror.test/npm",
  );
  const hostile = `${"/".repeat(100_000)}x`;
  const started = performance.now();
  assert.equal(registryEndpoint("npm", { [variable]: hostile }), hostile);
  assert.ok(performance.now() - started < 1000);
});

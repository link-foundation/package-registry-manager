import assert from "node:assert/strict";
import { test } from "node:test";
import { publishedNpmVersions, verifyNpmRelease } from "../src/npm-release.mjs";

test("post-release verification checks the actual version of both generated names independently", async () => {
  const calls = [];
  const session = {
    plan: {
      repository: { github_owner: "a", github_repository: "b" },
      package: {
        registry: "npm",
        name: "main",
        generated_artifact: { names: ["main", "alias"] },
      },
    },
    options: {
      probePackage: async (item) => {
        calls.push(`${item.name}@${item.version}`);
        return { manifest_version_exists: item.name === "main" };
      },
    },
    capture: async (step) => ({
      code: 0,
      stdout: step.command.args.includes("--log")
        ? "timestamp + main@2.0.0\ntimestamp + alias@2.0.1"
        : '{"jobs":[{"databaseId":3}]}',
    }),
  };
  assert.equal(
    publishedNpmVersions("+ @a/b@1.2.3\n", ["@a/b"]).get("@a/b"),
    "1.2.3",
  );
  await assert.rejects(verifyNpmRelease(session, 1), /alias@2.0.1/);
  assert.deepEqual(calls, ["main@2.0.0", "alias@2.0.1"]);
});

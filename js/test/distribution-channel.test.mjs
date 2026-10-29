import assert from "node:assert/strict";
import { test } from "node:test";
import { auditDistributionChannel } from "../src/distribution-channel.mjs";

test("Conan reads effective remote and credential health without exposing credentials or claiming publication", async () => {
  const calls = [];
  const session = {
    plan: {
      registry: "conan",
      package: { distribution: {} },
      repository: { github_owner: "a", github_repository: "b" },
    },
    options: {
      github: {
        variables: (scope) => ({
          effective: async (name) => {
            calls.push([scope, name]);
            return { verified: true, value: "https://conan.example/remote" };
          },
        }),
        health: {
          health: async (name) => ({
            name,
            status: "unknown",
            evidence: [{ reason: "logs-expired", match: "secret-value" }],
          }),
        },
      },
    },
  };
  const result = await auditDistributionChannel(session);
  assert.deepEqual(calls, [[{ repo: "a/b" }, "CONAN_REMOTE_URL"]]);
  assert.equal(result.remote.status, "configured");
  assert.equal(result.publication, "unverified");
  assert.equal(result.credentials.length, 2);
  assert.doesNotMatch(JSON.stringify(result), /secret-value/);
  session.options.github.variables = () => ({
    effective: async () => ({ verified: true, value: null }),
  });
  assert.equal(
    (await auditDistributionChannel(session)).remote.status,
    "missing",
  );
});

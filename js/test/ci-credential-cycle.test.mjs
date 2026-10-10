import assert from "node:assert/strict";
import { test } from "node:test";
import { cycleCredential, secretName } from "../src/ci-credential-cycle.mjs";

function adapter(status, tests = ["ok"], previous = { token_id: "old" }) {
  const calls = [];
  let index = 0;
  return {
    calls,
    health: async () => ({ status }),
    metadata: async () => previous,
    create: async () => {
      calls.push("create");
      return {
        value: "sensitive",
        id: `new-${++index}`,
        expires_at: "2099-01-01T00:00:00Z",
      };
    },
    ensure: async (credential) => {
      calls.push("ensure");
      assert.equal(credential.value, "sensitive");
      return { path: "repository", fallbackReason: "organization refused" };
    },
    test: async () => {
      calls.push("test");
      return { status: tests.shift() ?? "unknown" };
    },
    revoke: async (id) => calls.push(`revoke:${id}`),
    revoked: async () => true,
  };
}

test("healthy CI is a no-op despite expired metadata (#43)", async () => {
  const host = adapter("ok");
  host.metadata = () => {
    throw new Error("healthy secrets need no metadata request");
  };
  const result = await cycleCredential(host);
  assert.equal(result.status, "ok");
  assert.deepEqual(host.calls, []);
});

test("auth failure stores, tests, and only then revokes; fallback reaches summary (#43)", async () => {
  const host = adapter("auth-failing");
  const result = await cycleCredential(host);
  assert.deepEqual(host.calls, ["create", "ensure", "test", "revoke:old"]);
  assert.equal(result.path, "repository");
  assert.equal(result.fallbackReason, "organization refused");
});

test("no runs and absent secret creates, tests, and reissues once for auth failure (#43)", async () => {
  const host = adapter("unknown", ["auth-failing", "ok"], null);
  const result = await cycleCredential(host);
  assert.equal(result.status, "ok");
  assert.deepEqual(host.calls, [
    "create",
    "ensure",
    "test",
    "create",
    "ensure",
    "test",
    "revoke:new-1",
  ]);
});

test("existing unknown secret is tested before acquisition; unrelated failures do not rotate (#43)", async () => {
  const host = adapter("unknown");
  await cycleCredential(host);
  assert.deepEqual(host.calls, ["test"]);
  const failing = adapter("auth-failing", ["unknown"]);
  await assert.rejects(cycleCredential(failing), /verification.*unknown/);
  assert.deepEqual(failing.calls, ["create", "ensure", "test"]);
});

test("repeated auth failure stops after two candidates and preserves old credentials (#43)", async () => {
  const host = adapter("auth-failing", ["auth-failing", "auth-failing"]);
  await assert.rejects(cycleCredential(host), /auth-failing.*two/);
  assert.equal(host.calls.filter((item) => item === "create").length, 2);
  assert.ok(!host.calls.includes("revoke:old"));
});

test("custom secret templates resolve and reject invalid names (#43)", () => {
  assert.equal(
    secretName("{REGISTRY}_TOKEN_{REPO}", "docker-hub", "team/my-app"),
    "DOCKER_HUB_TOKEN_MY_APP",
  );
  assert.throws(
    () => secretName("{MISSING}", "npm", "team/app"),
    /secret name/,
  );
});

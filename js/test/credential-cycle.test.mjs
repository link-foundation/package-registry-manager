import assert from "node:assert/strict";
import { test } from "node:test";
import {
  credentialPolicy,
  needsRotation,
  rotateCredential,
} from "../src/credential-cycle.mjs";

test("registry policy prefers OIDC and built-in GitHub credentials (#38)", () => {
  for (const registry of [
    "npm",
    "pypi",
    "crates-io",
    "rubygems",
    "nuget",
    "jsr",
  ]) {
    assert.equal(credentialPolicy(registry).mode, "trusted");
  }
  assert.equal(credentialPolicy("ghcr").mode, "github-token");
  for (const registry of [
    "docker-hub",
    "maven-central",
    "vscode-marketplace",
    "open-vsx",
    "chrome-web-store",
  ]) {
    assert.equal(credentialPolicy(registry).mode, "token");
  }
});

test("missing, expiring, and rejected credentials rotate (#38)", () => {
  assert.equal(needsRotation({ present: false }), true);
  assert.equal(needsRotation({ present: true, valid: false }), true);
  assert.equal(
    needsRotation(
      { present: true, expires_at: "2026-10-08T00:00:00Z" },
      Date.parse("2026-10-07T00:00:00Z"),
    ),
    true,
  );
  assert.equal(
    needsRotation(
      { present: true, expires_at: "2027-01-01T00:00:00Z" },
      Date.parse("2026-10-07T00:00:00Z"),
    ),
    false,
  );
});

test("replacement is stored and verified before old token revocation (#38)", async () => {
  const calls = [];
  const token = "sensitive-value";
  await rotateCredential(
    { token_id: "old" },
    {
      create: async () => {
        calls.push("create");
        return { value: token, id: "new", expires_at: "2027-01-01T00:00:00Z" };
      },
      store: async (credential) => {
        assert.equal(credential.value, token);
        calls.push("store");
      },
      verify: async () => {
        calls.push("verify");
      },
      revoke: async (id) => {
        assert.equal(id, "old");
        calls.push("revoke");
      },
      revoked: async () => {
        calls.push("revoked");
        return true;
      },
    },
  );
  assert.deepEqual(calls, ["create", "store", "verify", "revoke", "revoked"]);
});

test("failed verification preserves old token and cleans up new credential (#38)", async () => {
  const revoked = [];
  await assert.rejects(
    rotateCredential(
      { token_id: "old" },
      {
        create: async () => ({
          id: "new",
          value: "sensitive",
          expires_at: "2027-01-01T00:00:00Z",
        }),
        store: async () => {},
        verify: async () => {
          throw new Error("verification failed");
        },
        revoke: async (id) => revoked.push(id),
        revoked: async () => true,
        restore: async () => {},
      },
    ),
    /verification failed/,
  );
  assert.deepEqual(revoked, ["new"]);
});

test("without rollback, failed validation retains both tokens (#38)", async () => {
  const revoked = [];
  const created = {
    id: "new",
    value: "sensitive",
    expires_at: "2027-01-01T00:00:00Z",
  };
  await assert.rejects(
    rotateCredential(
      { token_id: "old" },
      {
        create: async () => created,
        store: async () => {},
        verify: async () => {
          throw new Error("bad login");
        },
        revoke: async (id) => revoked.push(id),
      },
    ),
    /bad login/,
  );
  assert.deepEqual(revoked, []);
  assert.equal(created.value, "");
});

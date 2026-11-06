import assert from "node:assert/strict";
import { test } from "node:test";
import sodium from "libsodium-wrappers";
import { githubServices } from "../src/github.mjs";
import { readFile } from "node:fs/promises";
import { AUTH_FAILURE_PATTERNS } from "../src/publishing-policy.mjs";

test("both ports pin the same GitHub dependency and registry failure patterns (#43)", async () => {
  const policies = JSON.parse(
    await readFile(
      new URL("../../rust/src/publishing-policy.json", import.meta.url),
      "utf8",
    ),
  );
  assert.deepEqual(policies, AUTH_FAILURE_PATTERNS);
  const pkg = JSON.parse(
    await readFile(new URL("../package.json", import.meta.url), "utf8"),
  );
  const transport = await readFile(
    new URL("../../rust/src/github.rs", import.meta.url),
    "utf8",
  );
  assert.ok(
    transport.includes(pkg.dependencies["@link-foundation/gh-manager"]),
  );
});

test("installed gh-manager library accepts repository scopes and verifies org-refused fallback (#43)", async () => {
  await sodium.ready;
  const keys = sodium.crypto_box_keypair();
  const secrets = new Map();
  const calls = [];
  const rest = {
    hasToken: true,
    request: async () => ({}),
    send: async (url, { method = "GET", body } = {}) => {
      const path = url.split("?")[0];
      calls.push([method, path]);
      const answer = (status, body) => ({ ok: status < 400, status, body });
      if (path.startsWith("/orgs/team/actions/secrets")) {
        return answer(403, {});
      }
      if (path === "/repos/team/project") {
        return answer(200, { id: 1 });
      }
      if (path.endsWith("/public-key")) {
        return answer(200, {
          key_id: "test",
          key: sodium.to_base64(
            keys.publicKey,
            sodium.base64_variants.ORIGINAL,
          ),
        });
      }
      if (path.endsWith("/secrets")) {
        return answer(200, { secrets: [...secrets.values()] });
      }
      if (path.includes("/variables/")) {
        return answer(404, {});
      }
      const name = path.split("/").at(-1);
      if (method === "PUT") {
        const value = sodium.to_string(
          sodium.crypto_box_seal_open(
            sodium.from_base64(
              body.encrypted_value,
              sodium.base64_variants.ORIGINAL,
            ),
            keys.publicKey,
            keys.privateKey,
          ),
        );
        assert.equal(value, "private-value");
        secrets.set(name, { name, updated_at: new Date().toISOString() });
        return answer(204);
      }
      return secrets.has(name)
        ? answer(200, secrets.get(name))
        : answer(404, {});
    },
  };
  const github = githubServices({ rest });
  assert.equal(
    await github.secrets({ repo: "team/project" }).getMetadata("CUSTOM_TOKEN"),
    null,
  );
  const result = await github.secrets({ org: "team" }).ensure("CUSTOM_TOKEN", {
    repos: ["team/project"],
    acquire: async () => ({ value: "private-value" }),
    health: { status: "auth-failing" },
    rotateBeforeMs: 0,
  });
  assert.equal(result.path, "repository");
  assert.match(result.fallbackReason, /403/);
  assert.equal(result.results[0].verified, true);
  assert.ok(
    calls.some(
      ([method, path]) =>
        method === "PUT" &&
        path === "/repos/team/project/actions/secrets/CUSTOM_TOKEN",
    ),
  );
  assert.ok(!JSON.stringify(result).includes("private-value"));
});

test("installed gh-manager 1.5.0 encrypts a Central pair and exposes actual expiry to health", async () => {
  await sodium.ready;
  const keys = sodium.crypto_box_keypair();
  const secrets = new Map();
  const variables = new Map();
  const expiry = "2026-12-06T11:22:33.000Z";
  const rest = {
    hasToken: true,
    request: async (path) =>
      path.includes("/contents/")
        ? []
        : path === "/repos/team/library"
          ? { default_branch: "main" }
          : (variables.get(path.split("/").at(-1)) ??
            secrets.get(path.split("/").at(-1)) ??
            null),
    send: async (path, { method = "GET", body } = {}) => {
      const answer = (status, value) => ({
        status,
        ok: status < 400,
        body: value,
      });
      const name = path.split("/").at(-1);
      if (name === "public-key") {
        return answer(200, {
          key_id: "key",
          key: sodium.to_base64(
            keys.publicKey,
            sodium.base64_variants.ORIGINAL,
          ),
        });
      }
      if (method === "PUT" && path.includes("/secrets/")) {
        const plaintext = sodium.to_string(
          sodium.crypto_box_seal_open(
            sodium.from_base64(
              body.encrypted_value,
              sodium.base64_variants.ORIGINAL,
            ),
            keys.publicKey,
            keys.privateKey,
          ),
        );
        assert.equal(
          plaintext,
          name === "CENTRAL_USERNAME" ? "user-id" : "private-token",
        );
        secrets.set(name, { name, updated_at: "2026-11-06T00:00:00Z" });
        return answer(204);
      }
      if (
        (method === "POST" || method === "PATCH") &&
        path.includes("/variables")
      ) {
        variables.set(body.name ?? name, {
          name: body.name ?? name,
          value: body.value,
        });
        return answer(204);
      }
      if (name === "variables") {
        return answer(200, { variables: [...variables.values()] });
      }
      if (name === "secrets") {
        return answer(200, { secrets: [...secrets.values()] });
      }
      const found = path.includes("/variables/")
        ? variables.get(name)
        : secrets.get(name);
      return answer(found ? 200 : 404, found ?? {});
    },
  };
  const github = githubServices({ rest });
  for (const [name, value] of [
    ["CENTRAL_USERNAME", "user-id"],
    ["CENTRAL_TOKEN", "private-token"],
  ]) {
    const result = await github
      .secrets({ repo: "team/library" })
      .set(name, value, { expiresAt: expiry });
    assert.equal(result.verified, true);
    assert.equal(variables.get(`${name}_EXPIRES_AT`).value, expiry);
    const state = await github.health.health(name, {
      scope: { repo: "team/library" },
    });
    assert.equal(state.expiresAt, expiry);
    assert.ok(!JSON.stringify(result).includes(value));
  }
});

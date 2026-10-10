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

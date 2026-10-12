import assert from "node:assert/strict";
import { test } from "node:test";
import { runInNewContext } from "node:vm";
import { fileURLToPath } from "node:url";
import { rotateCratesToken } from "../src/crates-token-rotation.mjs";

function rotationFixture({
  visibility = "selected",
  status = "ok",
  fallback = false,
  ambiguous = false,
} = {}) {
  const calls = [];
  const previous = {
    id: 10,
    name: "CARGO_TOKEN",
    created_at: "2026-10-01T00:00:00Z",
    crate_scopes: ["start-command"],
    endpoint_scopes: ["publish-update"],
  };
  const tokens = [previous, ...(ambiguous ? [{ ...previous, id: 12 }] : [])];
  const page = {
    goto: async () => {},
    evaluate: async (script) =>
      runInNewContext(script, {
        fetch: async (url, init) => {
          let body;
          if (url === "/api/v1/me") {
            body = { user: { login: "maintainer", email_verified: true } };
          } else if (init.method === "GET") {
            body = { api_tokens: tokens };
          } else if (init.method === "PUT") {
            calls.push("create");
            const request = JSON.parse(init.body).api_token;
            assert.deepEqual(request.crate_scopes, previous.crate_scopes);
            assert.deepEqual(request.endpoint_scopes, previous.endpoint_scopes);
            assert.ok(Date.parse(request.expired_at) > Date.now());
            const token = {
              ...request,
              id: 11,
              token: "sensitive-replacement",
            };
            tokens.push(token);
            body = { api_token: token };
          } else {
            calls.push("revoke");
            assert.ok(
              calls.indexOf("test:acme/peer") < calls.indexOf("revoke"),
            );
            tokens.splice(
              tokens.findIndex(
                (token) => String(token.id) === url.split("/").at(-1),
              ),
              1,
            );
            body = {};
          }
          return { status: 200, json: async () => body };
        },
      }),
  };
  const metadata = {
    updated_at: previous.created_at,
    visibility,
    selected_repositories: [{ full_name: "acme/peer" }],
  };
  const github = {
    secrets: (scope) => ({
      getMetadata: async () =>
        scope.org
          ? metadata
          : scope.repo === "acme/start"
            ? { updated_at: previous.created_at }
            : null,
      ensure: async (secret, settings) => {
        assert.equal(secret, "CARGO_TOKEN");
        assert.deepEqual(settings.repos, ["acme/start", "acme/peer"]);
        assert.equal(settings.visibility, "selected");
        const credential = await settings.acquire();
        assert.equal(credential.value, "sensitive-replacement");
        assert.ok(credential.expiresAt);
        calls.push("store");
        return {
          valueChanged: true,
          path: fallback ? "repository" : "organization",
        };
      },
      set: async (secret, value, settings) => {
        assert.equal(secret, "CARGO_TOKEN");
        assert.equal(value, "sensitive-replacement");
        assert.ok(settings.expiresAt);
        if (scope.org) {
          assert.equal(settings.visibility, visibility);
        }
        calls.push(scope.org ? "store" : "override");
        return { valueChanged: true };
      },
    }),
    health: {
      test: async (_secret, settings) => {
        calls.push(`test:${settings.scope.repo}`);
        return { status };
      },
    },
  };
  const session = {
    plan: {
      registry: "crates-io",
      package: { name: "start-command" },
      repository: { github_owner: "acme", github_repository: "start" },
    },
    options: {
      repository: fileURLToPath(
        new URL("../../tests/fixtures/token-rotation", import.meta.url),
      ),
      rotateToken: "CARGO_TOKEN",
      yes: true,
      github,
      accountScan: true,
      rotatedSecrets: new Map(),
      secretRepositories: {
        "crates-io:CARGO_TOKEN": ["acme/start", "acme/peer"],
      },
      log: () => {},
    },
    conditions: new Set(),
    automatedPage: async () => page,
  };
  return { session, calls, tokens };
}

test("rotation stores expiry, replaces overrides, verifies every consumer, then revokes once", async () => {
  const { session, calls, tokens } = rotationFixture();
  await rotateCratesToken(session);
  assert.deepEqual(calls, [
    "create",
    "store",
    "override",
    "test:acme/start",
    "test:acme/peer",
    "revoke",
  ]);
  assert.deepEqual(
    tokens.map((token) => token.id),
    [11],
  );
  assert.equal(session.outcome.secret_scope, "organization");
  await rotateCratesToken(session);
  assert.equal(calls.filter((call) => call === "create").length, 1);
});

test("uncertain or failing CI retains old and replacement tokens and blocks repeated rotation", async () => {
  for (const status of ["unknown", "auth-failing"]) {
    const { session, calls, tokens } = rotationFixture({ status });
    await assert.rejects(rotateCratesToken(session), /retained/);
    assert.deepEqual(
      tokens.map((token) => token.id),
      [10, 11],
    );
    assert.ok(!calls.includes("revoke"));
    await assert.rejects(rotateCratesToken(session), /retained/);
    assert.equal(calls.filter((call) => call === "create").length, 1);
  }
});

test("organization broad visibility is preserved and repository fallback verifies consumers", async () => {
  for (const options of [
    { visibility: "all" },
    { visibility: "private" },
    { fallback: true },
  ]) {
    const { session, calls } = rotationFixture(options);
    await rotateCratesToken(session);
    assert.ok(calls.includes("revoke"));
    assert.equal(
      session.outcome.secret_scope,
      options.fallback ? "repository" : "organization",
    );
  }
});

test("ambiguous token identity and incomplete scans stop before creating credentials", async () => {
  const { session, calls } = rotationFixture({ ambiguous: true });
  await assert.rejects(rotateCratesToken(session), /uniquely/);
  assert.deepEqual(calls, []);
  const incomplete = rotationFixture();
  incomplete.session.options.scanErrors = true;
  await assert.rejects(
    rotateCratesToken(incomplete.session),
    /complete account scan/,
  );
  assert.deepEqual(incomplete.calls, []);
});

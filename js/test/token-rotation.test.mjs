import assert from "node:assert/strict";
import path from "node:path";
import { readFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import { inspectRepository } from "../src/discovery.mjs";
import { buildPlans } from "../src/plan.mjs";
import { scanRepositories } from "../src/organization-scan.mjs";
import { cycleCredential } from "../src/ci-credential-cycle.mjs";
import { accountCommand } from "../src/organization-setup.mjs";
import {
  selectCratesToken,
  replacementTokenRequest,
  tokenExpiryFindings,
} from "../src/crates-tokens.mjs";

const root = fileURLToPath(
  new URL("../../tests/fixtures/token-rotation", import.meta.url),
);

test("CLI rejects invalid rotation scope and offline private inventory", () => {
  const cli = fileURLToPath(new URL("../src/cli.mjs", import.meta.url));
  for (const args of [
    ["setup", "--registry", "npm", "--rotate-token", "CARGO_TOKEN"],
    ["setup", "--registry", "crates-io", "--rotate-token", "{ORG}_TOKEN"],
    ["inspect", "--org", "acme", "--offline", "--token-expiry-days", "7"],
  ]) {
    const output = spawnSync(process.execPath, [cli, ...args], {
      encoding: "utf8",
    });
    assert.notEqual(output.status, 0);
    assert.match(output.stderr, /--rotate-token|--token-expiry-days/);
  }
});

test("expiry warning window is inclusive, reports expired tokens, and ignores absent expiry", () => {
  const now = Date.parse("2026-10-01T00:00:00Z");
  const tokens = [
    { id: 1, expired_at: "2026-10-08T00:00:00Z" },
    { id: 2, expired_at: "2026-09-30T00:00:00Z" },
    { id: 3, expired_at: null },
  ];
  assert.deepEqual(
    tokenExpiryFindings(tokens, "acme", 6, now).map(
      (finding) => finding.evidence.token_id,
    ),
    [2],
  );
  assert.deepEqual(
    tokenExpiryFindings(tokens, "acme", 7, now).map((finding) => finding.type),
    ["registry-token-expiring", "registry-token-expired"],
  );
  assert.throws(() => tokenExpiryFindings(tokens, "acme", -1, now), /integer/);
});

test("scripted cargo variable and multiline args select the reusable workflow caller (#56)", async () => {
  const inspection = await inspectRepository(root);
  assert.equal(inspection.packages[0].workflow, "js.yml");
  assert.deepEqual(inspection.packages[0].workflow_jobs, ["rust-stage"]);
  assert.ok(inspection.packages[0].token_secrets.includes("CARGO_TOKEN"));
});

test("account scan warns about crates.io expired_at before CI fails (#56)", async () => {
  const scan = await scanRepositories({
    org: "acme",
    offline: true,
    tokenExpiryDays: 7,
    cratesTokens: [
      {
        id: 10,
        name: "CARGO_TOKEN",
        expired_at: new Date(Date.now() + 86400000).toISOString(),
      },
    ],
    github: { repos: { list: async () => [], files: async () => [] } },
  });
  assert.equal(scan.findings[0]?.type, "registry-token-expiring");
  assert.match(JSON.stringify(scan.findings), /CARGO_TOKEN/);
});

test("token matching rejects ambiguity and replacement preserves null or scoped restrictions (#56)", () => {
  const token = {
    id: 10,
    name: "CARGO_TOKEN",
    created_at: "2026-10-01T00:00:00Z",
    crate_scopes: ["start-command"],
    endpoint_scopes: ["publish-update"],
  };
  const metadata = { updated_at: "2026-10-01T00:00:02Z" };
  assert.equal(selectCratesToken([token], "CARGO_TOKEN", metadata).id, 10);
  assert.throws(
    () =>
      selectCratesToken([token, { ...token, id: 11 }], "CARGO_TOKEN", metadata),
    /uniquely/,
  );
  assert.equal(selectCratesToken([token], "CARGO_TOKEN", {}, 10).id, 10);
  assert.deepEqual(replacementTokenRequest(token).api_token.crate_scopes, [
    "start-command",
  ]);
  assert.equal(
    replacementTokenRequest({ ...token, crate_scopes: null }).api_token
      .crate_scopes,
    null,
  );
  assert.throws(
    () => replacementTokenRequest({ ...token, endpoint_scopes: undefined }),
    /broaden/,
  );
});

test("explicit rotation builds a token plan while ordinary crates setup stays trusted (#56)", async () => {
  const inspection = await inspectRepository(root);
  inspection.packages[0].workflow = "js.yml";
  const [plan] = buildPlans(inspection, ["crates-io"], {
    rotateToken: "CARGO_TOKEN",
  });
  assert.equal(plan.steps[0].id, "rotate-crates-token");
  assert.equal(plan.credential_policy.mode, "token");
  assert.equal(buildPlans(inspection)[0].credential_policy.mode, "trusted");
});

test("explicit rotation replaces a healthy credential and revokes only after CI (#56)", async () => {
  const calls = [];
  const result = await cycleCredential(
    {
      health: async () => ({ status: "ok" }),
      metadata: async () => ({ token_id: "old" }),
      create: async () => ({ id: "new", value: "private" }),
      ensure: async () => {
        calls.push("store");
        return {};
      },
      test: async () => {
        calls.push("test");
        return { status: "ok" };
      },
      revoke: async (id) => calls.push(`revoke:${id}`),
      revoked: async () => true,
    },
    { force: true },
  );
  assert.equal(result.changed, true);
  assert.deepEqual(calls, ["store", "test", "revoke:old"]);
});

test("account snapshots fetch publishing scripts (#56)", async () => {
  let scriptFetch = false;
  const scan = await scanRepositories({
    org: "acme",
    offline: true,
    github: {
      repos: {
        list: async () => [{ full_name: "acme/start", default_branch: "main" }],
        files: async (_slug, options) => {
          scriptFetch ||= options.match.includes(
            "**/scripts/publish-to-crates.mjs",
          );
          return Promise.all(
            (scriptFetch
              ? ["scripts/publish-to-crates.mjs"]
              : [
                  "Cargo.toml",
                  ".github/workflows/js.yml",
                  ".github/workflows/rust.yml",
                ]
            ).map(async (name) => ({
              path: name,
              content: await readFile(path.join(root, name), "utf8"),
            })),
          );
        },
      },
    },
  });
  assert.equal(scan.repositories[0].inspection.packages[0].workflow, "js.yml");
  assert.equal(scriptFetch, true);
});

test("account rotation verifies custom-secret readers even without package manifests", async () => {
  let executed = false;
  await accountCommand(
    "setup",
    {
      org: "acme",
      offline: true,
      rotateToken: "PRIVATE_CARGO",
      registries: ["crates-io"],
      probeEnvironment: async () => ({}),
      github: {
        repos: {
          list: async () => [
            { full_name: "acme/start", default_branch: "main" },
            { full_name: "acme/consumer", default_branch: "main" },
          ],
          files: async (slug) =>
            slug === "acme/consumer"
              ? [
                  {
                    path: ".github/workflows/check.yml",
                    content:
                      "on: push\njobs:\n  consumer:\n    steps:\n      - run: cargo search example\n        env:\n          CARGO_TOKEN: ${{ secrets.PRIVATE_CARGO }}\n",
                  },
                ]
              : Promise.all(
                  [
                    "Cargo.toml",
                    ".github/workflows/js.yml",
                    ".github/workflows/rust.yml",
                    "scripts/publish-to-crates.mjs",
                  ].map(async (name) => ({
                    path: name,
                    content: (
                      await readFile(path.join(root, name), "utf8")
                    ).replaceAll("CARGO_TOKEN", "PRIVATE_CARGO"),
                  })),
                ),
        },
      },
      executePlans: async (plans, options) => {
        executed = true;
        assert.equal(plans.length, 1);
        assert.equal(plans[0].steps[0].id, "rotate-crates-token");
        assert.deepEqual(options.secretRepositories, {
          "crates-io:PRIVATE_CARGO": ["acme/start", "acme/consumer"],
        });
        return [];
      },
    },
    () => {},
  );
  assert.equal(executed, true);
});

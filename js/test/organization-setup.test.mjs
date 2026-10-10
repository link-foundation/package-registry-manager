import assert from "node:assert/strict";
import { test } from "node:test";
import { stat } from "node:fs/promises";
import { accountCommand } from "../src/organization-setup.mjs";

test("account setup selects findings, groups npm, and executes once with per-repository snapshots (#43)", async () => {
  const prepared = [];
  let roots;
  let calls = 0;
  await accountCommand(
    "setup",
    {
      org: "team",
      execute: true,
      github: {
        repos: {
          list: async () =>
            ["rust", "npm", "healthy"].map((name) => ({
              full_name: `team/${name}`,
              default_branch: "main",
            })),
          files: async (slug) =>
            slug.endsWith("rust")
              ? [
                  {
                    path: "Cargo.toml",
                    content: '[package]\nname="rust"\nversion="1.0.0"',
                  },
                ]
              : [
                  {
                    path: "package.json",
                    content: JSON.stringify({
                      name: slug.split("/")[1],
                      version: "1.0.0",
                    }),
                  },
                ],
        },
        runs: { failures: async () => [] },
      },
      probe: async (inspection) => {
        inspection.packages[0].exists_on_registry =
          inspection.repository.github_repository === "healthy";
        inspection.packages[0].trusted_publishing =
          inspection.packages[0].exists_on_registry;
        return inspection;
      },
      probeEnvironment: async () => ({}),
      prepareRepository: async (item) => prepared.push(item.repository),
      executePlans: async (plans, options) => {
        calls += 1;
        assert.deepEqual(
          plans.map((plan) => plan.registry),
          ["npm", "crates-io"],
        );
        assert.equal(options.accountScan, true);
        roots = plans.map((plan) => plan.repository.root);
        assert.equal(new Set(roots).size, 2);
        for (const root of roots) {
          assert.ok((await stat(root)).isDirectory());
        }
        return [];
      },
    },
    (plans) => {
      assert.ok(
        plans.every((plan) =>
          plan.repository.root.startsWith("https://github.com/team/"),
        ),
      );
    },
  );
  assert.equal(calls, 1);
  assert.deepEqual(prepared, ["team/rust", "team/npm"]);
  for (const root of roots) {
    await assert.rejects(stat(root), { code: "ENOENT" });
  }
});

function containerAccount(overrides = {}) {
  return {
    org: "team",
    execute: true,
    github: {
      repos: {
        list: async () =>
          ["one", "two"].map((name) => ({
            full_name: `team/${name}`,
            default_branch: "main",
          })),
        files: async () => [
          { path: "Dockerfile", content: "FROM scratch" },
          {
            path: ".github/workflows/release.yml",
            content:
              "jobs:\n  publish:\n    steps:\n      - uses: docker/build-push-action@v6\n        with:\n          push: true\n          tags: docker.io/team/image:latest",
          },
          {
            path: ".package-registry-manager.json",
            content: JSON.stringify({
              tokens: { "docker-hub": { secret: "{REGISTRY}_CI" } },
            }),
          },
        ],
      },
      runs: { failures: async () => [] },
    },
    probe: async (inspection) => {
      inspection.packages[0].exists_on_registry = false;
      return inspection;
    },
    probeEnvironment: async () => ({
      github: { state: "unknown", scopes: [] },
    }),
    prepareRepository: async () => {},
    ...overrides,
  };
}

test("account credential groups honor configured secret templates (#43)", async () => {
  await accountCommand(
    "setup",
    containerAccount({
      executePlans: async (plans, options) => {
        assert.equal(plans.length, 2);
        assert.deepEqual(options.secretRepositories, {
          "docker-hub:DOCKER_HUB_CI": ["team/one", "team/two"],
        });
        return [];
      },
    }),
    () => {},
  );
});

test("a repository preparation failure does not prevent other account findings (#43)", async () => {
  let executed = false;
  await assert.rejects(
    accountCommand(
      "setup",
      containerAccount({
        prepareRepository: async (item) => {
          if (item.repository === "team/one") {
            throw new Error("cannot prepare team/one");
          }
        },
        executePlans: async (plans) => {
          executed = true;
          assert.deepEqual(
            plans.map((plan) => plan.repository.github_repository),
            ["two"],
          );
          return [];
        },
      }),
      () => {},
    ),
    /some repositories/,
  );
  assert.equal(executed, true);
});

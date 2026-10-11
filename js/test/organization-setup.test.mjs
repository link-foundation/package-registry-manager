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
        runs: { list: async () => [] },
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
        assert.equal(options.browser, "automated");
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
      runs: { list: async () => [] },
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

test("setup plans and executes remaining repositories after scan failures (#48)", async () => {
  const options = containerAccount();
  const files = options.github.repos.files;
  options.github.repos.files = async (slug) => {
    if (slug === "team/one") {
      throw new Error("files forbidden");
    }
    return files(slug);
  };
  let executed = false;
  options.executePlans = async (plans) => {
    executed = true;
    assert.equal(plans.length, 1);
    assert.equal(plans[0].repository.github_repository, "two");
    return [];
  };
  await assert.rejects(
    accountCommand("setup", options, () => {}),
    /some repositories/,
  );
  assert.equal(executed, true);
});

async function captureOutput(command, options) {
  const write = process.stdout.write;
  const exitCode = process.exitCode;
  let output = "";
  process.stdout.write = (chunk, ...args) => {
    if (typeof chunk !== "string") {
      return write.call(process.stdout, chunk, ...args);
    }
    output += chunk;
    return true;
  };
  try {
    await accountCommand(command, options, () => {});
    assert.equal(process.exitCode, 1);
    return output;
  } finally {
    process.stdout.write = write;
    process.exitCode = exitCode;
  }
}

async function captureJsonReport(command, options) {
  return JSON.parse(
    await captureOutput(command, { ...options, format: "json" }),
  );
}

test("text inspection keeps successful rows and lists failures in its final summary (#48)", async () => {
  const options = containerAccount({ format: "text" });
  const files = options.github.repos.files;
  options.github.repos.files = async (slug) => {
    if (slug === "team/one") {
      throw new Error("files forbidden");
    }
    return files(slug);
  };
  const output = await captureOutput("inspect", options);
  assert.match(output, /team\/two\tdocker-hub\tteam\/image\tunpublished/);
  assert.ok(
    output.endsWith(
      "Scanned 2 repositories; 1 with errors.\nteam/one: files forbidden\n",
    ),
  );
});

test("JSON inspection retains scan errors and sets failure only after every repository (#48)", async () => {
  const options = containerAccount();
  const files = options.github.repos.files;
  options.github.repos.files = async (slug) => {
    if (slug === "team/one") {
      throw new Error("files forbidden");
    }
    assert.notEqual(process.exitCode, 1);
    return files(slug);
  };
  const scan = await captureJsonReport("inspect", options);
  assert.equal(scan.repositories.length, 2);
  assert.match(scan.repositories[0].error, /files forbidden/);
  assert.equal(scan.findings[0].type, "unpublished");
  assert.equal(
    scan.repositories[1].inspection.repository.root,
    "https://github.com/team/two",
  );
});

test("JSON planning preserves scan and workflow errors while planning other repositories (#48)", async () => {
  const options = containerAccount({
    planOptions: { workflow: "publish.yml" },
  });
  const files = options.github.repos.files;
  options.github.repos.files = async (slug) => {
    const snapshot = await files(slug);
    if (slug === "team/one") {
      snapshot.push({ path: "bad/package.json", content: "{" });
    } else {
      snapshot.push({
        path: ".github/workflows/publish.yml",
        content: snapshot[1].content,
      });
    }
    return snapshot;
  };
  const report = await captureJsonReport("plan", options);
  assert.deepEqual(
    report.repositories[0].errors.map((error) => error.stage),
    ["manifest", "plan"],
  );
  assert.match(report.repositories[0].error, /bad\/package.json/);
  assert.match(report.repositories[0].error, /has no .*publish.yml/);
  assert.equal(report.plans.length, 1);
  assert.equal(report.plans[0].repository.github_repository, "two");
});

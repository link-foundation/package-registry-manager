import assert from "node:assert/strict";
import { cp, mkdtemp, readFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { inspectRepository } from "../src/discovery.mjs";
import { buildPlans } from "../src/plan.mjs";
import {
  eligibleReleaseRuns,
  failedPublishingJob,
  PUBLISH_REJECTED,
  selectReleaseRun,
} from "../src/release-runs.mjs";
import { installFakeTools, runMockSetup } from "./helpers/fake-tools.mjs";

for (const repair of [false, true]) {
  test(
    `release selection skips newer PR and lint runs (${repair ? "repair" : "attach"})`,
    {
      skip: process.platform === "win32" && "fake tools are POSIX scripts",
      timeout: 15000,
    },
    async () => {
      const temporary = await mkdtemp(
        path.join(os.tmpdir(), "prm-release-runs-"),
      );
      const keys = [
        "FAKE_STATE",
        "FAKE_RELEASE_RUN",
        "FAKE_TRANSFER",
        "FAKE_TRANSFER_METADATA",
      ];
      const previous = keys.map((key) => process.env[key]);
      try {
        const repository = path.join(temporary, "repository");
        await cp(
          new URL("../../tests/fixtures/pipeline-template", import.meta.url),
          repository,
          { recursive: true },
        );
        process.env.FAKE_STATE = temporary;
        process.env.FAKE_RELEASE_RUN = "mixed";
        await cp(
          new URL(
            "../../tests/fixtures/repository-transfer/release-runs.json",
            import.meta.url,
          ),
          path.join(temporary, "release-runs.json"),
        );
        if (repair) {
          process.env.FAKE_TRANSFER = "1";
          process.env.FAKE_TRANSFER_METADATA = "correct";
        }
        await installFakeTools(temporary);
        const inspection = await inspectRepository(repository);
        const pkg = inspection.packages.find(
          (item) => item.name === "pipeline-app",
        );
        Object.assign(pkg, {
          exists_on_registry: true,
          trusted_publishing: repair,
          ...(repair
            ? {
                repository_mismatches: [
                  {
                    source: "trusted publisher",
                    repository: "old/pipeline-app",
                  },
                ],
              }
            : {}),
        });
        const plan = buildPlans(inspection, ["npm"], {
          verifyRelease: true,
        }).find((item) => item.package.name === pkg.name);
        let lookups = 0;
        const { lines, log } = await runMockSetup(
          plan,
          () =>
            ++lookups === 1
              ? { version: "0.1.0" }
              : {
                  version: "0.1.1",
                  _npmUser: { trustedPublisher: {} },
                  dist: { attestations: {} },
                },
          repository,
        );
        const commands = log.map((entry) => entry.argv);
        const retries = commands.filter(
          (args) =>
            args[0] === "gh" && args[1] === "run" && args[2] === "rerun",
        );
        assert.deepEqual(
          retries.map((args) => args[3]),
          ["42"],
        );
        assert.ok(
          commands.some(
            (args) =>
              args[1] === "run" && args[2] === "watch" && args[3] === "42",
          ),
        );
        assert.ok(
          !commands.some((args) => args[1] === "workflow" && args[2] === "run"),
        );
        assert.ok(
          lines.some((line) =>
            line.includes(
              "https://github.com/acme/pipeline-app/actions/runs/42",
            ),
          ),
        );
        assert.ok(lines.some((line) => line.includes("OIDC")));
        assert.ok(
          !lines.some((line) =>
            line.includes("trusted publishing alone will not fix it"),
          ),
        );
        const queries = commands.filter(
          (args) => args[1] === "run" && args[2] === "list",
        );
        assert.equal(
          queries.length,
          2,
          "inspection and repair share one selection",
        );
        for (const args of queries) {
          assert.equal(args[args.indexOf("--branch") + 1], "main");
          assert.ok(
            ["push", "workflow_dispatch"].includes(
              args[args.indexOf("--event") + 1],
            ),
          );
        }
        const logs = commands.filter((args) => args.includes("--log-failed"));
        assert.deepEqual(
          logs.map((args) => args[3]),
          ["42"],
        );
      } finally {
        keys.forEach((key, index) => {
          if (previous[index] === undefined) {
            delete process.env[key];
          } else {
            process.env[key] = previous[index];
          }
        });
        await rm(temporary, { recursive: true, force: true });
      }
    },
  );
}

test("release history filters branches/events, deduplicates and sorts", async () => {
  const runs = JSON.parse(
    await readFile(
      new URL(
        "../../tests/fixtures/repository-transfer/release-runs.json",
        import.meta.url,
      ),
      "utf8",
    ),
  );
  runs.push({ databaseId: 100, headBranch: "v1.0.0", event: "push" });
  runs.push({ databaseId: 101, headBranch: "main", event: "schedule" });
  assert.deepEqual(
    eligibleReleaseRuns([...runs, runs[3]].reverse(), "main").map(
      (run) => run.databaseId,
    ),
    [98, 42, 41],
  );
  assert.deepEqual(eligibleReleaseRuns(runs, "trunk"), []);
});

test("publish/preflight selection requires a failed job or publishing step", () => {
  for (const name of [
    "release-preflight",
    "Publish npm",
    "publish-crates",
    "PyPI publishing",
  ]) {
    assert.equal(failedPublishingJob([{ name, conclusion: "failure" }]), true);
    assert.equal(
      failedPublishingJob([
        { name, conclusion: "success" },
        { name: "lint", conclusion: "failure" },
      ]),
      false,
    );
  }
  assert.equal(
    failedPublishingJob([
      {
        name: "release",
        conclusion: "failure",
        steps: [{ name: "Run cargo publish", conclusion: "failure" }],
      },
    ]),
    true,
  );
  assert.equal(
    failedPublishingJob([
      {
        name: "release",
        conclusion: "failure",
        steps: [
          { name: "Publish", conclusion: "success" },
          { name: "Lint", conclusion: "failure" },
        ],
      },
    ]),
    false,
  );
});

test("classifies registry OIDC preflight rejections", () => {
  for (const log of [
    "npm error code E404",
    "npm error 404 Not Found",
    "invalid-publisher",
    "npm refused the OIDC package exchange (404) -- the trusted publisher on npmjs.com does not match",
    "crates.io refused the OIDC token exchange (403)",
    "PyPI refused the OIDC exchange (401)",
    "invalid_publisher",
    "unregistered-publisher",
  ]) {
    assert.equal(PUBLISH_REJECTED.test(log), true, log);
  }
  for (const log of [
    "lint failed",
    "missing changeset",
    "npm refused the OIDC package exchange (500)",
  ]) {
    assert.equal(PUBLISH_REJECTED.test(log), false, log);
  }
});

test("selects newest failed publishing job across events and caches the run", async () => {
  const queries = [];
  const session = {
    plan: {
      repository: { github_owner: "acme", github_repository: "app" },
      trusted_publisher: { workflow: "release.yml" },
    },
    capture: async ({ command: { args } }) => {
      queries.push(args);
      const result =
        args[0] === "api"
          ? "trunk"
          : JSON.stringify(
              args[1] === "list"
                ? [
                    {
                      databaseId: 99,
                      headBranch: "trunk",
                      event: "pull_request",
                      conclusion: "failure",
                      createdAt: "2026-10-10T12:00:00Z",
                    },
                    {
                      databaseId: args.includes("push") ? 42 : 43,
                      headBranch: "trunk",
                      event: args.includes("push")
                        ? "push"
                        : "workflow_dispatch",
                      conclusion: "failure",
                      createdAt: args.includes("push")
                        ? "2026-10-10T10:00:00Z"
                        : "2026-10-10T09:00:00Z",
                    },
                  ]
                : { jobs: [{ name: "publish-npm", conclusion: "failure" }] },
            );
      return { code: 0, stdout: result };
    },
  };
  const selection = await selectReleaseRun(session);
  assert.equal(selection.run.databaseId, 42);
  assert.equal(selection.publishFailure, true);
  assert.equal(await selectReleaseRun(session), selection);
  assert.equal(queries.length, 4);
  assert.ok(
    queries
      .filter((args) => args[1] === "list")
      .every((args) => args[args.indexOf("--branch") + 1] === "trunk"),
  );
});

test("no eligible publishing failure prevents retry and failed lookup stops selection", async () => {
  const plan = {
    repository: { github_owner: "acme", github_repository: "app" },
    trusted_publisher: { workflow: "release.yml" },
  };
  for (const conclusion of ["success", "failure"]) {
    const session = {
      plan,
      capture: async ({ command: { args } }) => ({
        code: 0,
        stdout:
          args[0] === "api"
            ? "main"
            : JSON.stringify(
                args[1] === "list"
                  ? [
                      {
                        databaseId: 1,
                        headBranch: "main",
                        event: "push",
                        conclusion,
                      },
                    ]
                  : { jobs: [{ name: "lint", conclusion: "failure" }] },
              ),
      }),
    };
    assert.equal((await selectReleaseRun(session)).publishFailure, false);
  }
  await assert.rejects(
    selectReleaseRun({ plan, capture: async () => ({ code: 1, stdout: "" }) }),
    /default branch/,
  );
});

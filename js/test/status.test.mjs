import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

import {
  addExpectedVersions,
  collectReleaseStatus,
  eligibleStatusRuns,
  packageStatuses,
  releaseVersionFromBranch,
  renderStatusText,
  selectStatusWorkflow,
  summarizeReports,
  summarizeStatus,
  waitForReleaseStatus,
} from "../src/status.mjs";

const execute = promisify(execFile);
const here = path.dirname(fileURLToPath(import.meta.url));
const cli = path.resolve(here, "../src/cli.mjs");
const fixture = path.resolve(here, "../../tests/fixtures/polyglot");

function captureFor(runs, details) {
  const calls = [];
  return {
    calls,
    capture: async ({ command: { args } }) => {
      calls.push(args);
      if (args[0] === "api") {
        return { code: 0, stdout: "main\n" };
      }
      if (args[1] === "list") {
        return { code: 0, stdout: JSON.stringify(runs) };
      }
      return {
        code: 0,
        stdout: JSON.stringify(details(args[2])),
      };
    },
  };
}

test("release tag branches name the version under test", () => {
  assert.equal(releaseVersionFromBranch("v1.25.1"), "1.25.1");
  assert.equal(releaseVersionFromBranch("1.25.1"), "1.25.1");
  assert.equal(releaseVersionFromBranch("main"), null);
  assert.equal(releaseVersionFromBranch("dependabot/npm_and_yarn/v2"), null);
});

test("release history keeps default-branch and tag runs but drops PRs", () => {
  const runs = [
    { databaseId: 1, headBranch: "main", event: "push", createdAt: "1" },
    { databaseId: 2, headBranch: "v1.2.3", event: "push", createdAt: "3" },
    {
      databaseId: 3,
      headBranch: "feature",
      event: "workflow_dispatch",
      createdAt: "4",
    },
    {
      databaseId: 4,
      headBranch: "v1.2.3",
      event: "workflow_dispatch",
      createdAt: "2",
    },
    {
      databaseId: 5,
      headBranch: "main",
      event: "pull_request",
      createdAt: "5",
    },
  ];
  assert.deepEqual(
    eligibleStatusRuns(runs, "main").map((run) => run.databaseId),
    [2, 4, 1],
  );
});

test("status chooses a workflow from the repository or its packages", () => {
  assert.equal(
    selectStatusWorkflow({
      repository: { release_workflow: "release.yml" },
      packages: [{ publishable: true, workflow: "other.yml" }],
    }),
    "release.yml",
  );
  assert.equal(
    selectStatusWorkflow({
      repository: { release_workflow: null },
      packages: [
        { publishable: true, workflow: "release.yml" },
        { publishable: true, workflow: "release.yml" },
      ],
    }),
    "release.yml",
  );
  assert.equal(
    selectStatusWorkflow({
      repository: { release_workflow: null },
      packages: [
        { publishable: true, workflow: "a.yml" },
        { publishable: true, workflow: "b.yml" },
      ],
    }),
    null,
  );
});

test("account summaries aggregate repository states", () => {
  const report = (ok, state) => ({
    summary: {
      [state]: 1,
      total: 1,
      publishable: 1,
      ok,
    },
  });
  assert.deepEqual(summarizeReports([report(true, "published")]), {
    repositories: 1,
    published: 1,
    pending: 0,
    missing: 0,
    failed: 0,
    "not-published": 0,
    "not-publishable": 0,
    unknown: 0,
    total: 1,
    publishable: 1,
    ok: true,
  });
  const mixed = summarizeReports([
    report(true, "published"),
    report(false, "pending"),
  ]);
  assert.equal(mixed.repositories, 2);
  assert.equal(mixed.published, 1);
  assert.equal(mixed.pending, 1);
  assert.equal(mixed.ok, false);
});

test("status reports an in-progress tag release and its active job", async () => {
  const { capture, calls } = captureFor(
    [
      {
        databaseId: 100,
        headBranch: "v1.24.0",
        event: "push",
        status: "completed",
        conclusion: "success",
        createdAt: "2026-11-01T00:00:00Z",
      },
      {
        databaseId: 45508961771,
        headBranch: "v1.25.1",
        event: "workflow_dispatch",
        status: "in_progress",
        conclusion: null,
        createdAt: "2026-11-24T23:19:21Z",
        url: "https://github.com/link-assistant/router/actions/runs/45508961771",
      },
      {
        databaseId: 999,
        headBranch: "main",
        event: "pull_request",
        status: "completed",
        conclusion: "failure",
        createdAt: "2026-11-25T00:00:00Z",
      },
    ],
    () => ({
      databaseId: 45508961771,
      status: "in_progress",
      conclusion: null,
      url: "https://github.com/link-assistant/router/actions/runs/45508961771",
      headBranch: "v1.25.1",
      event: "workflow_dispatch",
      jobs: [
        {
          databaseId: 1,
          name: "Test (windows-latest)",
          status: "in_progress",
          conclusion: null,
          steps: [{ name: "Run tests", status: "in_progress" }],
        },
        {
          databaseId: 2,
          name: "Test (ubuntu-latest)",
          status: "completed",
          conclusion: "success",
          steps: [{ name: "Run tests", conclusion: "success" }],
        },
      ],
    }),
  );
  const status = await collectReleaseStatus({
    slug: "link-assistant/router",
    workflow: "release.yml",
    capture,
  });
  assert.equal(status.run.id, 45508961771);
  assert.equal(status.state, "running");
  assert.equal(status.version, "1.25.1");
  assert.deepEqual(
    status.active_jobs.map((job) => job.name),
    ["Test (windows-latest)"],
  );
  assert.equal(status.completed_job_count, 1);
  assert.equal(status.job_count, 2);
  assert.equal(status.publish_failure, false);
  assert.ok(calls.some((args) => args.includes("--json")));
  assert.ok(calls.every((args) => !args.includes("--branch")));
});

test("status names a failed publishing job", async () => {
  const { capture } = captureFor(
    [
      {
        databaseId: 42,
        headBranch: "main",
        event: "push",
        status: "completed",
        conclusion: "failure",
        createdAt: "2026-11-01T00:00:00Z",
      },
    ],
    () => ({
      databaseId: 42,
      status: "completed",
      conclusion: "failure",
      headBranch: "main",
      jobs: [
        {
          name: "Publish to Crates.io",
          status: "completed",
          conclusion: "failure",
          steps: [{ name: "cargo publish", conclusion: "failure" }],
        },
      ],
    }),
  );
  const status = await collectReleaseStatus({
    slug: "acme/tool",
    workflow: "release.yml",
    capture,
  });
  assert.equal(status.state, "failed");
  assert.equal(status.publish_failure, true);
  assert.deepEqual(status.failed_jobs[0].failed_steps, ["cargo publish"]);
});

test("a tag dispatch probes the release version, not the checkout version", async () => {
  const calls = [];
  const packages = await addExpectedVersions(
    [
      {
        registry: "crates-io",
        name: "router",
        version: "1.25.0",
        publishable: true,
      },
    ],
    "1.25.1",
    {
      probePackage: async (packageInfo) => {
        calls.push(packageInfo.version);
        return {
          exists: true,
          latest_version: "1.25.1",
          manifest_version_exists: true,
          version_evidence_url: "https://crates.io/api/v1/crates/router/1.25.1",
        };
      },
    },
  );
  assert.deepEqual(calls, ["1.25.1"]);
  assert.equal(packages[0].expected_version, "1.25.1");
  assert.equal(packages[0].expected_version_exists, true);
  assert.equal(packages[0].latest_version, "1.25.1");

  const [missing] = await addExpectedVersions(
    [
      {
        registry: "npm",
        name: "new-package",
        version: "0.1.0",
        publishable: true,
      },
    ],
    "1.0.0",
    { probePackage: async () => ({ exists: false, trusted: false }) },
  );
  assert.equal(missing.expected_version_exists, false);
});

test("package states distinguish running, succeeded and missing releases", () => {
  const base = {
    registry: "npm",
    name: "app",
    version: "1.0.0",
    publishable: true,
    expected_version: "1.0.0",
    expected_version_exists: false,
  };
  assert.equal(
    packageStatuses([base], { state: "running" })[0].state,
    "pending",
  );
  const missing = packageStatuses([base], { state: "succeeded" })[0];
  assert.equal(missing.state, "missing");
  assert.match(missing.action, /finished without this version/);
  const published = packageStatuses(
    [{ ...base, expected_version_exists: true }],
    { state: "failed" },
  )[0];
  assert.equal(published.state, "published");
  assert.equal(summarizeStatus([published, missing]).ok, false);
  assert.deepEqual(summarizeStatus([published]), {
    published: 1,
    total: 1,
    publishable: 1,
    ok: true,
  });
});

test("status text shows live jobs and pending packages", () => {
  const release = {
    workflow: "release.yml",
    default_branch: "main",
    state: "running",
    run: {
      id: 45508961771,
      status: "in_progress",
      conclusion: null,
      head_branch: "v1.25.1",
      url: "https://github.com/link-assistant/router/actions/runs/45508961771",
    },
    jobs: [],
    active_jobs: [{ name: "Test (windows-latest)", status: "in_progress" }],
    failed_jobs: [],
    completed_job_count: 3,
    job_count: 4,
    publish_failure: false,
    version: "1.25.1",
  };
  const packages = packageStatuses(
    [
      {
        registry: "crates-io",
        name: "router",
        version: "1.25.0",
        expected_version: "1.25.1",
        expected_version_exists: false,
        publishable: true,
        latest_version: "1.16.0",
      },
    ],
    release,
  );
  const text = renderStatusText({
    repository: { root: "/repo", name: "link-assistant/router" },
    release,
    packages,
    summary: summarizeStatus(packages),
  });
  assert.match(text, /run 45508961771: in_progress on v1\.25\.1/);
  assert.match(text, /active jobs: Test \(windows-latest\)/);
  assert.match(text, /expected 1\.25\.1 \(absent; manifest 1\.25\.0\)/);
  assert.match(text, /crates-io: router \(pending\)/);
});

test("wait reaches a completed run without changing its evidence", async () => {
  let views = 0;
  const { capture } = captureFor(
    [
      {
        databaseId: 7,
        headBranch: "main",
        event: "push",
        status: "in_progress",
        createdAt: "2026-11-01T00:00:00Z",
      },
    ],
    () => {
      views += 1;
      return {
        databaseId: 7,
        status: views === 1 ? "in_progress" : "completed",
        conclusion: views === 1 ? null : "success",
        headBranch: "main",
        jobs: views === 1 ? [{ name: "Test", status: "in_progress" }] : [],
      };
    },
  );
  const status = await waitForReleaseStatus({
    slug: "acme/tool",
    workflow: "release.yml",
    runId: 7,
    capture,
    intervalMs: 1,
    timeoutMs: 1_000,
    sleep: async () => {},
  });
  assert.equal(views, 2);
  assert.equal(status.state, "succeeded");
  assert.equal(status.wait_timeout, undefined);
});

test("status --offline --no-release reports local coverage without gh", async () => {
  const { stdout } = await execute(
    process.execPath,
    [
      cli,
      "--repository",
      fixture,
      "status",
      "--offline",
      "--no-release",
      "--format",
      "json",
    ],
    { env: { ...process.env, PATH: "/nonexistent" } },
  );
  const report = JSON.parse(stdout);
  assert.equal(report.schema_version, 1);
  assert.equal(report.release.state, "skipped");
  assert.equal(report.summary.publishable, 9);
  assert.equal(report.summary.ok, false);
});

test("status --require-published exits nonzero for missing evidence", async () => {
  await assert.rejects(
    execute(process.execPath, [
      cli,
      "--repository",
      fixture,
      "status",
      "--offline",
      "--no-release",
      "--require-published",
    ]),
    (error) => {
      assert.equal(error.code, 1);
      assert.match(error.stdout, /Summary:/);
      return true;
    },
  );
});

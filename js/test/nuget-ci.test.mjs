import assert from "node:assert/strict";
import { test } from "node:test";
import { verifyNugetRelease } from "../src/nuget-ci.mjs";

function fixture() {
  let retries = 0;
  let reads = 0;
  const current = {
    id: 20,
    head_branch: "main",
    event: "push",
    head_sha: "new",
    status: "completed",
    conclusion: "success",
    html_url: "https://github.com/a/b/actions/runs/20",
    run_attempt: 1,
  };
  const jobs = [
    {
      id: 30,
      name: "Publish",
      conclusion: "success",
      steps: [{ name: "Publish", conclusion: "success" }],
    },
  ];
  const github = {
    rest: {
      request: async (url) =>
        url.endsWith("/a/b") ? { default_branch: "main" } : { sha: "new" },
    },
    runs: {
      list: async () => [
        current,
        { ...current, id: 10, head_sha: "old", conclusion: "failure" },
      ],
      get: async () => {
        reads++;
        return current;
      },
      jobs: async () => jobs,
      jobLog: async () =>
        '{"selected_method":"trusted-publishing","publication":"accepted","package_id":"A.Lib","version":"1.0.0"}',
    },
    repair: {
      verify: async () => {
        retries++;
        throw new Error("must not retry historical failure");
      },
    },
  };
  const session = {
    plan: {
      repository: { github_owner: "a", github_repository: "b" },
      package: { name: "A.Lib", version: "1.0.0" },
      trusted_publisher: { workflow: "ci.yml" },
    },
    options: {
      github,
      recoveryChanges: new Map([["a/b:ci.yml", ["A.Lib"]]]),
      verifyNugetPublic: async () => {},
      pollIntervalMs: 1,
      waitTimeoutMs: 100,
    },
  };
  return { session, current, jobs, retries: () => retries, reads: () => reads };
}

test("NuGet current-main success supersedes historical failures without restarting CI", async () => {
  const f = fixture();
  await verifyNugetRelease(f.session);
  assert.equal(f.retries(), 0);
  assert.equal(f.session.outcome.ci.sha, "new");
  assert.equal(f.session.outcome.consumer, "passed");
});

test("NuGet skipped gates and expired logs cannot establish verified publication", async () => {
  const f = fixture();
  f.jobs[0].conclusion = "skipped";
  await assert.rejects(verifyNugetRelease(f.session), /incomplete gates/);
  f.jobs[0].conclusion = "success";
  f.session.options.github.runs.jobLog = async () => ({
    status: "unavailable",
  });
  await assert.rejects(verifyNugetRelease(f.session), /method.*unverified/);
});

test("NuGet success for another package cannot verify the selected artifact", async () => {
  const f = fixture();
  f.session.options.github.runs.jobLog = async () =>
    '{"selected_method":"trusted-publishing","publication":"accepted","package_id":"Other.Lib","version":"1.0.0"}';
  await assert.rejects(verifyNugetRelease(f.session), /method.*unverified/);
});

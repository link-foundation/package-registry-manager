import assert from "node:assert/strict";
import { test } from "node:test";
import { getJson, probePackage } from "../src/registry-state.mjs";
import { accountCommand } from "../src/organization-setup.mjs";

const crate = { registry: "crates-io", name: "command-stream" };
test("public crate inspection leaves owner-only settings unverified without an API error (#58)", async () => {
  const errors = [];
  const urls = [];
  const state = await probePackage(crate, {
    requestLimiter: async () => {},
    onError: (error) => errors.push(error),
    fetch: async (url) => {
      urls.push(url);
      if (url.includes("github_configs")) {
        return new globalThis.Response("{}", { status: 403 });
      }
      return globalThis.Response.json({
        crate: { max_version: "1.0.0" },
        versions: [
          {
            num: "1.0.0",
            trustpub_data: {
              repository_owner: "team",
              repository_name: "project",
            },
          },
        ],
      });
    },
  });
  assert.equal(state.exists, true);
  assert.equal(state.trusted, true);
  assert.equal(state.configured_publishers, undefined);
  assert.match(state.publisher_settings_reason, /authenticated/);
  assert.equal(errors.length, 0);
  assert.equal(urls.length, 1);
});

test("crates lookups retry 403/429 with Retry-After and retain unknown HTTP evidence (#58)", async () => {
  const delays = [];
  let calls = 0;
  const errors = [];
  const options = {
    sleep: async (ms) => delays.push(ms),
    requestLimiter: async () => {},
    onError: (error) => errors.push(error),
    fetch: async () => {
      calls++;
      return calls < 3
        ? new globalThis.Response("{}", {
            status: calls === 1 ? 403 : 429,
            headers: { "retry-after": "2" },
          })
        : globalThis.Response.json({ ok: true });
    },
  };
  assert.deepEqual(
    await getJson("https://crates.io/api/v1/crates/example", options),
    { ok: true },
  );
  assert.equal(calls, 3);
  assert.deepEqual(delays, [2000, 2000]);
  assert.equal(errors.length, 0);
  const state = await probePackage(crate, {
    ...options,
    fetch: async () => new globalThis.Response("{}", { status: 403 }),
  });
  assert.equal(state.exists, undefined);
  assert.equal(state.registry_http_status, 403);
  assert.equal(errors.at(-1).http_status, 403);
});

test("migration processes clean repositories and reports skipped repositories (#58)", async () => {
  let executed = false;
  await assert.rejects(
    accountCommand(
      "setup",
      {
        org: "team",
        migrate: true,
        execute: true,
        planOptions: { migrate: true },
        github: {
          repos: {
            list: async () =>
              ["broken", "healthy"].map((name) => ({
                full_name: `team/${name}`,
                default_branch: "main",
              })),
            files: async (slug) => {
              if (slug.endsWith("broken")) {
                throw new Error("HTTP 403");
              }
              return [
                {
                  path: "Cargo.toml",
                  content: '[package]\nname="healthy"\nversion="1.0.0"',
                },
              ];
            },
          },
          runs: { list: async () => [] },
        },
        probe: async (inspection) => inspection,
        probeEnvironment: async () => ({}),
        prepareRepository: async () => {},
        executePlans: async (plans, options) => {
          executed = true;
          assert.equal(options.scanErrors, true);
          assert.equal(plans.length, 1);
          assert.equal(plans[0].package.name, "healthy");
          return [];
        },
      },
      () => {},
    ),
    /some repositories/,
  );
  assert.equal(executed, true);
});

test("concurrent clients share pacing and HTTP-date retry delays (#58)", async () => {
  const { createRequestLimiter, retryDelay } =
    await import("../src/request-policy.mjs");
  let now = 0;
  const starts = [];
  const permit = createRequestLimiter({
    now: () => now,
    sleep: async (ms) => {
      now += ms;
    },
  });
  await Promise.all(
    [1, 2, 3, 4].map(async () => {
      await permit();
      starts.push(now);
    }),
  );
  assert.deepEqual(starts, [0, 1000, 2000, 3000]);
  assert.equal(
    retryDelay(
      "Thu, 29 Oct 2026 12:00:05 GMT",
      0,
      Date.parse("2026-10-29T12:00:00Z"),
    ),
    5000,
  );
});

test("GitHub rate-limited reads retry, while writes are never replayed (#58)", async () => {
  const { retryingGithubFetch } = await import("../src/github.mjs");
  let calls = 0;
  const delays = [];
  const fetch = retryingGithubFetch({
    githubFetch: async () =>
      ++calls < 3
        ? new globalThis.Response("{}", {
            status: 403,
            headers: { "retry-after": "3" },
          })
        : globalThis.Response.json({ ok: true }),
    sleep: async (ms) => delays.push(ms),
  });
  assert.equal(
    (await fetch("https://api.github.com/repos/team/project")).status,
    200,
  );
  assert.deepEqual(delays, [3000, 3000]);
  calls = 0;
  assert.equal(
    (
      await fetch("https://api.github.com/repos/team/project", {
        method: "POST",
      })
    ).status,
    403,
  );
  assert.equal(calls, 1);
});

test("special default branches retry without a branch filter and reject other-branch evidence (#58)", async () => {
  const { scanRepositories } = await import("../src/organization-scan.mjs");
  const branches = [];
  const scan = await scanRepositories({
    org: "team",
    github: {
      repos: {
        list: async () => [
          { full_name: "team/project", default_branch: "@release" },
        ],
        files: async () => [
          {
            path: "Cargo.toml",
            content: '[package]\nname="tool"\nversion="1.0.0"',
          },
          {
            path: ".github/workflows/release.yml",
            content:
              "on: push\njobs:\n  publish:\n    steps:\n      - run: cargo publish\n",
          },
        ],
      },
      runs: {
        list: async (_, options) => {
          branches.push(options.branch);
          if (options.branch) {
            throw new Error("HTTP 404");
          }
          return [
            {
              id: 1,
              head_branch: "@release",
              event: "push",
              created_at: "2026-10-01",
              conclusion: "success",
            },
            {
              id: 2,
              head_branch: "other",
              event: "push",
              created_at: "2026-10-02",
              conclusion: "failure",
            },
          ];
        },
        logs: async () => {
          throw new Error("must not read unrelated logs");
        },
      },
    },
    probe: async (inspection) => inspection,
  });
  assert.equal(scan.repositories[0].error, undefined);
  assert.deepEqual(branches, ["@release", undefined, "@release", undefined]);
  assert.equal(scan.findings.length, 0);
});

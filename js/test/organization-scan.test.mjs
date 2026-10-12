import assert from "node:assert/strict";
import { test } from "node:test";
import { scanRepositories, renderScan } from "../src/organization-scan.mjs";

const manifests = [
  {
    path: "package.json",
    content:
      '{"name":"scan-fixture","version":"1.0.0","repository":"old/project"}',
  },
  { path: "tests/package.json", content: '{"name":"ignored-fixture"}' },
];

test("shared credential maintenance includes archived and forked consumers", async () => {
  for (const options of [{ rotateToken: "CARGO_TOKEN" }, { migrate: true }]) {
    await scanRepositories({
      org: "team",
      offline: true,
      ...options,
      github: {
        repos: {
          list: async (scope) => {
            assert.equal(scope.includeArchived, true);
            assert.equal(scope.includeForks, true);
            return [];
          },
        },
      },
    });
  }
});

test("account scan uses gh-manager files and reports all four findings (#43)", async () => {
  const calls = [];
  const github = {
    repos: {
      list: async (scope) => {
        calls.push(scope);
        return ["missing", "token", "transferred", "failing"].map((name) => ({
          full_name: `team/${name}`,
          default_branch: "main",
        }));
      },
      files: async (slug, options) => {
        assert.equal(options.content, true);
        assert.ok(options.match.includes("**/package.json"));
        calls.push(slug);
        return manifests;
      },
    },
    runs: {
      list: async (slug) => [
        {
          id: 1,
          head_branch: "main",
          event: "push",
          conclusion: slug.endsWith("failing") ? "failure" : "success",
          html_url: `https://github.com/${slug}/actions/runs/1`,
        },
      ],
      logs: async (_, options) => {
        assert.ok(options.grep.length);
        return [{ line: "ENEEDAUTH" }];
      },
    },
  };
  const scan = await scanRepositories({
    org: "team",
    github,
    probe: async (inspection) => {
      const slug = `${inspection.repository.github_owner}/${inspection.repository.github_repository}`;
      assert.equal(inspection.packages.length, 1);
      inspection.repository.release_workflow = "release.yml";
      const item = inspection.packages[0];
      item.exists_on_registry = !slug.endsWith("missing");
      item.trusted_publishing = slug.endsWith("transferred");
      if (slug.endsWith("transferred")) {
        item.configured_publishers = [
          { repository: "old/project", workflow: "release.yml" },
        ];
      }
      return inspection;
    },
  });
  assert.deepEqual(calls[0], { org: "team", user: undefined });
  assert.deepEqual(
    new Set(scan.findings.map((item) => item.type)),
    new Set([
      "unpublished",
      "published-without-trusted-publishing",
      "trusted-publisher-other-repository",
      "release-failing",
    ]),
  );
  assert.ok(
    scan.findings.find((item) => item.type === "release-failing").evidence
      .runUrl,
  );
  assert.equal(scan.repositories.length, 4);
});

for (const scope of [{ org: "team" }, { user: "team" }]) {
  test(`expired release logs retain packages and continue ${Object.keys(scope)[0]} scans (#48)`, async () => {
    const completed = [];
    const logs = [];
    const github = {
      repos: {
        list: async () =>
          ["expired", "healthy", "broken", "last"].map((name) => ({
            full_name: `team/${name}`,
            default_branch: "main",
          })),
        files: async (slug) => {
          assert.equal(
            completed.length,
            ["expired", "healthy", "broken", "last"].indexOf(
              slug.split("/")[1],
            ),
          );
          if (slug.endsWith("broken")) {
            throw new Error("files forbidden");
          }
          return manifests;
        },
      },
      runs: {
        failures: async () => {
          throw new Error("unisolated organization scan");
        },
        list: async (slug, options) => {
          assert.equal(options.workflow, "release.yml");
          assert.equal(options.branch, "main");
          return [
            {
              id: 99,
              head_branch: "feature",
              event: "pull_request",
              conclusion: "failure",
            },
            {
              id: 3,
              head_branch: "main",
              event: "push",
              created_at: "2026-10-11",
              conclusion: slug.endsWith("expired") ? "failure" : "success",
              html_url: `https://github.com/${slug}/actions/runs/3`,
            },
            {
              id: 1,
              head_branch: "main",
              event: "push",
              created_at: "2026-10-10",
              conclusion: "failure",
            },
          ];
        },
        logs: async (id, { repo }) => {
          logs.push([id, repo]);
          throw new Error("GitHub log download 410");
        },
      },
    };
    const scan = await scanRepositories({
      ...scope,
      github,
      probe: async (inspection) => {
        inspection.repository.release_workflow = "release.yml";
        inspection.packages[0].exists_on_registry = false;
        return inspection;
      },
      onRepository: (item) => completed.push(item.repository),
    });
    assert.deepEqual(completed, [
      "team/expired",
      "team/healthy",
      "team/broken",
      "team/last",
    ]);
    assert.deepEqual(logs, [[3, "team/expired"]]);
    assert.equal(scan.repositories[0].inspection.packages.length, 1);
    assert.match(scan.repositories[0].error, /410/);
    assert.equal(
      scan.findings.filter((item) => item.type === "unpublished").length,
      3,
    );
    assert.match(renderScan(scan), /410/);
    assert.match(JSON.stringify(scan), /files forbidden/);
  });
}

test("one invalid manifest preserves other packages in the same repository (#48)", async () => {
  const scan = await scanRepositories({
    org: "team",
    offline: true,
    github: {
      repos: {
        list: async () => [{ full_name: "team/polyglot" }],
        files: async () => [
          ...manifests,
          { path: "sub/package.json", content: "invalid [" },
        ],
      },
    },
  });
  assert.equal(
    scan.repositories[0].inspection.packages[0].name,
    "scan-fixture",
  );
  assert.match(scan.repositories[0].error, /sub\/package.json/);
});

test("one registry probe failure preserves another package's finding (#48)", async () => {
  const scan = await scanRepositories({
    org: "team",
    github: {
      repos: {
        list: async () => [{ full_name: "team/polyglot" }],
        files: async () => [
          ...manifests,
          {
            path: "sub/package.json",
            content: '{"name":"peer","version":"1.0.0"}',
          },
        ],
      },
    },
    probe: async (inspection) => {
      if (inspection.packages[0].name === "scan-fixture") {
        throw new Error("registry unavailable");
      }
      inspection.packages[0].exists_on_registry = false;
      return inspection;
    },
  });
  assert.equal(scan.repositories[0].inspection.packages.length, 2);
  assert.match(scan.repositories[0].error, /registry unavailable/);
  assert.equal(scan.findings[0].package, "peer");
});

test("HTTP registry errors retain unknown state and successful peer findings (#48)", async () => {
  const scan = await scanRepositories({
    org: "team",
    github: {
      repos: {
        list: async () => [{ full_name: "team/polyglot" }],
        files: async () => [
          ...manifests,
          {
            path: "sub/package.json",
            content: '{"name":"peer","version":"1.0.0"}',
          },
        ],
      },
    },
    fetch: async (url) => ({
      status: url.includes("/scan-fixture/") ? 503 : 404,
      ok: false,
    }),
  });
  const repo = scan.repositories[0];
  assert.equal(repo.inspection.packages[0].exists_on_registry, undefined);
  assert.deepEqual(repo.errors, [
    {
      stage: "registry",
      manifest: "package.json",
      package: "scan-fixture",
      message: "registry lookup returned HTTP 503",
    },
  ]);
  assert.equal(scan.findings.length, 1);
  assert.equal(scan.findings[0].package, "peer");
});

test("unknown registry state stays unknown and unsafe file paths are rejected (#43)", async () => {
  const github = {
    repos: {
      list: async () => [{ full_name: "user/project" }],
      files: async () => manifests,
    },
    runs: { list: async () => [] },
  };
  const scan = await scanRepositories({ user: "user", github, offline: true });
  assert.deepEqual(scan.findings, []);
  github.repos.files = async () => [
    { path: "../escape/package.json", content: "{}" },
  ];
  const unsafe = await scanRepositories({
    user: "user",
    github,
    offline: true,
  });
  assert.match(unsafe.repositories[0].error, /unsafe.*path/);
});

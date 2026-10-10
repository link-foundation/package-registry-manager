import assert from "node:assert/strict";
import { test } from "node:test";
import { scanRepositories } from "../src/organization-scan.mjs";

const manifests = [
  {
    path: "package.json",
    content:
      '{"name":"scan-fixture","version":"1.0.0","repository":"old/project"}',
  },
  { path: "tests/package.json", content: '{"name":"ignored-fixture"}' },
];

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
      failures: async (options) => {
        assert.equal(options.org, "team");
        assert.ok(
          options.grep.some((pattern) =>
            new RegExp(pattern, "i").test("npm ERR! E404 PUT"),
          ),
        );
        return [
          {
            repository: "team/failing",
            runUrl: "https://github.com/team/failing/actions/runs/1",
            matches: [{ line: "ENEEDAUTH" }],
          },
        ];
      },
    },
  };
  const scan = await scanRepositories({
    org: "team",
    github,
    probe: async (inspection) => {
      const slug = `${inspection.repository.github_owner}/${inspection.repository.github_repository}`;
      assert.equal(inspection.packages.length, 1);
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

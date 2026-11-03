import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { createRepoManager } from "@link-foundation/gh-manager";
import { inspectRepository } from "../src/discovery.mjs";
import {
  packageFindings,
  scanRepositories,
} from "../src/organization-scan.mjs";
import { versionSidecars } from "../src/scan-sidecars.mjs";

const root = fileURLToPath(
  new URL("../../tests/fixtures/links-notation", import.meta.url),
);
const SHA = "5ce8287c880239ae4202c9071daa4429342b4a09";
const SLUG = "link-foundation/links-notation";

async function tree(directory, prefix = "") {
  const entries = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const relative = `${prefix}${entry.name}`;
    if (entry.isDirectory()) {
      entries.push(
        ...(await tree(path.join(directory, entry.name), `${relative}/`)),
      );
    } else if (entry.isFile() && relative !== "expected-packages.json") {
      entries.push({ path: relative, type: "blob", sha: relative });
    }
  }
  return entries;
}

/** The real gh-manager repo manager over a REST double serving the fixture. */
async function fixtureGithub({ deny = () => false, truncated = false } = {}) {
  const entries = await tree(root);
  const refs = [];
  const rest = {
    request: async (apiPath) => {
      const base = `/repos/${SLUG.replace("/", "/")}`;
      if (apiPath === `${base}/commits/main`) {
        return { sha: SHA };
      }
      const treeMatch = /\/git\/trees\/([^?]+)\?recursive=1$/.exec(apiPath);
      if (treeMatch) {
        refs.push(decodeURIComponent(treeMatch[1]));
        if (truncated) {
          throw Object.assign(
            new Error("GitHub truncated a nonrecursive tree."),
            {},
          );
        }
        return { sha: SHA, tree: entries, truncated: false };
      }
      const blob = /\/git\/blobs\/(.+)$/.exec(apiPath);
      if (blob) {
        const file = decodeURIComponent(blob[1]);
        if (deny(file)) {
          throw Object.assign(new Error("Resource not accessible"), {
            status: 403,
          });
        }
        return {
          encoding: "base64",
          content: (await readFile(path.join(root, file))).toString("base64"),
        };
      }
      throw Object.assign(new Error(`unexpected ${apiPath}`), { status: 404 });
    },
  };
  return {
    refs,
    rest,
    repos: {
      ...createRepoManager({ rest }),
      list: async () => [{ full_name: SLUG, default_branch: "main" }],
    },
  };
}

const identity = (item) => ({
  registry: item.registry,
  name: item.name,
  manifest: item.manifest,
  version: item.version ?? null,
  version_source:
    item.version_source ?? item.distribution?.version_source ?? null,
  warnings: item.warnings ?? [],
  workflow: item.workflow ?? null,
});

test("account snapshot and local inspection agree on versions and sources (#80)", async () => {
  const local = await inspectRepository(root);
  const github = await fixtureGithub();
  const scan = await scanRepositories({
    org: "link-foundation",
    offline: true,
    github,
  });
  const [repository] = scan.repositories;
  assert.equal(repository.error, undefined);
  assert.equal(repository.commit, SHA);
  assert.ok(
    github.refs.every((ref) => ref === SHA),
    "every read uses one commit",
  );
  assert.deepEqual(
    repository.inspection.packages.map(identity),
    local.packages.map(identity),
  );
  const byRegistry = Object.fromEntries(
    repository.inspection.packages.map((item) => [item.registry, item]),
  );
  assert.equal(byRegistry["go-modules"].version, "v0.30.0");
  assert.equal(byRegistry["go-modules"].version_source, "go/VERSION");
  const cpp = repository.inspection.packages.find(
    (item) => item.manifest === "cpp/CMakeLists.txt",
  );
  assert.equal(cpp.version, "0.30.0");
  assert.equal(cpp.distribution.version_source, "cpp/VERSION");
  assert.equal(byRegistry.swiftpm.version, "0.30.0");
  const kotlin = repository.inspection.packages.find((item) => item.gradle);
  assert.equal(kotlin.version, "0.30.0");
});

test("unreadable sidecars keep peers and leave versions unknown with an error", async () => {
  const github = await fixtureGithub({
    deny: (file) => /(^|\/)VERSION$/.test(file),
  });
  const scan = await scanRepositories({
    org: "link-foundation",
    offline: true,
    github,
  });
  const [repository] = scan.repositories;
  assert.match(repository.error, /version files unavailable \(HTTP 403/);
  const go = repository.inspection.packages.find(
    (item) => item.registry === "go-modules",
  );
  assert.equal(go.version, null);
  const local = await inspectRepository(root);
  assert.equal(repository.inspection.packages.length, local.packages.length);
});

test("truncated trees surface as a repository error instead of a silent empty scan", async () => {
  const github = await fixtureGithub({ truncated: true });
  const scan = await scanRepositories({
    org: "link-foundation",
    offline: true,
    github,
  });
  assert.match(scan.repositories[0].error, /truncated/);
});

test("unsafe and escaping version references are rejected, not followed", () => {
  const result = versionSidecars([
    {
      path: "cpp/CMakeLists.txt",
      content:
        'file(READ "${CMAKE_CURRENT_SOURCE_DIR}/../../etc/VERSION" V)\nfile(READ "${CMAKE_CURRENT_SOURCE_DIR}/version.txt" W)',
    },
    {
      path: ".github/workflows/r.yml",
      content: "run: cat ../VERSION && cat go/VERSION",
    },
    { path: "go/go.mod", content: "module x" },
  ]);
  assert.deepEqual(result.paths, [
    "cpp/VERSION",
    "cpp/version.txt",
    "go/VERSION",
  ]);
  assert.deepEqual(result.rejected, ["../VERSION", "../etc/VERSION"]);
});

test("an absent exact version is reported even when older versions exist", () => {
  const findings = packageFindings({
    repository: {
      github_owner: "link-foundation",
      github_repository: "links-notation",
    },
    packages: [
      {
        registry: "go-modules",
        name: "github.com/link-foundation/links-notation/go",
        manifest: "go/go.mod",
        version: "v0.30.0",
        publishable: true,
        exists_on_registry: true,
        latest_version: "v0.29.0",
        manifest_version_exists: false,
      },
    ],
  });
  assert.deepEqual(
    findings.map((finding) => [
      finding.type,
      finding.evidence?.expected_version,
    ]),
    [["manifest-version-absent", "v0.30.0"]],
  );
});

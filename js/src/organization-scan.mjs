import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { inspectRepository } from "./discovery.mjs";
import { githubServices } from "./github.mjs";
import { probeRegistryState } from "./registry-state.mjs";
import { RELEASE_FAILURE_PATTERNS } from "./publishing-policy.mjs";
import { credentialPolicy } from "./credential-cycle.mjs";
import { identityCommand } from "./repository-identity.mjs";

/** Only files used by existing discovery; no full repository clones for scans. */
export const SCAN_MATCHES = [
  ...[
    "package.json",
    "Cargo.toml",
    "pyproject.toml",
    "setup.py",
    "go.mod",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "composer.json",
    "jsr.json",
    "deno.json",
    "*.csproj",
    "*.fsproj",
    "*.vbproj",
    "*.gemspec",
    "Dockerfile*",
    "Containerfile*",
    "docker-compose*.yml",
    "docker-compose*.yaml",
    "compose*.yml",
    "compose*.yaml",
  ].map((name) => `**/${name}`),
  ".github/workflows/*.yml",
  ".github/workflows/*.yaml",
  ".package-registry-manager.json",
];

export function packageFindings(inspection) {
  const slug = `${inspection.repository.github_owner}/${inspection.repository.github_repository}`;
  return inspection.packages
    .filter((item) => item.publishable)
    .flatMap((item) => {
      const common = {
        repository: slug,
        registry: item.registry,
        package: item.name,
        manifest: item.manifest,
      };
      const findings = [];
      if (item.exists_on_registry === false) {
        findings.push({ ...common, type: "unpublished" });
      }
      if (
        item.exists_on_registry === true &&
        (item.trusted_publishing === false ||
          credentialPolicy(item.registry).mode === "token")
      ) {
        findings.push({
          ...common,
          type: "published-without-trusted-publishing",
        });
      }
      const others = (item.configured_publishers ?? []).filter(
        (publisher) =>
          publisher.repository.toLowerCase() !== slug.toLowerCase(),
      );
      if (others.length) {
        findings.push({
          ...common,
          type: "trusted-publisher-other-repository",
          evidence: others,
        });
      }
      return findings;
    });
}

async function writeSnapshot(root, slug, files) {
  for (const file of files) {
    if (
      typeof file.path !== "string" ||
      file.path.includes("\\") ||
      file.path.includes(":") ||
      file.path
        .split("/")
        .some(
          (part) => !part || part === "." || part === ".." || part === ".git",
        )
    ) {
      throw new Error("unsafe repository file path");
    }
    if (typeof file.content !== "string") {
      throw new Error("gh-manager returned no file content");
    }
    const target = path.join(root, file.path);
    await mkdir(path.dirname(target), { recursive: true });
    await writeFile(target, file.content);
  }
  await mkdir(path.join(root, ".git"), { recursive: true });
  await writeFile(
    path.join(root, ".git/config"),
    `[remote "origin"]\n  url = https://github.com/${slug}.git\n`,
  );
}

/** Keep snapshots alive only for the callback; cleanup also runs after errors. */
export async function withAccountScan(options, callback) {
  if (
    Boolean(options.org) === Boolean(options.user) ||
    !/^[\w.-]+$/.test(options.org ?? options.user)
  ) {
    throw new Error("choose exactly one valid --org or --user");
  }
  const github = options.github ?? githubServices(options);
  const directory = await mkdtemp(path.join(os.tmpdir(), "prm-account-"));
  try {
    const repositories = await github.repos.list({
      org: options.org,
      user: options.user,
    });
    const scan = {
      schema_version: 1,
      account: options.org ?? options.user,
      repositories: [],
      findings: [],
    };
    for (const [index, repo] of repositories.entries()) {
      const slug = repo.full_name;
      if (!/^[\w.-]+\/[\w.-]+$/.test(slug)) {
        throw new Error("gh-manager returned an invalid repository name");
      }
      try {
        const root = path.join(directory, String(index));
        await writeSnapshot(
          root,
          slug,
          await github.repos.files(slug, {
            match: SCAN_MATCHES,
            content: true,
            ref: repo.default_branch,
          }),
        );
        const discovered = await inspectRepository(root, {
          includeSkipped: options.verbose,
        });
        const inspection = options.offline
          ? discovered
          : await (options.probe ?? probeRegistryState)(discovered, {
              ...options,
              runIdentity: async (program, args) =>
                program === "gh"
                  ? slug
                  : identityCommand(program, args, {
                      ...options,
                      repository: root,
                    }),
              repository: root,
            });
        scan.repositories.push({
          repository: slug,
          default_branch: repo.default_branch,
          inspection,
        });
        scan.findings.push(...packageFindings(inspection));
      } catch (error) {
        scan.repositories.push({ repository: slug, error: error.message });
      }
    }
    if (!options.offline) {
      let failures;
      if (options.org) {
        failures = await github.runs.failures({
          org: options.org,
          grep: RELEASE_FAILURE_PATTERNS,
        });
      } else {
        failures = [];
        for (const repo of repositories) {
          const [latest] = await github.runs.list(repo.full_name, {
            branch: repo.default_branch,
            limit: 1,
          });
          if (latest?.conclusion === "failure") {
            const matches = await github.runs.logs(latest.id, {
              repo: repo.full_name,
              grep: RELEASE_FAILURE_PATTERNS,
            });
            if (matches.length) {
              failures.push({
                repository: repo.full_name,
                runUrl: latest.html_url,
                matches,
              });
            }
          }
        }
      }
      scan.findings.push(
        ...failures.map((evidence) => ({
          repository: evidence.repository,
          type: "release-failing",
          evidence,
        })),
      );
    }
    return await callback(scan, github);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

export async function scanRepositories(options) {
  return withAccountScan(options, (scan) => {
    for (const item of scan.repositories) {
      if (item.inspection) {
        item.inspection.repository.root = `https://github.com/${item.repository}`;
      }
    }
    return scan;
  });
}

export function renderScan(scan) {
  const rows = ["Repository\tRegistry\tPackage\tFinding"];
  for (const item of scan.findings) {
    rows.push(
      `${item.repository}\t${item.registry ?? "-"}\t${item.package ?? "-"}\t${item.type.replaceAll("-", " ")}`,
    );
  }
  for (const item of scan.repositories.filter((repo) => repo.error)) {
    rows.push(`${item.repository}\t-\t-\tscan failed: ${item.error}`);
  }
  for (const repo of scan.repositories) {
    for (const pkg of repo.inspection?.packages ?? []) {
      if (
        !pkg.publishable ||
        scan.findings.some(
          (finding) =>
            finding.repository === repo.repository &&
            finding.package === pkg.name &&
            finding.registry === pkg.registry,
        )
      ) {
        continue;
      }
      const status =
        pkg.exists_on_registry === undefined
          ? "registry state unknown"
          : pkg.trusted_publishing === true
            ? "published with trusted publishing"
            : "published; trusted publishing unknown";
      rows.push(`${repo.repository}\t${pkg.registry}\t${pkg.name}\t${status}`);
    }
  }
  return `${rows.join("\n")}\n`;
}

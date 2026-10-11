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
      const item = {
        repository: slug,
        default_branch: repo.default_branch,
        errors: [],
      };
      const onError = (error) => item.errors.push(error);
      const findingStart = scan.findings.length;
      try {
        if (!/^[\w.-]+\/[\w.-]+$/.test(slug)) {
          throw new Error("gh-manager returned an invalid repository name");
        }
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
          onError,
        });
        item.inspection = discovered;
        if (!options.offline) {
          // Probe packages independently so an unexpected failure retains peers.
          const packages = [];
          for (const pkg of discovered.packages) {
            try {
              const inspection = await (options.probe ?? probeRegistryState)(
                { ...discovered, packages: [pkg] },
                {
                  ...options,
                  onError,
                  runIdentity: async (program, args) =>
                    program === "gh"
                      ? slug
                      : identityCommand(program, args, {
                          ...options,
                          repository: root,
                        }),
                  repository: root,
                },
              );
              item.inspection.repository = inspection.repository;
              packages.push(...inspection.packages);
            } catch (error) {
              onError({
                stage: "registry",
                manifest: pkg.manifest,
                package: pkg.name,
                message: error.message,
              });
              packages.push(pkg);
            }
          }
          item.inspection.packages = packages;
        }
        scan.findings.push(...packageFindings(item.inspection));
        if (!options.offline) {
          await releaseHealth(github, item, scan.findings, onError);
        }
      } catch (error) {
        onError({ stage: "repository", message: error.message });
      }
      if (item.errors.length) {
        item.error = item.errors
          .map(
            (error) =>
              `${error.manifest ? `${error.manifest}: ` : ""}${error.message}`,
          )
          .join("; ");
      } else {
        delete item.errors;
      }
      scan.repositories.push(item);
      await options.onRepository?.(
        item,
        scan.findings.slice(findingStart),
        index + 1,
        repositories.length,
      );
    }
    return await callback(scan, github);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

/** Only inspect the newest default-branch release; successful runs need no logs. */
async function releaseHealth(github, item, findings, onError) {
  const workflows = new Set(
    [
      item.inspection.repository.release_workflow,
      ...item.inspection.packages.flatMap((pkg) => [
        pkg.workflow,
        ...(pkg.workflow_candidates ?? []),
      ]),
    ].filter(Boolean),
  );
  for (const workflow of workflows) {
    try {
      if (!item.default_branch) {
        throw new Error(
          "repository has no default branch for release inspection",
        );
      }
      const runs = [];
      for (const event of ["push", "workflow_dispatch"]) {
        runs.push(
          ...(await github.runs.list(item.repository, {
            workflow,
            branch: item.default_branch,
            event,
            limit: 1,
          })),
        );
      }
      const latest = runs
        .filter(
          (run) =>
            run.head_branch === item.default_branch &&
            ["push", "workflow_dispatch"].includes(run.event),
        )
        .sort(
          (left, right) =>
            String(right.created_at).localeCompare(String(left.created_at)) ||
            right.id - left.id,
        )[0];
      if (latest?.conclusion !== "failure") {
        continue;
      }
      const evidence = {
        repository: item.repository,
        workflow,
        runId: latest.id,
        runUrl: latest.html_url,
        conclusion: latest.conclusion,
      };
      // Preserve known failed-release health even if its diagnostic logs expired.
      const finding = {
        repository: item.repository,
        type: "release-failing",
        evidence,
      };
      findings.push(finding);
      evidence.matches = await github.runs.logs(latest.id, {
        repo: item.repository,
        grep: RELEASE_FAILURE_PATTERNS,
      });
    } catch (error) {
      onError({ stage: "release", workflow, message: error.message });
    }
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

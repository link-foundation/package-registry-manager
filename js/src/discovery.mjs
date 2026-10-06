import { readFile, readdir, realpath } from "node:fs/promises";
import path from "node:path";

import { CONTAINER_FILES, containerPackages } from "./containers.mjs";
import { REGISTRIES } from "./model.mjs";
import {
  TRUSTED_REGISTRIES,
  detectPublisher,
  tokenSecretWarning,
  tokenSecrets,
} from "./publishers.mjs";
import {
  ignoreReason,
  ignoredBy,
  readIgnoreList,
  referencedByWorkflow,
  testDirectory,
  testDirectoryReason,
} from "./skips.mjs";
import { pagesWorkflow } from "./pages.mjs";
import { readWorkflows } from "./workflows.mjs";

const IGNORED_DIRECTORIES = new Set([
  ".git",
  ".package-registry-manager",
  ".venv",
  "node_modules",
  "target",
  "vendor",
]);

const MANIFEST_NAMES = new Set([
  "package.json",
  "Cargo.toml",
  "pyproject.toml",
  "setup.py",
  "go.mod",
  "pom.xml",
  "build.gradle",
  "build.gradle.kts",
  "composer.json",
]);

/**
 * Finds the packages a repository publishes. Manifests in test and example
 * directories, or matched by the ignore list, are left out; `includeSkipped`
 * lists them under `skipped`.
 */
export async function inspectRepository(repository, options = {}) {
  const root = await realpath(repository);
  const manifests = [];
  const dockerfiles = [];
  await collectManifests(root, manifests, dockerfiles);
  manifests.sort();
  const preferredManifests = manifests.filter(
    (manifest) =>
      path.basename(manifest) !== "setup.py" ||
      !manifests.includes(path.join(path.dirname(manifest), "pyproject.toml")),
  );
  const workflows = await readWorkflows(root);
  const ignore = await readIgnoreList(root);
  const skipped = [];
  const skip = (manifest, reason) => {
    skipped.push({ manifest, reason });
    return false;
  };
  const ignored = (manifest) => {
    const pattern = ignoredBy(ignore, manifest);
    return pattern !== null && !skip(manifest, ignoreReason(pattern));
  };
  // Test and example manifests stay only when a workflow publishes them.
  const kept = (manifest, publishable) => {
    const directory = testDirectory(manifest);
    return (
      directory === null ||
      (publishable && referencedByWorkflow(workflows, manifest)) ||
      skip(manifest, testDirectoryReason(directory))
    );
  };
  const parsed = await Promise.all(
    preferredManifests
      .map((manifest) => [manifest, relativePath(root, manifest)])
      .filter(([, manifest]) => !ignored(manifest))
      .map(async ([manifestPath, manifest]) => {
        try {
          return await parseManifest(manifestPath, manifest);
        } catch (error) {
          if (testDirectory(manifest) === null) {
            throw error;
          }
          return { manifest, publishable: false, invalid: true };
        }
      }),
  );
  const coordinates = await githubCoordinates(
    root,
    preferredManifests.filter((manifest) => {
      const relative = relativePath(root, manifest);
      return ignoredBy(ignore, relative) === null && !testDirectory(relative);
    }),
  );
  const publishers = new Map();
  for (const registry of TRUSTED_REGISTRIES) {
    publishers.set(registry, await detectPublisher(root, workflows, registry));
  }
  const containerFiles = dockerfiles
    .map((item) => relativePath(root, item))
    .filter((item) => !ignored(item) && kept(item, true));
  const packages = parsed
    .filter(Boolean)
    .filter((item) => kept(item.manifest, item.publishable))
    .map((item) => withPublisher(item, workflows, publishers))
    .concat(containerPackages(containerFiles, workflows, coordinates))
    .sort((left, right) => {
      const registryOrder =
        REGISTRIES.indexOf(left.registry) - REGISTRIES.indexOf(right.registry);
      return (
        registryOrder ||
        [left.manifest, left.name]
          .join("\0")
          .localeCompare([right.manifest, right.name].join("\0"))
      );
    });

  const inspection = {
    schema_version: 1,
    repository: {
      root,
      ...coordinates,
      release_workflow: releaseWorkflow(publishers),
    },
    packages,
  };
  const pages = pagesWorkflow(workflows);
  if (pages) {
    inspection.repository.pages_workflow = pages;
  }
  if (options.includeSkipped && skipped.length > 0) {
    inspection.skipped = skipped.sort((left, right) =>
      left.manifest.localeCompare(right.manifest),
    );
  }
  return inspection;
}

/**
 * Names the workflow that npm publishes from, or else the first registry's,
 * when exactly one workflow file publishes it with an OIDC token.
 */
function releaseWorkflow(publishers) {
  for (const registry of TRUSTED_REGISTRIES) {
    const workflow = publishers.get(registry)?.workflow;
    if (workflow) {
      return workflow;
    }
  }
  return null;
}

function withPublisher(item, workflows, publishers) {
  const publisher = item.publishable ? publishers.get(item.registry) : null;
  if (!publisher) {
    return item;
  }
  const result = { ...item };
  if (publisher.workflow) {
    result.workflow = publisher.workflow;
  }
  if (publisher.jobs.length > 0) {
    result.workflow_jobs = publisher.jobs;
  }
  if (publisher.environment) {
    result.environment = publisher.environment;
  }
  if (publisher.candidates.length > 1) {
    result.workflow_candidates = publisher.candidates;
  }
  const secrets = tokenSecrets(workflows, item.registry);
  const warnings = [
    ...publisher.warnings,
    ...(!publisher.workflow && publisher.candidates.length === 0
      ? [
          `no workflow publishes ${item.name} to ${item.registry}; CI releases will not reach it`,
        ]
      : []),
    ...secrets.map((secret) => tokenSecretWarning(item.registry, secret)),
  ];
  if (warnings.length > 0) {
    result.warnings = [...(item.warnings ?? []), ...warnings];
  }
  if (secrets.length > 0) {
    result.token_secrets = [...new Set(secrets.map((item) => item.secret))];
  }
  return result;
}

async function collectManifests(directory, manifests, dockerfiles) {
  const entries = await readdir(directory, { withFileTypes: true });
  entries.sort((left, right) => left.name.localeCompare(right.name));
  for (const entry of entries) {
    const item = path.join(directory, entry.name);
    if (entry.isDirectory() && !IGNORED_DIRECTORIES.has(entry.name)) {
      await collectManifests(item, manifests, dockerfiles);
    } else if (entry.isFile() && CONTAINER_FILES.has(entry.name)) {
      dockerfiles.push(item);
    } else if (
      entry.isFile() &&
      (MANIFEST_NAMES.has(entry.name) ||
        /\.(?:csproj|fsproj|vbproj)$/.test(entry.name))
    ) {
      manifests.push(item);
    }
  }
}

async function parseManifest(manifestPath, manifest) {
  const contents = await readManifest(manifestPath);
  const filename = path.basename(manifestPath);
  switch (filename) {
    case "package.json":
      return parseNpm(contents, manifest);
    case "Cargo.toml":
      return parseCargo(contents, manifest);
    case "pyproject.toml":
      return parsePyproject(contents, manifest);
    case "setup.py":
      return packageInfo(
        "pypi",
        capture(contents, /\bname\s*=\s*['"]([^'"]+)/) ??
          "unknown-python-package",
        capture(contents, /\bversion\s*=\s*['"]([^'"]+)/),
        manifest,
      );
    case "go.mod":
      return packageInfo(
        "go-modules",
        capture(contents, /^\s*module\s+(\S+)/m) ?? "unknown-go-module",
        null,
        manifest,
      );
    case "pom.xml":
      return parseMaven(contents, manifest);
    case "build.gradle":
    case "build.gradle.kts":
      return parseGradle(contents, manifest);
    case "composer.json":
      return parseComposer(contents, manifest);
    default:
      if (/\.(?:csproj|fsproj|vbproj)$/.test(filename)) {
        return parseDotnet(contents, manifestPath, manifest);
      }
      return null;
  }
}

function parseNpm(contents, manifest) {
  const metadata = parseJson(contents, manifest);
  if (!metadata.name) {
    return null;
  }
  const publishable = metadata.private !== true;
  return packageInfo(
    "npm",
    metadata.name,
    metadata.version ?? null,
    manifest,
    publishable,
    publishable ? [] : ["package.json marks this package as private"],
  );
}

function parseCargo(contents, manifest) {
  const section = tomlSection(contents, "package");
  if (!section) {
    return null;
  }
  const name = tomlString(section, "name");
  if (!name) {
    return null;
  }
  const publishable = !/^\s*publish\s*=\s*false\s*$/m.test(section);
  return packageInfo(
    "crates-io",
    name,
    tomlString(section, "version"),
    manifest,
    publishable,
    publishable ? [] : ["Cargo.toml disables publishing"],
  );
}

function parsePyproject(contents, manifest) {
  const section =
    tomlSection(contents, "project") ?? tomlSection(contents, "tool.poetry");
  const name = section && tomlString(section, "name");
  if (!name) {
    return null;
  }
  const item = packageInfo(
    "pypi",
    name,
    tomlString(section, "version"),
    manifest,
  );
  const required = tomlString(section, "requires-python");
  if (required) {
    item.requires_python = required;
  }
  return item;
}

function parseDotnet(contents, manifestPath, manifest) {
  return packageInfo(
    "nuget",
    xmlTag(contents, "PackageId") ??
      xmlTag(contents, "AssemblyName") ??
      path.basename(manifestPath, path.extname(manifestPath)),
    xmlTag(contents, "PackageVersion") ?? xmlTag(contents, "Version"),
    manifest,
  );
}

function parseMaven(contents, manifest) {
  const artifact = xmlTag(contents, "artifactId") ?? "unknown-maven-artifact";
  const group = xmlTag(contents, "groupId");
  return packageInfo(
    "maven-central",
    group ? `${group}:${artifact}` : artifact,
    xmlTag(contents, "version"),
    manifest,
  );
}

function parseGradle(contents, manifest) {
  const group = capture(contents, /^\s*group\s*=\s*['"]([^'"]+)/m);
  const artifact =
    capture(
      contents,
      /^\s*(?:archivesBaseName|rootProject\.name)\s*=\s*['"]([^'"]+)/m,
    ) ?? "gradle-project";
  return packageInfo(
    "maven-central",
    group ? `${group}:${artifact}` : artifact,
    capture(contents, /^\s*version\s*=\s*['"]([^'"]+)/m),
    manifest,
  );
}

function parseComposer(contents, manifest) {
  const metadata = parseJson(contents, manifest);
  if (!metadata.name) {
    return null;
  }
  return packageInfo(
    "packagist",
    metadata.name,
    metadata.version ?? null,
    manifest,
  );
}

function packageInfo(
  registry,
  name,
  version,
  manifest,
  publishable = true,
  problems = [],
) {
  const result = {
    registry,
    name,
    version: version ?? null,
    manifest,
    publishable,
  };
  if (problems.length > 0) {
    result.problems = problems;
  }
  return result;
}

// Editors on Windows may save a byte order mark; npm and Cargo accept it.
async function readManifest(manifest) {
  return (await readFile(manifest, "utf8")).replace(/^\uFEFF/, "");
}

function parseJson(contents, manifest) {
  try {
    return JSON.parse(contents);
  } catch (error) {
    throw new Error(`invalid JSON in ${manifest}: ${error.message}`, {
      cause: error,
    });
  }
}

function tomlSection(contents, name) {
  const escaped = name.replaceAll(".", "\\.");
  return capture(
    contents,
    new RegExp(`^\\[${escaped}\\]\\s*$([\\s\\S]*?)(?=^\\[|(?![\\s\\S]))`, "m"),
  );
}

function tomlString(section, key) {
  return capture(section, new RegExp(`^\\s*${key}\\s*=\\s*['"]([^'"]+)`, "m"));
}

function xmlTag(contents, tag) {
  return capture(
    contents,
    new RegExp(`<${tag}(?:\\s[^>]*)?>\\s*([^<]+?)\\s*</${tag}>`, "s"),
  );
}

function capture(contents, expression) {
  return expression.exec(contents)?.[1] ?? null;
}

function relativePath(root, item) {
  return path.relative(root, item).split(path.sep).join("/");
}

async function githubCoordinates(root, manifests) {
  let contents;
  try {
    contents = await readFile(path.join(root, ".git/config"), "utf8");
  } catch {
    contents = "";
  }
  const remote = capture(contents, /^\s*url\s*=\s*(\S+)/m);
  const remoteCoordinates = parseGithubUrl(remote);
  if (remoteCoordinates) {
    return remoteCoordinates;
  }

  for (const manifest of manifests) {
    const filename = path.basename(manifest);
    contents = await readManifest(manifest);
    let repository;
    if (filename === "package.json") {
      const metadata = parseJson(contents, relativePath(root, manifest));
      repository =
        typeof metadata.repository === "string"
          ? metadata.repository
          : metadata.repository?.url;
    } else if (filename === "Cargo.toml") {
      const packageSection = tomlSection(contents, "package");
      repository = packageSection && tomlString(packageSection, "repository");
    }
    const coordinates = parseGithubUrl(repository);
    if (coordinates) {
      return coordinates;
    }
  }
  return { github_owner: null, github_repository: null };
}

function parseGithubUrl(remote) {
  const normalized = remote
    ?.replace(/\.git$/, "")
    .replace(/^git\+/, "")
    .replace("git@github.com:", "https://github.com/")
    .replace("ssh://git@github.com/", "https://github.com/");
  if (!normalized?.startsWith("https://github.com/")) {
    return null;
  }
  const [github_owner, github_repository] = normalized
    .slice("https://github.com/".length)
    .split("/");
  return {
    github_owner: github_owner ?? null,
    github_repository: github_repository ?? null,
  };
}

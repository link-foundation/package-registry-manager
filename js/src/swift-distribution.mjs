import { readFile } from "node:fs/promises";
import path from "node:path";

/**
 * SwiftPM distributes a package from semantic-version tags of its Git
 * repository; there is no upload. An optional Swift package registry
 * (SE-0292) is a separate channel and is never assumed.
 */
export const SWIFT_DISTRIBUTION = Object.freeze({
  channel: "git-tag",
  registry: "none",
  tag_format: "bare semantic version (SwiftPM ignores prefixed tags)",
  identity: "repository URL",
});

const SEMVER = /^v?\d+\.\d+\.\d+(?:-[\w.-]+)?(?:\+[\w.-]+)?$/;

/**
 * Reads Package.swift as text, never running it: the package name, its
 * library products, target paths, and local path dependencies. A manifest
 * with no library product, or one that depends on an ancestor checkout by
 * path, is a consumer or example rather than a distribution.
 */
export function parseSwiftManifest(contents) {
  const code = contents
    .replace(/\/\/[^\n]*/g, "")
    .replace(/\/\*[\s\S]*?\*\//g, "");
  const name = /\bPackage\s*\(\s*name\s*:\s*"([^"]+)"/.exec(code)?.[1] ?? null;
  const libraries = [
    ...code.matchAll(/\.library\s*\(\s*name\s*:\s*"([^"]+)"/g),
  ].map((match) => match[1]);
  const executables = [
    ...code.matchAll(/\.executable\s*\(\s*name\s*:\s*"([^"]+)"/g),
  ].map((match) => match[1]);
  const paths = [
    ...code.matchAll(
      /\.(?:target|executableTarget|testTarget|binaryTarget)\s*\([^)]*?\bpath\s*:\s*"([^"]+)"/g,
    ),
  ].map((match) => match[1]);
  const localDependencies = [
    ...code.matchAll(/\.package\s*\([^)]*?\bpath\s*:\s*"([^"]+)"/g),
  ].map((match) => match[1]);
  const tools = /^\s*\/\/\s*swift-tools-version\s*:\s*([\d.]+)/m.exec(
    contents,
  )?.[1];
  return {
    name,
    libraries,
    executables,
    paths,
    localDependencies,
    tools: tools ?? null,
  };
}

/**
 * Version-file candidates for a Swift package: a sibling VERSION, then a
 * VERSION in the one top-level directory that holds every target source.
 * Only repository-relative paths below the manifest are returned.
 */
export function swiftVersionCandidates(manifest, parsed) {
  const directory = path.posix.dirname(manifest);
  const join = (...parts) =>
    path.posix.normalize(path.posix.join(...parts)).replace(/^\.\//, "");
  const candidates = [join(directory, "VERSION")];
  const roots = new Set(
    parsed.paths
      .map((item) => path.posix.normalize(item))
      .filter((item) => !item.startsWith("..") && !path.posix.isAbsolute(item))
      .map((item) => item.split("/")[0])
      .filter((item) => item && item !== "."),
  );
  if (roots.size === 1) {
    candidates.push(join(directory, [...roots][0], "VERSION"));
  }
  return [...new Set(candidates)];
}

/** Builds the SwiftPM package record for one Package.swift. */
export async function swiftDistribution(contents, manifest, root, mapping) {
  const parsed = parseSwiftManifest(contents);
  const ancestor = parsed.localDependencies.find((item) =>
    path.posix.normalize(item).startsWith(".."),
  );
  if (ancestor && !mapping) {
    return {
      manifest,
      publishable: false,
      discovery_skip: `Package.swift depends on the checkout at ${ancestor} by path: a consumer example, not a SwiftPM distribution`,
    };
  }
  if (!parsed.name || (!parsed.libraries.length && !mapping)) {
    return {
      manifest,
      publishable: false,
      discovery_skip:
        "Package.swift declares no library product; SwiftPM consumers can only depend on library products",
    };
  }
  const problems = [];
  let version = null;
  let versionSource = null;
  const candidates = mapping?.version_file
    ? [mapping.version_file]
    : swiftVersionCandidates(manifest, parsed);
  const found = [];
  for (const candidate of candidates) {
    const value = await readVersion(root, candidate);
    if (value?.error) {
      problems.push(value.error);
    } else if (value) {
      found.push([candidate, value]);
    }
  }
  if (new Set(found.map(([, value]) => value)).size > 1) {
    problems.push(
      `ambiguous Swift version: ${found.map(([file, value]) => `${file}=${value}`).join(", ")}; list the version file under "swiftpm" in .package-registry-manager.json`,
    );
  } else if (found.length) {
    [versionSource, version] = found[0];
  } else if (!problems.length) {
    problems.push(
      `no Swift version source: none of ${candidates.join(", ")} holds a semantic version; the release tag stays unknown`,
    );
  }
  const item = {
    registry: "swiftpm",
    name: mapping?.name ?? parsed.name,
    version,
    manifest,
    publishable: true,
    swift_package: {
      ...SWIFT_DISTRIBUTION,
      products: mapping?.products ?? parsed.libraries,
      ...(parsed.executables.length ? { executables: parsed.executables } : {}),
      ...(parsed.tools ? { tools_version: parsed.tools } : {}),
      tag: version ? version.replace(/^v/, "") : null,
    },
  };
  if (versionSource) {
    item.version_source = versionSource;
  }
  if (problems.length) {
    item.warnings = problems;
  }
  return item;
}

async function readVersion(root, relative) {
  const normalized = path.posix.normalize(relative);
  if (
    normalized.startsWith("..") ||
    path.posix.isAbsolute(normalized) ||
    normalized.split("/").includes("..")
  ) {
    return { error: `unsafe Swift version file reference ${relative}` };
  }
  try {
    const value = (await readFile(path.join(root, normalized), "utf8")).trim();
    return SEMVER.test(value) ? value : null;
  } catch (error) {
    if (error.code === "ENOENT" || error.code === "ENOTDIR") {
      return null;
    }
    return {
      error: `unreadable Swift version file ${relative}: ${error.code}`,
    };
  }
}

/**
 * Release evidence for SwiftPM: the job creates a tag or GitHub release and
 * names the Swift package (its manifest, version file or toolchain).
 */
export function swiftReleaseEvidence(text) {
  return (
    /\bgh\s+release\s+create\b|\bgit\s+tag\b|\bgit\s+push\b[^\n]*\btags?\b|ensure-[\w-]*tag/.test(
      text,
    ) &&
    /Package\.swift|swift\/VERSION|\bswift\s+(?:package|build|run|test)\b/.test(
      text,
    )
  );
}

/** Validation and verification steps for a Git-tag SwiftPM distribution. */
export function swiftPlan(item, slug) {
  const check = (id, title, description, args) => ({
    id,
    title,
    kind: "check",
    description,
    command: { program: "swift", args },
  });
  const tag = item.swift_package?.tag ?? item.version;
  return [
    check(
      "describe-package",
      "Resolve the Swift package manifest",
      "Let the official toolchain evaluate Package.swift in a reviewed checkout; discovery itself never runs it.",
      ["package", "describe", "--type", "json"],
    ),
    check("build-package", "Build the Swift package", "Build every product.", [
      "build",
    ]),
    check("test-package", "Test the Swift package", "Run the package tests.", [
      "test",
    ]),
    {
      id: "verify-swift-release",
      title: `Verify tag ${tag ?? "(unresolved)"}, tagged Package.swift and GitHub release`,
      kind: "api",
      description: `SwiftPM publishes by tag: read the exact tag ${tag ?? "(unresolved)"}${slug ? ` of ${slug}` : ""}, its commit, the tagged Package.swift and the public GitHub release. Missing tag, wrong commit, missing or incorrect manifest and missing release are distinct; an existing tag is never moved. ${item.workflow ? `The existing ${item.workflow} workflow creates the tag with its GITHUB_TOKEN; no SwiftPM OIDC, token or registry upload exists.` : "No workflow creates the tag yet; add the release job in a reviewed pull request."}`,
      ...(tag ? { tag } : {}),
    },
    {
      id: "verify-swift-consumer",
      title: "Verify a downstream consumer resolves the exact version",
      kind: "manual",
      state: "manual-prerequisite",
      description: `Resolve .package(url: ..., exact: "${tag ?? "<version>"}") from a clean consumer and run it (swift run --package-path <consumer>). A skipped or failed consumer job keeps the release unverified.`,
    },
  ];
}

/**
 * Composer and SwiftPM both read bare semantic-version tags of the same
 * repository, so one tag releases both: record the sharing so neither side
 * moves or recreates the other's tag.
 */
export function markSharedTags(items) {
  for (const swift of items.filter((item) => item?.registry === "swiftpm")) {
    const tag = swift.swift_package?.tag;
    const peers = items.filter(
      (item) =>
        item?.registry === "packagist" &&
        item.manifest === "composer.json" &&
        !item.discovery_skip,
    );
    if (tag && peers.length) {
      swift.swift_package.shared_tag = {
        tag,
        with: peers.map((item) => `${item.registry}:${item.name}`),
        policy:
          "create the tag once at the gated commit, verify an existing tag points there, never move it",
      };
    }
  }
}

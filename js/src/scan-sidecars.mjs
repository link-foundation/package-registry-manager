import path from "node:path";
import {
  parseSwiftManifest,
  swiftVersionCandidates,
} from "./swift-distribution.mjs";

/** At most this many sidecars per repository; the rest are reported. */
export const SIDECAR_LIMIT = 200;

const SAFE_PATH = /^[\w.+-]+(?:\/[\w.+-]+)*$/;
const GRADLE_SIDECARS = [
  "gradle.properties",
  "settings.gradle",
  "settings.gradle.kts",
  "gradle/libs.versions.toml",
];
const CMAKE_READ =
  /\bfile\s*\(\s*READ\s+"?\$\{CMAKE_CURRENT_(?:SOURCE|LIST)_DIR\}\/([\w./+-]+)"?\s+\w+/gi;
/** `cat go/VERSION`, `< cpp/VERSION`, `version-file: swift/VERSION` and the like. */
const WORKFLOW_VERSION =
  /(?:^|[\s"'=:(<])((?:[\w.+-]+\/)*VERSION)(?=$|[\s"')|;])/gm;

/**
 * Repository-relative version sources local discovery reads next to the
 * fetched manifests: sibling VERSION files (Go, C++, SwiftPM), CMake
 * `file(READ ...)` targets, SwiftPM target-directory VERSION files, Gradle
 * properties/settings/version catalogs, and VERSION files that workflow
 * commands name. Nothing is executed; unsafe or escaping references are
 * rejected, not followed.
 */
export function versionSidecars(files) {
  const known = new Set(files.map((file) => file.path));
  const wanted = new Set();
  const rejected = [];
  const add = (base, reference) => {
    const joined = path.posix.normalize(path.posix.join(base, reference));
    if (
      path.posix.isAbsolute(reference) ||
      joined.startsWith("../") ||
      joined === ".." ||
      !SAFE_PATH.test(joined) ||
      joined.split("/").some((part) => part === ".git" || part === "..")
    ) {
      rejected.push(path.posix.join(base, reference));
    } else if (!known.has(joined)) {
      wanted.add(joined);
    }
  };
  for (const file of files) {
    const name = path.posix.basename(file.path);
    const directory = path.posix.dirname(file.path);
    const text = file.content ?? "";
    if (name === "go.mod" || name === "CMakeLists.txt") {
      add(directory, "VERSION");
    }
    if (name === "CMakeLists.txt") {
      for (const match of text.matchAll(CMAKE_READ)) {
        add(directory, match[1]);
      }
    }
    if (name === "Package.swift") {
      for (const candidate of swiftVersionCandidates(
        file.path,
        parseSwiftManifest(text),
      )) {
        add(".", candidate);
      }
    }
    if (name === "build.gradle" || name === "build.gradle.kts") {
      for (const sidecar of GRADLE_SIDECARS) {
        add(directory, sidecar);
      }
    }
    if (file.path.startsWith(".github/workflows/")) {
      for (const match of text.matchAll(WORKFLOW_VERSION)) {
        add(".", match[1].replace(/^\.\//, ""));
      }
    }
  }
  const sorted = [...wanted].sort();
  return {
    paths: sorted.slice(0, SIDECAR_LIMIT),
    skipped: sorted.slice(SIDECAR_LIMIT),
    rejected: [...new Set(rejected)].sort(),
  };
}

/**
 * Fetches the version sidecars at the same commit as the manifests. A
 * failed read is an explicit repository error; the peer packages found from
 * the manifests are kept either way.
 */
export async function fetchVersionSidecars(github, slug, files, ref, onError) {
  const { paths, skipped, rejected } = versionSidecars(files);
  for (const reference of rejected) {
    onError({
      stage: "sidecar",
      manifest: reference,
      message: "unsafe version file reference was not followed",
    });
  }
  if (skipped.length) {
    onError({
      stage: "sidecar",
      message: `${skipped.length} version files beyond the ${SIDECAR_LIMIT}-file limit were not fetched; their packages keep unknown versions`,
    });
  }
  if (!paths.length) {
    return { requested: [], missing: [] };
  }
  let fetched;
  try {
    fetched = await github.repos.files(slug, {
      match: paths,
      content: true,
      ref,
    });
  } catch (error) {
    onError({
      stage: "sidecar",
      message: `version files unavailable (${error.status ? `HTTP ${error.status}: ` : ""}${error.message}); affected versions stay unknown`,
    });
    return { requested: paths, missing: paths, failed: true };
  }
  const exact = fetched.filter((file) => paths.includes(file.path));
  files.push(...exact);
  const found = new Set(exact.map((file) => file.path));
  return {
    requested: paths,
    missing: paths.filter((item) => !found.has(item)),
  };
}

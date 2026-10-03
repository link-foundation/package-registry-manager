import { readFile } from "node:fs/promises";
import path from "node:path";

/** The optional per-repository settings file read during inspection. */
export const CONFIG_FILE = ".package-registry-manager.json";

/** Directories that hold tests and examples rather than released packages. */
export const TEST_DIRECTORIES = Object.freeze([
  "tests",
  "test",
  "fixtures",
  "__fixtures__",
  "examples",
]);

/**
 * Reads the ignore list from `.package-registry-manager.json`, whose
 * `ignore` array holds glob patterns (`*`, `**`, `?`) relative to the root.
 */
export async function readIgnoreList(root) {
  let contents;
  try {
    contents = await readFile(path.join(root, CONFIG_FILE), "utf8");
  } catch (error) {
    if (error.code === "ENOENT") {
      return [];
    }
    throw error;
  }
  let config;
  try {
    config = JSON.parse(contents);
  } catch (error) {
    throw new Error(`invalid JSON in ${CONFIG_FILE}: ${error.message}`, {
      cause: error,
    });
  }
  const ignore = config?.ignore ?? [];
  if (
    !Array.isArray(ignore) ||
    !ignore.every((item) => typeof item === "string")
  ) {
    throw new Error(`${CONFIG_FILE}: "ignore" must be an array of strings`);
  }
  return ignore;
}

/** Returns the first ignore pattern that matches a path or one of its parents. */
export function ignoredBy(ignore, manifest) {
  const segments = manifest.split("/");
  const prefixes = segments.map((_, index) =>
    segments.slice(0, index + 1).join("/"),
  );
  return (
    ignore.find((pattern) => {
      const expression = globExpression(pattern);
      return prefixes.some((prefix) => expression.test(prefix));
    }) ?? null
  );
}

/** Returns the test or example directory that holds a path, if any. */
export function testDirectory(manifest) {
  return (
    manifest
      .split("/")
      .slice(0, -1)
      .find((segment) => TEST_DIRECTORIES.includes(segment)) ?? null
  );
}

/** Reports whether any workflow mentions the directory that holds a file. */
export function referencedByWorkflow(workflows, manifest) {
  const directory = path.posix.dirname(manifest);
  return workflows.some((workflow) =>
    workflow.contents
      .split(/\r?\n/)
      .some((line) => !/^\s*#/.test(line) && line.includes(directory)),
  );
}

/** Explains why a manifest in a test or example directory was skipped. */
export function testDirectoryReason(directory) {
  return `under ${directory}/, a test or example directory, and no workflow publishes it`;
}

/** Explains why a manifest matched by the ignore list was skipped. */
export function ignoreReason(pattern) {
  return `ignored by "${pattern}" in ${CONFIG_FILE}`;
}

function globExpression(pattern) {
  const normalized = pattern
    .trim()
    .replace(/^\.?\/+/, "")
    .replace(/\/+$/, "");
  let source = "";
  for (let index = 0; index < normalized.length; index += 1) {
    const character = normalized[index];
    if (character === "*" && normalized[index + 1] === "*") {
      source += ".*";
      index += 1;
      if (normalized[index + 1] === "/") {
        source = `${source.slice(0, -2)}(?:.*/)?`;
        index += 1;
      }
    } else if (character === "*") {
      source += "[^/]*";
    } else if (character === "?") {
      source += "[^/]";
    } else {
      source += character.replace(/[.+^${}()|[\]\\]/g, "\\$&");
    }
  }
  return new RegExp(`^${source}$`);
}

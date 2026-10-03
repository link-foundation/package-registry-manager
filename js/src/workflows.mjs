import { readFile, readdir } from "node:fs/promises";
import path from "node:path";

// Container registries; npm, crates.io, and PyPI use the job-aware detection
// in publishers.mjs.
const PUBLISH_PATTERNS = new Map([
  ["docker-hub", /\bDOCKER_?HUB_|\bdocker\.io\/|hub\.docker\.com/i],
  ["ghcr", /\bghcr\.io\b/],
]);

/** Reads GitHub Actions workflow files sorted by filename. */
export async function readWorkflows(root) {
  const directory = path.join(root, ".github/workflows");
  let entries;
  try {
    entries = await readdir(directory, { withFileTypes: true });
  } catch {
    return [];
  }
  const names = entries
    .filter((entry) => entry.isFile() && /\.ya?ml$/.test(entry.name))
    .map((entry) => entry.name)
    .sort((left, right) => left.localeCompare(right));
  return Promise.all(
    names.map(async (name) => ({
      name,
      contents: await readFile(path.join(directory, name), "utf8"),
    })),
  );
}

/** Returns the first workflow that publishes to the registry, if any. */
export function publishingWorkflow(workflows, registry) {
  const pattern = PUBLISH_PATTERNS.get(registry);
  return pattern
    ? (workflows.find((workflow) => pattern.test(workflow.contents)) ?? null)
    : null;
}

/** Reports whether a workflow grants `packages: write` to its token. */
export function grantsPackagesWrite(contents) {
  return (
    /^\s*packages\s*:\s*['"]?write\b/m.test(contents) ||
    /^\s*permissions\s*:\s*['"]?write-all\b/m.test(contents)
  );
}

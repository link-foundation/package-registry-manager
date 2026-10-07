import { readFile, readdir } from "node:fs/promises";
import path from "node:path";
import { parseWorkflow, executableLines } from "./publishers.mjs";
import { stripComments, commandPosition } from "./source-code.mjs";

function buildPush(lines) {
  let building = false;
  const start = lines.findIndex((line) => /^\s*steps\s*:/.test(line));
  const first = lines
    .slice(start + 1)
    .find((line) => /^\s*-\s+[\w-]+\s*:/.test(line));
  const stepIndent = first?.search(/\S/) ?? -1;
  for (const line of lines) {
    if (line.search(/\S/) === stepIndent && /^\s*-\s+[\w-]+\s*:/.test(line)) {
      building = false;
    }
    if (/^\s*-?\s*uses\s*:\s*['"]?docker\/build-push-action@/.test(line)) {
      building = true;
    }
    if (building && /^\s*push\s*:\s*true\b/.test(line)) {
      return true;
    }
  }
  return false;
}

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
  if (!["docker-hub", "ghcr"].includes(registry)) {
    return null;
  }
  return (
    workflows.find((workflow) =>
      parseWorkflow(stripComments(workflow.contents)).jobs.some((job) => {
        const text = job.lines.join("\n");
        const commands = executableLines(job.lines);
        const pushes =
          commands.some(
            (line) =>
              /\bdocker\s+push\b/.test(line) &&
              commandPosition(line.slice(0, line.indexOf("docker"))),
          ) || buildPush(job.lines);
        if (!pushes) {
          return false;
        }
        const ghcr = /\bghcr\.io\b/.test(text);
        return registry === "ghcr"
          ? ghcr
          : /\bDOCKER_?HUB_|\bdocker\.io\/|hub\.docker\.com/i.test(text) ||
              (/^\s*-?\s*uses\s*:\s*['"]?docker\/login-action@/m.test(text) &&
                !ghcr);
      }),
    ) ?? null
  );
}

/** Reports whether a workflow grants `packages: write` to its token. */
export function grantsPackagesWrite(contents) {
  contents = stripComments(contents);
  return (
    /^\s*packages\s*:\s*['"]?write\b/m.test(contents) ||
    /^\s*permissions\s*:\s*['"]?write-all\b/m.test(contents)
  );
}

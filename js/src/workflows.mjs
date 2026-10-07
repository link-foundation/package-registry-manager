import { readFile, readdir } from "node:fs/promises";
import path from "node:path";
import { parseWorkflow, executableLines } from "./publishers.mjs";
import { stripComments, commandPosition } from "./source-code.mjs";

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
          ) ||
          (/uses:\s*docker\/build-push-action@/.test(text) &&
            /^\s*push:\s*true\b/m.test(text));
        if (!pushes) {
          return false;
        }
        const ghcr = /\bghcr\.io\b/.test(text);
        return registry === "ghcr"
          ? ghcr
          : /\bDOCKER_?HUB_|\bdocker\.io\/|hub\.docker\.com/i.test(text) ||
              (/uses:\s*docker\/login-action@/.test(text) && !ghcr);
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

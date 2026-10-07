import { mkdtemp, rm } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import os from "node:os";
import path from "node:path";
import { inspectRepository } from "./discovery.mjs";

/** Translate a branch, PR number, or GitHub pull-request URL into a fetch ref. */
export function bootstrapRef(ref) {
  const pull =
    /^(?:https:\/\/github\.com\/[\w.-]+\/[\w.-]+\/pull\/)?(\d+)$/.exec(ref);
  const result = pull ? `refs/pull/${pull[1]}/head` : ref;
  if (
    !/^[\w][\w./-]*$/.test(result) ||
    result.includes("..") ||
    result.includes("//") ||
    result.endsWith("/") ||
    result.toLowerCase().endsWith(".lock")
  ) {
    throw new Error("invalid bootstrap ref");
  }
  return result;
}

/** Inspect a pushed ref in a disposable worktree without changing the checkout. */
export async function inspectReference(repository, ref, options) {
  const run = (args) => {
    const result = spawnSync("git", args, {
      cwd: repository,
      encoding: "utf8",
    });
    if (result.status !== 0) {
      throw new Error(`git failed while inspecting ref: ${result.stderr}`);
    }
    return result.stdout.trim();
  };
  const temporary = await mkdtemp(path.join(os.tmpdir(), "prm-ref-"));
  const checkout = path.join(temporary, "checkout");
  let created = false;
  try {
    run(["fetch", "origin", "HEAD"]);
    const main = run(["rev-parse", "FETCH_HEAD"]);
    run(["fetch", "origin", bootstrapRef(ref)]);
    run(["worktree", "add", "--detach", checkout, "FETCH_HEAD"]);
    created = true;
    const inspection = await inspectRepository(checkout, options);
    for (const item of inspection.packages.filter(
      (item) => item.registry === "npm",
    )) {
      const result = spawnSync("git", ["show", `${main}:${item.manifest}`], {
        cwd: repository,
        encoding: "utf8",
      });
      let relationship = "manifest is absent from remote main";
      if (result.status === 0) {
        const data = JSON.parse(result.stdout);
        relationship = `remote main is ${data.name}@${data.version}`;
      }
      item.warnings = [
        ...(item.warnings ?? []),
        `bootstrap ${item.name}@${item.version} from ${ref}; ${relationship}; the next main release must use an unpublished version`,
      ];
    }
    inspection.repository.root = await import("node:fs/promises").then(
      ({ realpath }) => realpath(repository),
    );
    return inspection;
  } finally {
    try {
      if (created) {
        run(["worktree", "remove", "--force", checkout]);
      }
    } finally {
      await rm(temporary, { recursive: true, force: true });
    }
  }
}

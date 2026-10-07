import { randomUUID } from "node:crypto";
import { setTimeout as delay } from "node:timers/promises";

/** Correlate dispatch by a unique run-name input, never by the latest unrelated run. */
export async function verifyCredentialWorkflow(session, workflow) {
  const { github_owner: owner, github_repository: repository } =
    session.plan.repository;
  const repo = `${owner}/${repository}`;
  const capture = async (args) => {
    const result = await session.capture({
      id: "verify-registry-login",
      command: { program: "gh", args },
      cwd: ".",
    });
    if (result.code !== 0) {
      throw new Error(
        "credential verification command failed; old token remains active",
      );
    }
    return String(result.stdout);
  };
  const metadata = JSON.parse(
    await capture(["repo", "view", repo, "--json", "defaultBranchRef"]),
  );
  const ref = metadata.defaultBranchRef?.name;
  if (!ref) {
    throw new Error("cannot determine verification workflow branch");
  }
  const nonce = session.options.verificationNonce ?? randomUUID();
  await capture([
    "workflow",
    "run",
    workflow,
    "--repo",
    repo,
    "--ref",
    ref,
    "-f",
    `prm_nonce=${nonce}`,
  ]);
  const deadline =
    Date.now() + (session.options.verificationTimeoutMs ?? 300_000);
  let runId;
  while (Date.now() < deadline) {
    if (!runId) {
      const runs = JSON.parse(
        await capture([
          "run",
          "list",
          "--repo",
          repo,
          "--workflow",
          workflow,
          "--event",
          "workflow_dispatch",
          "--limit",
          "50",
          "--json",
          "databaseId,displayTitle",
        ]),
      );
      const matching = runs.filter((run) => run.displayTitle?.includes(nonce));
      if (matching.length > 1) {
        throw new Error(
          "ambiguous credential verification runs; old token remains active",
        );
      }
      runId = matching[0]?.databaseId;
    }
    if (runId) {
      const run = JSON.parse(
        await capture([
          "run",
          "view",
          String(runId),
          "--repo",
          repo,
          "--json",
          "status,conclusion",
        ]),
      );
      if (run.status === "completed") {
        if (run.conclusion !== "success") {
          throw new Error(
            "registry rejected credential verification; old token remains active",
          );
        }
        return;
      }
    }
    await (session.options.verificationDelay ?? delay)(2000);
  }
  throw new Error(
    "credential verification timed out; old token remains active",
  );
}

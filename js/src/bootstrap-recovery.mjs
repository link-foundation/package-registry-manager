import { probePackage } from "./registry-state.mjs";
import {
  publisherIdentities,
  publisherMatches,
  repositorySlug,
} from "./repository-identity.mjs";

/** Bootstrap is a repaired prerequisite, separate from OIDC rejection. */
export function bootstrapNames(log) {
  const names = [];
  for (const match of log.matchAll(
    /npm owner setup required for ([^:\n]+):/gi,
  )) {
    for (const name of match[1].split(/[,\s]+/).filter(Boolean)) {
      if (!/^(?:@[a-z0-9._-]+\/)?[a-z0-9][a-z0-9._-]*$/.test(name)) {
        return [];
      }
      names.push(name);
    }
  }
  return [...new Set(names)];
}

/** Fresh name and exact publisher checks are equivalent to the owner-setup preflight. */
export async function verifyBootstrapRecovery(session) {
  if (!session.bootstrapNames?.length) {
    return;
  }
  for (const name of session.bootstrapNames) {
    const state = await (session.options.probePackage ?? probePackage)(
      { ...session.plan.package, name },
      session.options,
    );
    if (state.exists !== true) {
      throw new Error(`bootstrap preflight still unresolved for ${name}`);
    }
    const trust = session.plan.steps?.find(
      (step) => step.id === "verify-trusted-publisher",
    )?.command;
    const args = trust
      ? trust.args.map((argument) =>
          argument === session.plan.package.name ? name : argument,
        )
      : ["-y", "npm@^11.15", "trust", "list", name, "--browser=false"];
    const result = await session.capture({
      command: { program: trust?.program ?? "npx", args },
    });
    if (
      result.code !== 0 ||
      !publisherIdentities(String(result.stdout)).some((publisher) =>
        publisherMatches(publisher, session.plan.trusted_publisher),
      )
    ) {
      throw new Error(`bootstrap publisher is not verified for ${name}`);
    }
  }
  const slug = repositorySlug(session.plan.repository);
  const result = await session.capture({
    command: {
      program: "gh",
      args: [
        "api",
        `repos/${slug}/commits/${session.releaseSelection.branch}`,
        "--jq",
        ".sha",
      ],
    },
    cwd: ".",
  });
  if (
    result.code !== 0 ||
    String(result.stdout).trim() !== session.releaseSelection.run.headSha
  ) {
    throw new Error(
      "default branch changed; follow the new revision's release instead of rerunning the old SHA",
    );
  }
}

/** Retry only jobs whose own failed log contains the repaired prerequisite. */
export async function recoverBootstrap(session) {
  await verifyBootstrapRecovery(session);
  const selection = session.releaseSelection;
  const slug = repositorySlug(session.plan.repository);
  const jobIds = [];
  for (const job of selection.jobs ?? []) {
    if (job.conclusion !== "failure") {
      continue;
    }
    const log = await session.capture({
      command: {
        program: "gh",
        args: [
          "run",
          "view",
          String(selection.run.databaseId),
          "--repo",
          slug,
          "--job",
          String(job.databaseId),
          "--log",
        ],
      },
      cwd: ".",
    });
    if (log.code !== 0) {
      throw new Error(
        "causal failed-job logs are unavailable; no CI was restarted",
      );
    }
    if (
      bootstrapNames(String(log.stdout)).some((name) =>
        session.bootstrapNames.includes(name),
      )
    ) {
      jobIds.push(job.databaseId);
    }
  }
  if (!jobIds.length) {
    throw new Error(
      "no causal bootstrap job could be verified; unrelated failed jobs were not retried",
    );
  }
  const changes =
    session.options.recoveryChanges?.get(
      `${slug}:${session.plan.trusted_publisher.workflow}`,
    ) ?? [];
  if (!changes.length) {
    throw new Error(
      "no verified configuration change in this setup; CI was not restarted",
    );
  }
  const { githubServices } = await import("./github.mjs");
  const github = session.options.github ?? githubServices(session.options);
  const result = await github.repair.verify({
    repo: slug,
    workflow: session.plan.trusted_publisher.workflow,
    runId: selection.run.databaseId,
    jobIds,
    changes: changes.map(() => ({
      kind: "setting",
      changed: true,
      verified: true,
    })),
    timeout: session.options.waitTimeoutMs ?? 600000,
  });
  console.log(JSON.stringify({ bootstrap_recovery: result }));
  session.conditions.delete("release-dispatch");
  session.values.run_id = String(result.runId);
  if (result.status !== "success") {
    throw new Error(
      `repaired bootstrap CI remains ${result.status}; ${result.runUrl ?? selection.run.url}`,
    );
  }
  const { verifyNpmRelease } = await import("./npm-release.mjs");
  const packages = await verifyNpmRelease(session, result.runId);
  session.outcome = {
    status: "verified",
    url: result.runUrl,
    ci: result,
    packages,
  };
}

import { repositorySlug } from "./repository-identity.mjs";

export const RELEASE_RUN_FIELDS =
  "databaseId,conclusion,status,url,headSha,headBranch,event,createdAt";
/** Registry rejection of the workflow's trusted-publishing identity. */
export const PUBLISH_REJECTED =
  /\bE404\b|404 Not Found|invalid[-_ ]publisher|(?:npm|crates\.io|PyPI) refused the OIDC (?:package |token )?exchange\s*\((?:400|401|403|404)\)|trusted publisher[^\n]*(?:does not match|mismatch)|(?:invalid|unregistered)[-_ ](?:publishing[-_ ])?publisher/i;
const PUBLISH_JOB = /(?:^|[^a-z])(?:publish(?:ing)?|preflight)(?:[^a-z]|$)/i;

/** Only failed publish/preflight jobs (or failed publishing steps) qualify. */
export function failedPublishingJob(jobs) {
  return jobs.some(
    (job) =>
      job.conclusion === "failure" &&
      (PUBLISH_JOB.test(job.name ?? "") ||
        job.steps?.some(
          (step) =>
            step.conclusion === "failure" && PUBLISH_JOB.test(step.name ?? ""),
        )),
  );
}

/** Keep PRs, tags and other branches out even if a tool ignores its filters. */
export function eligibleReleaseRuns(runs, branch) {
  const unique = new Map();
  for (const run of runs) {
    if (
      run.headBranch === branch &&
      ["push", "workflow_dispatch"].includes(run.event)
    ) {
      unique.set(run.databaseId, run);
    }
  }
  return [...unique.values()].sort(
    (left, right) =>
      String(right.createdAt).localeCompare(String(left.createdAt)) ||
      Number(right.databaseId) - Number(left.databaseId),
  );
}

/** Cache one selection so inspection and repair retry the same release. */
export async function selectReleaseRun(session) {
  if (session.releaseSelection) {
    return session.releaseSelection;
  }
  const slug = repositorySlug(session.plan.repository);
  const capture = (args) =>
    session.capture({ command: { program: "gh", args }, cwd: "." });
  const result = await capture([
    "api",
    `repos/${slug}`,
    "--jq",
    ".default_branch",
  ]);
  const branch = String(result.stdout ?? "").trim();
  if (
    result.code !== 0 ||
    !branch ||
    branch.startsWith("-") ||
    /[\s:]/.test(branch)
  ) {
    throw new Error(
      "could not determine the release workflow's default branch",
    );
  }
  const runs = [];
  // Each event gets its own bounded history, so PR traffic cannot hide releases.
  for (const event of ["push", "workflow_dispatch"]) {
    const list = await capture([
      "run",
      "list",
      "--repo",
      slug,
      "--workflow",
      session.plan.trusted_publisher.workflow,
      "--branch",
      branch,
      "--event",
      event,
      "--limit",
      "100",
      "--json",
      RELEASE_RUN_FIELDS,
    ]);
    if (list.code !== 0) {
      throw new Error("could not list the release workflow runs");
    }
    runs.push(...JSON.parse(String(list.stdout || "[]")));
  }
  const eligible = eligibleReleaseRuns(runs, branch);
  for (const run of eligible.filter((item) => item.conclusion === "failure")) {
    const details = await capture([
      "run",
      "view",
      String(run.databaseId),
      "--repo",
      slug,
      "--json",
      "jobs",
    ]);
    if (details.code !== 0) {
      throw new Error("could not read the failed release jobs");
    }
    if (
      failedPublishingJob(JSON.parse(String(details.stdout || "{}")).jobs ?? [])
    ) {
      session.releaseSelection = { run, publishFailure: true, branch };
      return session.releaseSelection;
    }
  }
  session.releaseSelection = {
    run: eligible[0],
    publishFailure: false,
    branch,
  };
  return session.releaseSelection;
}

import { githubServices } from "./github.mjs";
import { repositorySlug } from "./repository-identity.mjs";
import { AUTH_FAILURE_PATTERNS } from "./publishing-policy.mjs";
import { fileURLToPath } from "node:url";

/** Fresh main-revision CI, actual method and exact public consumers are separate gates. */
export async function verifyNugetRelease(session) {
  const github = session.options.github ?? githubServices(session.options);
  const slug = repositorySlug(session.plan.repository);
  const publisher = session.plan.trusted_publisher;
  const repo = await github.rest.request(`/repos/${slug}`);
  const revision = await github.rest.request(
    `/repos/${slug}/commits/${encodeURIComponent(repo.default_branch)}`,
  );
  const histories = await Promise.all(
    ["push", "workflow_dispatch"].map((event) =>
      github.runs.list(slug, {
        workflow: publisher.workflow,
        branch: repo.default_branch,
        event,
        limit: 100,
      }),
    ),
  );
  const runs = histories
    .flat()
    .filter(
      (run) =>
        run.head_branch === repo.default_branch &&
        ["push", "workflow_dispatch"].includes(run.event),
    )
    .sort((a, b) => b.id - a.id);
  const current = runs.find((run) => run.head_sha === revision.sha);
  const failed = runs.find((run) => run.conclusion === "failure");
  const changes =
    session.options.recoveryChanges?.get(`${slug}:${publisher.workflow}`) ?? [];
  let result;
  if (failed && changes.length && current?.conclusion !== "success") {
    const jobs = await github.runs.jobs(slug, failed.id);
    const causal = [];
    const auth = new RegExp(AUTH_FAILURE_PATTERNS.nuget.join("|"), "i");
    for (const job of jobs.filter((job) => job.conclusion === "failure")) {
      const log = await github.runs.jobLog(slug, job.id);
      if (
        typeof log === "string" &&
        auth.test(log) &&
        /nuget|NuGet\/login/i.test(log)
      ) {
        causal.push(job.id);
      }
    }
    if (!causal.length) {
      throw new Error(
        "NuGet settings are verified but no causal authentication failure was established; unrelated CI was not restarted",
      );
    }
    if (failed.head_sha !== revision.sha && !current) {
      throw new Error(
        "NuGet code repair has no run at current main; merge the reviewed workflow and follow its push run",
      );
    }
    result = await github.repair.verify({
      repo: slug,
      workflow: publisher.workflow,
      runId: failed.id,
      jobIds: causal,
      requiredPermissions: (session.plan.package.workflow_jobs ?? []).map(
        (job) => ({ job, permission: "id-token", level: "write" }),
      ),
      changes: [
        { kind: "setting", changed: true, verified: true },
        ...(failed.head_sha !== revision.sha
          ? [{ kind: "workflow", changed: true, verified: true }]
          : []),
      ],
      ...(failed.head_sha !== revision.sha
        ? {
            revision: { ref: repo.default_branch, sha: revision.sha },
            newRunId: current.id,
          }
        : {}),
      timeout: session.options.waitTimeoutMs ?? 600000,
    });
    console.log(JSON.stringify({ nuget_ci: result }));
    if (result.status !== "success") {
      throw new Error(
        `NuGet repaired CI remains ${result.status}; ${result.runUrl ?? failed.html_url}`,
      );
    }
  }
  const id = result?.runId ?? current?.id;
  if (!id) {
    throw new Error(
      "NuGet current-main release has not executed; PR checks and settings readback do not verify publication",
    );
  }
  const deadline = Date.now() + (session.options.waitTimeoutMs ?? 600000);
  let run;
  for (;;) {
    run = await github.runs.get(slug, id);
    if (run.status === "completed") {
      break;
    }
    if (Date.now() >= deadline) {
      throw new Error(
        `NuGet current-main pipeline remains pending: ${run.html_url}`,
      );
    }
    await new Promise((resolve) =>
      setTimeout(resolve, session.options.pollIntervalMs ?? 5000),
    );
  }
  const jobs = await github.runs.jobs(slug, id);
  if (
    run.head_sha !== revision.sha ||
    run.status !== "completed" ||
    run.conclusion !== "success" ||
    !jobs.length ||
    jobs.some((job) => job.conclusion !== "success")
  ) {
    throw new Error(
      `NuGet current-main pipeline has incomplete gates: ${run.html_url}`,
    );
  }
  const methods = [];
  for (const job of jobs) {
    const source = await github.runs.jobLog(slug, job.id);
    if (typeof source !== "string") {
      continue;
    }
    for (const line of source.split("\n")) {
      const start = line.indexOf('{"selected_method"');
      if (start < 0) {
        continue;
      }
      try {
        const evidence = JSON.parse(line.slice(start));
        if (
          ["trusted-publishing", "token-fallback"].includes(
            evidence.selected_method,
          ) &&
          evidence.publication === "accepted" &&
          evidence.package_id?.toLowerCase() ===
            session.plan.package.name.toLowerCase() &&
          /^[0-9][0-9a-z.+-]*$/i.test(evidence.version ?? "")
        ) {
          methods.push({ job: job.name, ...evidence });
        }
      } catch {
        /* Invalid or unavailable logs cannot establish a method. */
      }
    }
  }
  if (!methods.length) {
    throw new Error(
      "NuGet authentication method and actual upload are unverified; use the reviewed publication controller in current-main CI",
    );
  }
  const fresh = await github.rest.request(
    `/repos/${slug}/commits/${encodeURIComponent(repo.default_branch)}`,
  );
  if (fresh.sha !== run.head_sha) {
    throw new Error(
      "main advanced during NuGet verification; follow its new run before claiming completion",
    );
  }
  const versions = [...new Set(methods.map((method) => method.version))];
  if (versions.length !== 1) {
    throw new Error("NuGet actual released version is ambiguous");
  }
  const released = { ...session.plan.package, version: versions[0] };
  if (session.options.verifyNugetPublic) {
    await session.options.verifyNugetPublic(released);
  } else {
    const checked = await session.capture({
      command: {
        program: "python3",
        args: [
          fileURLToPath(new URL("./nuget-publish.py", import.meta.url)),
          "--verify-public",
          session.plan.package.name,
          released.version,
        ],
      },
      cwd: ".",
    });
    if (checked.code !== 0) {
      throw new Error(
        "exact NuGet public download and consumer restore/build remain unverified",
      );
    }
  }
  session.outcome = {
    status: "verified",
    url: run.html_url,
    ci: {
      run_id: id,
      attempt: run.run_attempt,
      sha: run.head_sha,
      jobs,
      authentication: methods,
    },
    public_version: released.version,
    consumer: "passed",
  };
  console.log(JSON.stringify(session.outcome));
}

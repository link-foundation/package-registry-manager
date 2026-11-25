import { execFile } from "node:child_process";
import { promisify } from "node:util";

import { failedPublishingJob } from "./release-runs.mjs";
import { repositorySlug } from "./repository-identity.mjs";
import { probePackage } from "./registry-state.mjs";

const execute = promisify(execFile);

export const STATUS_RUN_FIELDS =
  "databaseId,status,conclusion,url,headSha,headBranch,event,createdAt,updatedAt";
export const STATUS_DETAIL_FIELDS = `${STATUS_RUN_FIELDS},jobs`;

const FAILED_CONCLUSIONS = new Set([
  "action_required",
  "cancelled",
  "failure",
  "startup_failure",
  "timed_out",
]);
const RELEASE_BRANCH =
  /^v?\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;
const PUBLISHABLE_STATES = new Set(["published"]);

/** A release tag branch such as `v1.25.1` also names the version it publishes. */
export function releaseVersionFromBranch(branch) {
  return RELEASE_BRANCH.test(branch ?? "") ? branch.replace(/^v/, "") : null;
}

export function runState(run) {
  if (!run) {
    return "missing";
  }
  if (run.status && run.status !== "completed") {
    return "running";
  }
  if (FAILED_CONCLUSIONS.has(run.conclusion)) {
    return "failed";
  }
  return run.conclusion === "success" ? "succeeded" : "completed";
}

/** Keep only push/manual runs on the default branch or a release tag. */
export function eligibleStatusRuns(runs, branch) {
  const unique = new Map();
  for (const run of runs) {
    if (!["push", "workflow_dispatch"].includes(run.event)) {
      continue;
    }
    const head = run.headBranch ?? run.head_branch;
    if (branch && head !== branch && !RELEASE_BRANCH.test(head ?? "")) {
      continue;
    }
    unique.set(run.databaseId ?? run.id, run);
  }
  return [...unique.values()].sort(
    (left, right) =>
      String(right.createdAt ?? right.created_at).localeCompare(
        String(left.createdAt ?? left.created_at),
      ) ||
      Number(right.databaseId ?? right.id) - Number(left.databaseId ?? left.id),
  );
}

/** Run gh without throwing, so a status report can retain partial evidence. */
export async function captureGh({ command: { program, args }, cwd = "." }) {
  try {
    const { stdout } = await execute(program, args, {
      cwd,
      timeout: 30_000,
      maxBuffer: 16 * 1024 * 1024,
    });
    return { code: 0, stdout };
  } catch (error) {
    return {
      code: typeof error.code === "number" ? error.code : 1,
      stdout: error.stdout ?? "",
      stderr: error.stderr ?? error.message,
    };
  }
}

export async function readDefaultBranch({ slug, capture = captureGh }) {
  const result = await capture({
    command: {
      program: "gh",
      args: ["api", `repos/${slug}`, "--jq", ".default_branch"],
    },
  });
  const branch = String(result.stdout ?? "").trim();
  return result.code === 0 &&
    branch &&
    !branch.startsWith("-") &&
    !/[\s:]/.test(branch)
    ? branch
    : null;
}

function normalizedRun(run) {
  if (!run) {
    return null;
  }
  return {
    id: Number(run.databaseId ?? run.id),
    status: run.status ?? "unknown",
    conclusion: run.conclusion ?? null,
    url: run.url ?? run.html_url ?? null,
    head_sha: run.headSha ?? run.head_sha ?? null,
    head_branch: run.headBranch ?? run.head_branch ?? null,
    event: run.event ?? null,
    created_at: run.createdAt ?? run.created_at ?? null,
    updated_at: run.updatedAt ?? run.updated_at ?? null,
  };
}

function summarizedSteps(steps) {
  return (steps ?? []).map((step) => ({
    name: step.name,
    status: step.status,
    conclusion: step.conclusion,
    number: step.number,
  }));
}

function summarizedJobs(jobs) {
  return (jobs ?? []).map((job) => ({
    id: Number(job.databaseId ?? job.id),
    name: job.name,
    status: job.status,
    conclusion: job.conclusion,
    url: job.url ?? job.html_url ?? null,
    started_at: job.startedAt ?? null,
    completed_at: job.completedAt ?? null,
    failed_steps: (job.steps ?? [])
      .filter((step) => step.conclusion === "failure")
      .map((step) => step.name),
    steps: summarizedSteps(job.steps),
  }));
}

async function listRuns({ slug, workflow, capture }) {
  const result = await capture({
    command: {
      program: "gh",
      args: [
        "run",
        "list",
        "--repo",
        slug,
        "--workflow",
        workflow,
        "--limit",
        "100",
        "--json",
        STATUS_RUN_FIELDS,
      ],
    },
    cwd: ".",
  });
  if (result.code !== 0) {
    return {
      error: `could not list release workflow runs${
        result.stderr ? `: ${String(result.stderr).trim()}` : ""
      }`,
    };
  }
  try {
    return { runs: JSON.parse(String(result.stdout || "[]")) };
  } catch {
    return { error: "gh returned invalid release run history" };
  }
}

async function readRun({ slug, runId, capture }) {
  const result = await capture({
    command: {
      program: "gh",
      args: [
        "run",
        "view",
        String(runId),
        "--repo",
        slug,
        "--json",
        STATUS_DETAIL_FIELDS,
      ],
    },
    cwd: ".",
  });
  if (result.code !== 0) {
    return {
      error: `could not read release run ${runId}${
        result.stderr ? `: ${String(result.stderr).trim()}` : ""
      }`,
    };
  }
  try {
    const details = JSON.parse(String(result.stdout || "{}"));
    return {
      run: normalizedRun({
        ...details,
        databaseId: details.databaseId ?? runId,
      }),
      jobs: details.jobs ?? [],
    };
  } catch {
    return { error: `gh returned invalid details for release run ${runId}` };
  }
}

function releaseRecord({ workflow, branch, run, jobs, error }) {
  const summarized = summarizedJobs(jobs);
  return {
    workflow,
    default_branch: branch ?? null,
    state: runState(run),
    run: normalizedRun(run),
    jobs: summarized,
    active_jobs: summarized.filter(
      (job) => job.status && job.status !== "completed",
    ),
    failed_jobs: summarized.filter((job) => job.conclusion === "failure"),
    completed_job_count: summarized.filter((job) => job.status === "completed")
      .length,
    job_count: summarized.length,
    publish_failure: failedPublishingJob(jobs),
    version: releaseVersionFromBranch(run?.headBranch ?? run?.head_branch),
    ...(error ? { error } : {}),
  };
}

/** Read the newest release run, or a specific run, with its live job state. */
export async function collectReleaseStatus(options) {
  const { slug, workflow, runId, capture = captureGh } = options;
  if (!slug) {
    return {
      workflow: workflow ?? null,
      default_branch: null,
      state: "unavailable",
      run: null,
      jobs: [],
      active_jobs: [],
      failed_jobs: [],
      completed_job_count: 0,
      job_count: 0,
      publish_failure: false,
      version: null,
      error: "repository has no GitHub origin",
    };
  }
  if (!workflow) {
    return {
      workflow: null,
      default_branch: null,
      state: "unavailable",
      run: null,
      jobs: [],
      active_jobs: [],
      failed_jobs: [],
      completed_job_count: 0,
      job_count: 0,
      publish_failure: false,
      version: null,
      error: "no release workflow was detected; pass --workflow <file>",
    };
  }
  const branch = options.branch ?? (await readDefaultBranch({ slug, capture }));
  let selected = runId;
  if (selected === undefined) {
    const history = await listRuns({ slug, workflow, capture });
    if (history.error) {
      return releaseRecord({
        workflow,
        branch,
        error: history.error,
      });
    }
    const eligible = eligibleStatusRuns(history.runs, branch);
    selected = eligible[0]?.databaseId ?? eligible[0]?.id;
  }
  if (selected === undefined) {
    return releaseRecord({ workflow, branch });
  }
  const details = await readRun({ slug, runId: selected, capture });
  if (details.error) {
    return releaseRecord({ workflow, branch, error: details.error });
  }
  return releaseRecord({
    workflow,
    branch,
    run: details.run,
    jobs: details.jobs,
  });
}

/** Poll until a run appears and/or finishes; a failed run is returned as-is. */
export async function waitForReleaseStatus(options) {
  const {
    intervalMs = 5_000,
    timeoutMs = 30 * 60_000,
    sleep = (milliseconds) =>
      new Promise((resolve) => setTimeout(resolve, milliseconds)),
    onPoll,
  } = options;
  const started = Date.now();
  let runId = options.runId;
  for (;;) {
    const status = await collectReleaseStatus({ ...options, runId });
    onPoll?.(status);
    runId ??= status.run?.id;
    if (!["running", "missing"].includes(status.state)) {
      return { ...status, waited_ms: Date.now() - started };
    }
    const remaining = timeoutMs - (Date.now() - started);
    if (remaining <= 0) {
      return {
        ...status,
        wait_timeout: true,
        waited_ms: Date.now() - started,
      };
    }
    await sleep(Math.max(1, Math.min(intervalMs, remaining)));
  }
}

/**
 * Probe the version named by the release tag. A tag dispatch runs from the
 * release commit, so the checkout's manifest may still be one version behind.
 */
export async function addExpectedVersions(
  packages,
  expectedVersion,
  options = {},
) {
  const probe = options.probePackage ?? probePackage;
  return Promise.all(
    packages.map(async (packageInfo) => {
      const expected = expectedVersion ?? packageInfo.version ?? null;
      const result = { ...packageInfo, expected_version: expected };
      if (!packageInfo.publishable || !expected) {
        return result;
      }
      if (packageInfo.version === expected) {
        result.expected_version_exists = packageInfo.manifest_version_exists;
        return result;
      }
      const errors = [];
      try {
        const state = await probe(
          { ...packageInfo, version: expected },
          {
            ...options,
            onError: (error) => {
              errors.push(error);
              options.onError?.(error);
            },
          },
        );
        if (state.exists !== undefined) {
          result.exists_on_registry = state.exists;
        }
        if (state.latest_version !== undefined) {
          result.latest_version = state.latest_version;
        }
        if (state.manifest_version_exists !== undefined) {
          result.expected_version_exists = state.manifest_version_exists;
        } else if (state.exists === false) {
          result.expected_version_exists = false;
        }
        if (state.version_state !== undefined) {
          result.expected_version_state = state.version_state;
        }
        if (state.version_evidence_url !== undefined) {
          result.expected_version_evidence_url = state.version_evidence_url;
        }
        if (state.registry_state_reason !== undefined) {
          result.registry_state_reason = state.registry_state_reason;
        } else if (errors.length) {
          result.registry_state_reason = errors.at(-1).message;
        }
      } catch (error) {
        result.expected_version_error = error.message;
      }
      return result;
    }),
  );
}

const ACTIONS = {
  published: "none",
  pending: "wait for the release workflow to finish",
  failed: "read the failed jobs, then retry the release",
  missing: "the release finished without this version; inspect its publish job",
  "not-published":
    "run the release workflow; this version is not on the registry",
  "not-publishable": "none",
  unknown: "look up the version on the registry and the release run",
};

/** Combine registry evidence with the release that should have produced it. */
export function packageStatuses(packages, release) {
  return packages.map((packageInfo) => {
    const expected =
      packageInfo.expected_version ?? packageInfo.version ?? null;
    let state;
    if (!packageInfo.publishable) {
      state = "not-publishable";
    } else if (packageInfo.expected_version_exists === true) {
      state = "published";
    } else if (release?.state === "running" || release?.wait_timeout) {
      state = "pending";
    } else if (release?.state === "failed") {
      state = "failed";
    } else if (release?.state === "succeeded") {
      state = "missing";
    } else if (packageInfo.expected_version_exists === false) {
      state = "not-published";
    } else {
      state = "unknown";
    }
    return {
      ...packageInfo,
      expected_version: expected,
      state,
      action: ACTIONS[state],
    };
  });
}

/** Choose the release workflow that covers the selected packages. */
export function selectStatusWorkflow(
  inspection,
  packages = inspection.packages,
) {
  if (inspection.repository.release_workflow) {
    return inspection.repository.release_workflow;
  }
  const workflows = new Set(
    packages
      .filter((packageInfo) => packageInfo.publishable && packageInfo.workflow)
      .map((packageInfo) => packageInfo.workflow),
  );
  return workflows.size === 1 ? [...workflows][0] : null;
}

/** Aggregate per-repository status reports for account scans. */
export function summarizeReports(reports) {
  const summary = {
    repositories: reports.length,
    published: 0,
    pending: 0,
    missing: 0,
    failed: 0,
    "not-published": 0,
    "not-publishable": 0,
    unknown: 0,
    total: 0,
    publishable: 0,
    ok: true,
  };
  for (const report of reports) {
    for (const key of [
      "published",
      "pending",
      "missing",
      "failed",
      "not-published",
      "not-publishable",
      "unknown",
    ]) {
      summary[key] += report.summary[key] ?? 0;
    }
    summary.total += report.summary.total;
    summary.publishable += report.summary.publishable;
    summary.ok &&= report.summary.ok;
  }
  return summary;
}

export function summarizeStatus(packages) {
  const counts = {};
  for (const packageInfo of packages) {
    counts[packageInfo.state] = (counts[packageInfo.state] ?? 0) + 1;
  }
  const publishable = packages.filter((item) => item.publishable);
  return {
    ...counts,
    total: packages.length,
    publishable: publishable.length,
    ok: publishable.every((item) => PUBLISHABLE_STATES.has(item.state)),
  };
}

function packageLine(packageInfo, verbose) {
  const lines = [
    `- ${packageInfo.registry}: ${packageInfo.name} (${packageInfo.state})`,
  ];
  if (packageInfo.expected_version) {
    const manifest =
      packageInfo.version &&
      packageInfo.version !== packageInfo.expected_version
        ? `; manifest ${packageInfo.version}`
        : "";
    const present =
      packageInfo.expected_version_exists === true
        ? "published"
        : packageInfo.expected_version_exists === false
          ? "absent"
          : "unknown";
    lines.push(
      `  expected ${packageInfo.expected_version} (${present}${manifest})`,
    );
  }
  if (packageInfo.latest_version) {
    lines.push(`  registry latest: ${packageInfo.latest_version}`);
  }
  if (packageInfo.publishing_policy) {
    lines.push(`  authentication: ${packageInfo.publishing_policy}`);
  }
  if (packageInfo.trusted_publishing !== undefined) {
    lines.push(
      `  trusted publishing: ${
        packageInfo.trusted_publishing ? "verified" : "not verified"
      }`,
    );
  }
  if (packageInfo.registry_state_reason) {
    lines.push(`  registry state: ${packageInfo.registry_state_reason}`);
  }
  if (packageInfo.expected_version_error) {
    lines.push(`  registry error: ${packageInfo.expected_version_error}`);
  }
  if (packageInfo.action && packageInfo.action !== "none") {
    lines.push(`  next: ${packageInfo.action}`);
  }
  if (verbose) {
    const evidence =
      packageInfo.expected_version_evidence_url ??
      packageInfo.version_evidence_url ??
      packageInfo.registry_evidence_url;
    if (evidence) {
      lines.push(`  evidence: ${evidence}`);
    }
    for (const warning of packageInfo.warnings ?? []) {
      lines.push(`  warning: ${warning}`);
    }
  }
  return lines;
}

export function renderStatusText(report, options = {}) {
  const lines = [];
  const repository = report.repository;
  lines.push(`Repository: ${repository.name ?? repository.root}`);
  if (repository.root && repository.name) {
    lines.push(`  ${repository.root}`);
  }
  const release = report.release;
  if (release.workflow) {
    lines.push(
      `Release: ${release.workflow}${
        release.default_branch
          ? ` (default branch ${release.default_branch})`
          : ""
      }`,
    );
  }
  if (release.error) {
    lines.push(`  error: ${release.error}`);
  }
  if (release.run) {
    const run = release.run;
    const where = run.head_branch ? ` on ${run.head_branch}` : "";
    lines.push(
      `  run ${run.id}: ${run.status}${
        run.conclusion ? ` (${run.conclusion})` : ""
      }${where}`,
    );
    if (run.url) {
      lines.push(`  ${run.url}`);
    }
    if (release.active_jobs.length) {
      lines.push(
        `  active jobs: ${release.active_jobs
          .map((job) => `${job.name} (${job.status})`)
          .join(", ")}`,
      );
    }
    if (release.failed_jobs.length) {
      lines.push(
        `  failed jobs: ${release.failed_jobs
          .map((job) => {
            const steps = job.failed_steps.length
              ? `: ${job.failed_steps.join(", ")}`
              : "";
            return `${job.name}${steps}`;
          })
          .join(", ")}`,
      );
    }
    if (release.publish_failure) {
      lines.push("  publish failure: yes");
    }
    lines.push(
      `  jobs: ${release.completed_job_count}/${release.job_count} completed`,
    );
    if (options.verbose) {
      for (const job of release.jobs) {
        lines.push(
          `    - ${job.name}: ${job.status}${
            job.conclusion ? ` (${job.conclusion})` : ""
          }`,
        );
      }
    }
  } else if (release.state === "missing") {
    lines.push("  no release run found yet");
  }
  if (release.wait_timeout) {
    lines.push("  wait timed out before the release finished");
  }
  lines.push("Packages:");
  if (!report.packages.length) {
    lines.push("  no matching packages");
  }
  for (const packageInfo of report.packages) {
    lines.push(...packageLine(packageInfo, options.verbose));
  }
  const summary = report.summary;
  const states = [
    "published",
    "pending",
    "missing",
    "failed",
    "not-published",
    "unknown",
  ]
    .filter((state) => summary[state])
    .map((state) => `${summary[state]} ${state}`);
  lines.push(
    `Summary: ${summary.publishable} publishable package(s); ${
      states.length ? states.join(", ") : "no state"
    }; ${summary.ok ? "all published" : "not all published"}`,
  );
  return `${lines.join("\n")}\n`;
}

/** Build the report shared by the CLI and its tests. */
export async function buildStatusReport(inspection, options = {}) {
  const filtered = inspection.packages.filter(
    (packageInfo) =>
      (!options.registries?.length ||
        options.registries.includes(packageInfo.registry)) &&
      (!options.package || packageInfo.name === options.package),
  );
  const release = options.release ?? { state: "unavailable" };
  const packages = options.expectedVersions
    ? await addExpectedVersions(
        filtered,
        release.version,
        options.probeOptions ?? {},
      )
    : filtered;
  const statuses = packageStatuses(packages, release);
  return {
    schema_version: 1,
    repository: {
      root: inspection.repository.root,
      name: repositorySlug(inspection.repository),
    },
    release,
    packages: statuses,
    summary: summarizeStatus(statuses),
  };
}

import { githubServices } from "./github.mjs";
import { registryTokenSecrets } from "./publishers.mjs";

/** Dynamic secret indexing and inherited secrets prevent a proof of non-use. */
export function readsSecret(files, secret) {
  return files.some((file) => {
    if (typeof file.content !== "string") {
      throw new Error("incomplete workflow contents");
    }
    const text = file.content
      .split(/\r?\n/)
      .filter((line) => !line.trimStart().startsWith("#"))
      .join("\n");
    return (
      new RegExp(`\\bsecrets\\.${secret}\\b`).test(text) ||
      /\bsecrets\s*\[|\bsecrets:\s*inherit\b/.test(text)
    );
  });
}

/** Successful authentication and a real upload in the same successful job. */
export function publishedThroughOidc(job, log, registry) {
  if (job.conclusion !== "success") {
    return false;
  }
  const steps = (job.steps ?? []).filter(
    (step) => step.conclusion === "success",
  );
  if (!steps.some((step) => /publish|upload|twine/i.test(step.name))) {
    return false;
  }
  if (registry === "crates-io") {
    return (
      steps.some((step) =>
        /Authenticate with crates.io|crates-io-auth-action/i.test(step.name),
      ) && /Published\s+\S+\s+v\d/.test(log)
    );
  }
  if (steps.some((step) => /token fallback/i.test(step.name))) {
    return false;
  }
  if (registry === "npm") {
    return (
      steps.some((step) => /OIDC/i.test(step.name)) &&
      /^.*\+\s+(?:@[^/\s]+\/)?[^@\s]+@\d/m.test(log)
    );
  }
  if (registry === "pypi") {
    return (
      steps.some(
        (step) =>
          /prm_oidc|OIDC/i.test(step.name) && !/fallback/.test(step.name),
      ) && /Successfully uploaded|View at:\s*https:\/\/pypi\.org/.test(log)
    );
  }
  return false;
}

async function defaultWorkflows(github, slug) {
  const repository = await github.rest.request(`/repos/${slug}`);
  const commit = await github.rest.request(
    `/repos/${slug}/commits/${encodeURIComponent(repository.default_branch)}`,
  );
  if (!commit.sha || !repository.default_branch) {
    throw new Error("cannot verify default-branch identity");
  }
  const files = await github.repos.files(slug, {
    match: [".github/workflows/*.yml", ".github/workflows/*.yaml"],
    content: true,
    ref: commit.sha,
  });
  if (!Array.isArray(files)) {
    throw new Error("incomplete workflow listing");
  }
  return { branch: repository.default_branch, sha: commit.sha, files };
}

/** Delete only after publication at the current default head; retain shared org secrets. */
export async function cleanupMigratedSecrets(session) {
  const { plan, options } = session;
  const github = options.github ?? githubServices(options);
  const owner = plan.repository.github_owner;
  const slug = `${owner}/${plan.repository.github_repository}`;
  const names = [
    ...new Set([
      ...registryTokenSecrets(plan.registry),
      ...(options.secretName && /^[A-Z_][A-Z0-9_]*$/.test(options.secretName)
        ? [options.secretName]
        : []),
    ]),
  ];
  const current = await defaultWorkflows(github, slug);
  const stillRead = names.filter((name) => readsSecret(current.files, name));
  const workflow = plan.trusted_publisher?.workflow ?? plan.package.workflow;
  const runs = await github.runs.list(slug, {
    workflow,
    branch: current.branch,
    limit: 100,
  });
  let verified = false;
  for (const run of runs.filter(
    (run) =>
      run.head_sha === current.sha &&
      run.conclusion === "success" &&
      ["push", "workflow_dispatch", "release"].includes(run.event),
  )) {
    for (const job of await github.runs.jobs(slug, run.id)) {
      if (
        job.conclusion === "success" &&
        publishedThroughOidc(
          job,
          await github.runs.jobLog(slug, job.id),
          plan.registry,
        )
      ) {
        verified = true;
        break;
      }
    }
    if (verified) {
      break;
    }
  }
  if (!verified) {
    session.outcome = { status: "awaiting-oidc-release" };
    console.log(
      `${slug}: legacy secrets retained until ${workflow} publishes through OIDC at default-branch commit ${current.sha}. Re-run setup --migrate after the release succeeds.`,
    );
    return;
  }
  if (!options.deleteTokenSecrets) {
    session.outcome = {
      status: "fallback-retained",
      oidc_verified: true,
      fallback_secrets: stillRead,
    };
    console.log(
      `${slug}: OIDC publication verified; token secrets (${stillRead.join(", ") || "none referenced"}) and expiry metadata retained for fallback. Use --delete-token-secrets only after removing every consumer.`,
    );
    return;
  }
  if (stillRead.length) {
    session.outcome = { status: "migration-required" };
    console.log(
      `${slug} still reads ${stillRead.join(", ")}; merge the conversion PR before cleanup.`,
    );
    return;
  }
  const deletions = [];
  for (const name of names) {
    const manager = github.secrets({ repo: slug });
    if (await manager.getMetadata(name)) {
      deletions.push({ scope: slug, manager, name });
    }
  }
  const snapshots = [];
  if (options.accountScan && !options.user) {
    if (options.scanErrors) {
      throw new Error(
        "organization secret cleanup requires a complete account scan",
      );
    }
    const repositories = await github.repos.list({
      org: owner,
      includeArchived: true,
      includeForks: true,
    });
    for (const repo of repositories) {
      snapshots.push({
        slug: repo.full_name,
        ...(await defaultWorkflows(github, repo.full_name)),
      });
    }
    for (const name of names) {
      const consumers = snapshots
        .filter((item) => readsSecret(item.files, name))
        .map((item) => item.slug);
      if (consumers.length) {
        console.log(
          `Keep organization ${name}; migrate: ${consumers.join(", ")}`,
        );
        continue;
      }
      const manager = github.secrets({ org: owner });
      if (await manager.getMetadata(name)) {
        deletions.push({ scope: owner, manager, name });
      }
    }
  } else {
    console.log(
      `Organization secrets retained; run setup --org ${owner} --registry ${plan.registry} --migrate to check every consumer.`,
    );
  }
  if (
    deletions.length &&
    !options.yes &&
    !/^y(?:es)?$/i.test(
      await session.prompt(
        `Delete verified legacy secrets ${deletions.map((item) => `${item.scope}:${item.name}`).join(", ")}? [y/N] `,
      ),
    )
  ) {
    session.outcome = { status: "cleanup-declined" };
    return;
  }
  // The confirmation can take time; check all consumers again after it returns.
  for (const snapshot of snapshots) {
    const latest = await github.rest.request(
      `/repos/${snapshot.slug}/commits/${encodeURIComponent(snapshot.branch)}`,
    );
    if (latest.sha !== snapshot.sha) {
      throw new Error(
        "default branch changed during secret cleanup; secrets retained",
      );
    }
  }
  const latest = await github.rest.request(
    `/repos/${slug}/commits/${encodeURIComponent(current.branch)}`,
  );
  if (latest.sha !== current.sha) {
    throw new Error(
      "default branch changed during secret cleanup; secrets retained",
    );
  }
  for (const { scope, manager, name } of deletions) {
    await manager.delete(name);
    if (await manager.getMetadata(`${name}_EXPIRES_AT`)) {
      await manager.delete(`${name}_EXPIRES_AT`);
    }
    console.log(`Deleted ${scope}:${name} after verified OIDC publication.`);
  }
  session.outcome = {
    status: "migrated",
    deleted_secrets: deletions.map((item) => `${item.scope}:${item.name}`),
  };
}

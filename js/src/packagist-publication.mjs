import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { packagistRequest, services, repository } from "./packagist-setup.mjs";
import { repositorySlug } from "./repository-identity.mjs";
import { delay } from "./request-policy.mjs";

const execute = promisify(execFile);

/** Only causal publisher/audit jobs in a freshly read failed workflow are retried. */
export async function retryPackagistJobs(session) {
  const verified = (id) =>
    session.stepResults?.some(
      (entry) =>
        entry.step === id && ["verified", "configured"].includes(entry.status),
    );
  if (
    !verified("ensure-packagist-package") ||
    !verified("packagist-update-api") ||
    !verified("packagist-github-hook") ||
    session.options.noRetry ||
    !session.plan.package.workflow
  ) {
    return { status: "not-run" };
  }
  const github = services(session);
  const slug = repositorySlug(session.plan.repository);
  const repo = await github.rest.request(`/repos/${slug}`);
  const runs = await github.runs.list(slug, {
    workflow: session.plan.package.workflow,
    branch: repo.default_branch,
    limit: 20,
  });
  // A newer success or in-progress run supersedes an older failure.
  const run = [...runs].sort((a, b) => b.id - a.id)[0];
  if (!run || run.conclusion !== "failure") {
    return { status: "not-needed" };
  }
  const causal = [];
  for (const job of await github.runs.jobs(slug, run.id)) {
    if (job.conclusion !== "failure") {
      continue;
    }
    const log = await github.runs.jobLog(slug, job.id).catch(() => null);
    const affected =
      typeof log === "string" &&
      log
        .split("\n")
        .some(
          (line) =>
            (/PACKAGIST_(USERNAME|TOKEN)/.test(line) &&
              /missing|not set|empty|authentication|unauthorized|invalid/i.test(
                line,
              )) ||
            (/packagist/i.test(line) &&
              /authentication failed|unauthorized|not auto.updated|invalid (?:api )?token/i.test(
                line,
              )) ||
            (line.includes(session.plan.package.name) &&
              /could not find|not found|not indexed|no matching package/i.test(
                line,
              )),
        );
    if (affected) {
      causal.push(job.id);
    }
  }
  if (!causal.length) {
    return {
      status: "unknown",
      reason: "no affected failed publisher or audit jobs found",
    };
  }
  const result = await github.repair.verify({
    repo: slug,
    workflow: session.plan.package.workflow,
    runId: run.id,
    jobIds: causal,
    changes: [{ kind: "setting", changed: true, verified: true }],
    timeout: session.options.waitTimeoutMs ?? 600000,
  });
  return {
    status: result.status,
    jobs: causal,
    run: result.runUrl ?? run.html_url,
  };
}

/** Indexed version, immutable source, consumer and release are independent gates. */
export async function verifyPackagistPublication(session) {
  const item = session.plan.package;
  const evidence = {
    registered: "unknown",
    update: session.stepResults?.some((entry) => entry.update === "queued")
      ? "queued"
      : "unknown",
    indexed: "unknown",
    source: "unknown",
    consumer: "unknown",
    release: "unknown",
  };
  if (
    item.manifest !== "composer.json" ||
    !/^[a-z0-9][a-z0-9_.-]*\/[a-z0-9][a-z0-9_.-]*$/.test(item.name)
  ) {
    return {
      status: "unsupported",
      reason:
        "Packagist requires the canonical root Composer descriptor and vendor/package name",
      evidence,
    };
  }
  const wanted = item.version?.replace(/^v/, "");
  if (!wanted || /^dev-|-dev$|[-.](?:alpha|beta|rc|dev)/i.test(wanted)) {
    return {
      status: "unknown",
      reason: "exact stable version unresolved",
      evidence,
    };
  }
  const endpoint = `/packages/${item.name}.json`;
  let metadata;
  let entry;
  for (let attempt = 0; attempt < 3; attempt++) {
    metadata = await packagistRequest(session, endpoint);
    if (metadata.status !== 200) {
      break;
    }
    evidence.registered = "verified";
    const pkg = metadata.json?.package;
    if (
      pkg?.name !== item.name ||
      pkg.repository?.replace(/\.git$/, "").toLowerCase() !==
        repository(session).toLowerCase()
    ) {
      return { status: "repository-mismatch", evidence };
    }
    entry = Object.values(pkg.versions ?? {}).find(
      (version) => version.version?.replace(/^v/, "") === wanted,
    );
    if (entry) {
      break;
    }
    if (attempt < 2) {
      await (session.options.sleep ?? delay)(1000);
    }
  }
  if (metadata.status === 404) {
    return { status: "unregistered", evidence };
  }
  if ([401, 403].includes(metadata.status)) {
    return { status: "auth-rejected", evidence };
  }
  if (metadata.status !== 200 || !metadata.json?.package) {
    return { status: "transport-unknown", evidence };
  }
  if (!entry) {
    return {
      status: Object.values(metadata.json.package.versions ?? {}).some(
        (version) => /^dev-|-dev$/.test(version.version ?? ""),
      )
        ? "dev-only"
        : "stable-version-missing",
      reason: "registered but exact stable version is not indexed yet",
      evidence,
    };
  }
  evidence.indexed = "verified";
  const github = services(session);
  const slug = repositorySlug(session.plan.repository);
  const tag = entry.version;
  try {
    const ref = await github.rest.request(
      `/repos/${slug}/git/ref/tags/${encodeURIComponent(tag)}`,
      { allowNotFound: true },
    );
    let object = ref?.object;
    for (let depth = 0; object?.type === "tag" && depth < 5; depth++) {
      if (!/^[a-f0-9]{40}$/.test(object.sha ?? "")) {
        break;
      }
      const annotated = await github.rest.request(
        `/repos/${slug}/git/tags/${object.sha}`,
        { allowNotFound: true },
      );
      object = annotated?.object;
    }
    const sha = object?.type === "commit" ? object.sha : null;
    if (
      !/^[a-f0-9]{40}$/.test(sha ?? "") ||
      entry.source?.reference !== sha ||
      entry.source?.url?.replace(/\.git$/, "").toLowerCase() !==
        repository(session).toLowerCase()
    ) {
      return { status: "source-mismatch", version: wanted, evidence };
    }
    evidence.source = "verified";
    const release = await github.rest.request(
      `/repos/${slug}/releases/tags/${encodeURIComponent(tag)}`,
      { allowNotFound: true },
    );
    evidence.release =
      release && !release.draft && release.tag_name === tag
        ? "verified"
        : "missing";
    let temporary;
    try {
      temporary = await mkdtemp(path.join(tmpdir(), "prm-composer-"));
      await writeFile(
        path.join(temporary, "composer.json"),
        JSON.stringify({
          require: { [item.name]: wanted },
          config: { "allow-plugins": false },
        }),
      );
      await (session.options.execFile ?? execute)(
        "composer",
        [
          "install",
          "--no-interaction",
          "--no-plugins",
          "--no-scripts",
          "--no-progress",
          "--prefer-dist",
        ],
        { cwd: temporary, maxBuffer: 1024 * 1024 },
      );
      const lock = JSON.parse(
        await readFile(path.join(temporary, "composer.lock"), "utf8"),
      );
      const resolved = lock.packages?.find((pkg) => pkg.name === item.name);
      evidence.consumer =
        resolved?.version?.replace(/^v/, "") === wanted &&
        resolved?.source?.reference === sha
          ? "passed"
          : "source-mismatch";
    } catch {
      evidence.consumer = "failed";
    } finally {
      if (temporary) {
        await rm(temporary, { recursive: true, force: true });
      }
    }
    return {
      status:
        evidence.consumer === "passed" && evidence.release === "verified"
          ? "verified"
          : evidence.consumer !== "passed"
            ? "consumer-failed"
            : "release-missing",
      version: wanted,
      evidence,
    };
  } catch {
    return { status: "transport-unknown", evidence };
  }
}

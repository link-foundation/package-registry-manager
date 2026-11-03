import { githubServices } from "./github.mjs";
import { repositorySlug } from "./repository-identity.mjs";
import { parseSwiftManifest } from "./swift-distribution.mjs";

const CONSUMER_STEP = /consumer|resolves?\b|verify\b.*\b(?:swift|release)/i;

function services(session) {
  return session.options.github ?? githubServices(session.options);
}

async function fileAt(github, slug, file, ref) {
  const body = await github.rest.request(
    `/repos/${slug}/contents/${file.split("/").map(encodeURIComponent).join("/")}?ref=${encodeURIComponent(ref)}`,
    { allowNotFound: true },
  );
  if (!body || typeof body.content !== "string") {
    return null;
  }
  return Buffer.from(body.content, body.encoding ?? "base64").toString("utf8");
}

/**
 * Reads the public SwiftPM release of the exact version: the bare tag, the
 * commit it names (its version source must hold the version), the tagged
 * Package.swift and its library products, and a public GitHub release.
 * Each failure is a distinct state, and nothing is ever created or moved.
 */
export async function verifySwiftRelease(session, step) {
  const item = session.plan.package;
  const tag = step.tag ?? item.swift_package?.tag;
  const slug = repositorySlug(session.plan.repository);
  if (!tag || !slug) {
    return {
      status: "unknown",
      reason: !tag
        ? "the Swift version source is unresolved, so the expected tag is unknown"
        : "no GitHub repository identity",
    };
  }
  const github = services(session);
  const ref = await github.rest.request(
    `/repos/${slug}/git/ref/tags/${encodeURIComponent(tag)}`,
    { allowNotFound: true },
  );
  if (!ref) {
    return {
      status: "missing-tag",
      tag,
      reason: `the bare tag ${tag} does not exist; the release workflow creates it at the gated commit`,
    };
  }
  let commit = ref.object?.sha;
  if (ref.object?.type === "tag") {
    commit = (await github.rest.request(`/repos/${slug}/git/tags/${commit}`))
      .object?.sha;
  }
  if (step.commit && step.commit !== commit) {
    return {
      status: "wrong-commit",
      tag,
      commit,
      reason: `the tag names ${commit}, not ${step.commit}; existing tags are never moved`,
    };
  }
  if (item.version_source) {
    const declared = (await fileAt(github, slug, item.version_source, commit))
      ?.trim()
      .replace(/^v/, "");
    if (declared !== tag.replace(/^v/, "")) {
      return {
        status: "wrong-commit",
        tag,
        commit,
        reason: `${item.version_source} at the tagged commit declares ${declared ?? "nothing"}, not ${tag}; existing tags are never moved`,
      };
    }
  }
  const manifest = await fileAt(github, slug, item.manifest, commit);
  if (manifest === null) {
    return {
      status: "manifest-missing",
      tag,
      commit,
      reason: `${item.manifest} is absent at the tagged commit`,
    };
  }
  const parsed = parseSwiftManifest(manifest);
  const products = item.swift_package?.products ?? [];
  const absent = products.filter((name) => !parsed.libraries.includes(name));
  if (parsed.name !== item.name || absent.length) {
    return {
      status: "manifest-incorrect",
      tag,
      commit,
      reason: `the tagged Package.swift declares ${parsed.name ?? "no package"} with products ${parsed.libraries.join(", ") || "none"}; expected ${item.name} with ${products.join(", ")}`,
    };
  }
  const release = await github.rest.request(
    `/repos/${slug}/releases/tags/${encodeURIComponent(tag)}`,
    { allowNotFound: true },
  );
  if (!release || release.draft) {
    return {
      status: "release-missing",
      tag,
      commit,
      reason: release
        ? "the GitHub release is still a draft"
        : `no public GitHub release for ${tag}`,
    };
  }
  session.conditions.add("swift-release-verified");
  return { status: "verified", tag, commit, url: release.html_url };
}

/**
 * The exact-version consumer gate runs inside the release workflow: its
 * latest main run's consumer step is the only evidence. Skipped, failed,
 * missing or unreadable steps keep the release unverified.
 */
export async function verifySwiftConsumer(session, step) {
  const workflow = session.plan.package.workflow;
  const slug = repositorySlug(session.plan.repository);
  if (!session.conditions.has("swift-release-verified")) {
    return {
      status: "manual-prerequisite",
      reason:
        "the exact tag and release are unverified, so no consumer can resolve them",
    };
  }
  if (!workflow || !slug) {
    return {
      status: "manual-prerequisite",
      reason: step.description,
    };
  }
  const github = services(session);
  const repo = await github.rest.request(`/repos/${slug}`);
  const runs = await github.runs.list(slug, {
    workflow,
    branch: repo.default_branch,
    limit: 10,
  });
  const run = runs
    .filter((entry) => entry.status === "completed")
    .sort((a, b) => b.id - a.id)[0];
  if (!run) {
    return {
      status: "consumer-unknown",
      reason: `no completed ${workflow} run`,
    };
  }
  const callee = (session.plan.package.workflow_callees ?? [])
    .at(-1)
    ?.split(" -> ")
    .at(-1);
  const jobs = (await github.runs.jobs(slug, run.id)).filter(
    (job) => !callee || job.name === callee || job.name.endsWith(`/ ${callee}`),
  );
  const check = jobs
    .flatMap((job) => job.steps ?? [])
    .find((entry) => CONSUMER_STEP.test(entry.name));
  if (!check) {
    return {
      status: "consumer-unknown",
      run: run.html_url,
      reason: "the run has no readable consumer step",
    };
  }
  if (check.conclusion === "success") {
    return { status: "verified", run: run.html_url, step: check.name };
  }
  return {
    status:
      check.conclusion === "skipped" ? "consumer-skipped" : "consumer-failed",
    run: run.html_url,
    step: check.name,
    reason: `the consumer step ${check.conclusion ?? check.status}`,
  };
}

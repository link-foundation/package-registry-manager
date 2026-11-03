import { githubServices } from "./github.mjs";
import { AUTH_FAILURE_PATTERNS } from "./publishing-policy.mjs";
import { repositorySlug } from "./repository-identity.mjs";
import { verifySwiftConsumer, verifySwiftRelease } from "./swift-release.mjs";

/**
 * Generated Central, Packagist and SwiftPM steps and their handlers. None of
 * them is ever routed to the crates.io session.
 */
export const CENTRAL_PACKAGIST_STEPS = new Set([
  "ensure-central-credential-pair",
  "verify-namespace",
  "ensure-signing-key",
  "verify-central-publication",
  "verify-packagist-package",
  "packagist-github-hook",
  "packagist-update-api",
  "verify-swift-release",
  "verify-swift-consumer",
  "verify-gradle-toolchain",
]);

const MAVEN_REPOSITORY = "https://repo1.maven.org/maven2";
const MISSING_SECRET =
  /secrets? (?:are |is )?missing|missing (?:secrets?|credentials?)|(?:CENTRAL_USERNAME|CENTRAL_TOKEN|GPG_PRIVATE_KEY|GPG_PASSPHRASE)[^\n]*(?:not set|missing|empty)/i;

/** Runs one generated Central/Packagist/SwiftPM step. */
export async function runCentralPackagistStep(session, step) {
  const handlers = {
    "ensure-central-credential-pair": ensureCentralPair,
    "verify-namespace": verifyNamespace,
    "ensure-signing-key": ensureSigningKey,
    "verify-central-publication": verifyCentralPublication,
    "verify-packagist-package": verifyPackagistPackage,
    "packagist-github-hook": packagistHook,
    "packagist-update-api": packagistUpdateApi,
    "verify-swift-release": verifySwiftRelease,
    "verify-swift-consumer": verifySwiftConsumer,
    "verify-gradle-toolchain": manualPrerequisite,
  };
  const result = await handlers[step.id](session, step);
  record(session, step, result);
  return result;
}

/**
 * Collects the step result. Anything short of fresh evidence keeps the
 * setup incomplete; `stop` ends the plan before later browsers or commands.
 */
function record(session, step, result) {
  session.stepResults ??= [];
  session.stepResults.push({ step: step.id, ...result });
  if (
    !["verified", "present", "configured", "not-used"].includes(result.status)
  ) {
    session.incomplete ??= [];
    session.incomplete.push({ step: step.id, ...result });
  }
  if (result.stop) {
    session.outcome = {
      status: result.status,
      incomplete: session.incomplete ?? [],
    };
  }
  session.log(
    `${step.id}: ${result.status}${result.reason ? ` (${result.reason})` : ""}`,
  );
}

function services(session) {
  return session.options.github ?? githubServices(session.options);
}

/** Selected secret presence at repository, then organization scope. */
async function secretPresence(session, names) {
  const slug = repositorySlug(session.plan.repository);
  const github = services(session);
  const owner = slug.split("/")[0];
  const present = [];
  const missing = [];
  for (const name of names) {
    let metadata = await github.secrets({ repo: slug }).getMetadata(name);
    if (!metadata) {
      try {
        metadata = await github.secrets({ org: owner }).getMetadata(name);
      } catch (error) {
        if (![403, 404, 422].includes(error.status)) {
          throw error;
        }
      }
    }
    (metadata ? present : missing).push(name);
  }
  return { slug, present, missing };
}

/**
 * Keeps an existing Central user-token pair; otherwise acquires one new
 * pair in the dedicated browser, holds both fields in memory and stores
 * them as encrypted GitHub secrets through gh-manager.
 */
async function ensureCentralPair(session, step) {
  const names = (step.secrets ?? []).map((entry) => entry.secret);
  if (names.length !== 2 || !session.plan.repository?.github_owner) {
    return {
      status: "unsupported",
      reason:
        "the Central pair needs GitHub repository coordinates and exactly two selected secret names",
      stop: true,
    };
  }
  const { slug, present, missing } = await secretPresence(session, names);
  if (!missing.length) {
    return {
      status: "present",
      secrets: present,
      reason: "existing Central secrets kept; nothing rotated",
    };
  }
  const { options } = session;
  if (
    options.noBrowser ||
    options.browserOptions?.attach ||
    options.browserOptions?.importScope === "full"
  ) {
    return {
      status: "manual-prerequisite",
      secrets: missing,
      reason: `generate a Central Portal user token at ${step.url} and store ${missing.join(" + ")} as encrypted secrets; automation needs a dedicated profile with domain-scoped sign-in import`,
      stop: true,
    };
  }
  const replaces = present.length
    ? ` A Central token is a matched pair, so ${present.join(" + ")} is replaced too.`
    : "";
  if (
    !options.yes &&
    !/^y(?:es)?$/i.test(
      await session.prompt(
        `Generate one Central Portal user token and store ${names.join(" + ")} for ${slug}?${replaces} [y/N] `,
      ),
    )
  ) {
    return {
      status: "interrupted",
      secrets: missing,
      reason: "Central user-token creation declined",
      stop: true,
    };
  }
  const page = await session.automatedPage(
    {
      ...options.browserOptions,
      import: options.browserOptions?.import ?? {
        browser: "auto",
        profile: null,
      },
      importScope: "domains",
    },
    true,
  );
  await page.goto(step.url);
  await session.prompt(
    "Generate the user token in this browser and leave its username and password dialog open, then press Enter (never paste the values)...",
  );
  const pair = await page.evaluate(READ_CENTRAL_PAIR);
  if (!pair?.username || !pair?.password) {
    return {
      status: "manual-prerequisite",
      secrets: missing,
      reason:
        "the Central token dialog was not readable; nothing was stored, so the pair is still missing",
      stop: true,
    };
  }
  const github = services(session);
  const values = [pair.username, pair.password];
  // The pair is matched: both fields are written together after the
  // explicit confirmation above, never one silently.
  for (const [index, name] of names.entries()) {
    await github.secrets({ repo: slug }).set(name, values[index]);
  }
  const check = await secretPresence(session, names);
  if (check.missing.length) {
    return {
      status: "unverified",
      secrets: check.missing,
      reason: "gh-manager did not read the stored secrets back",
      stop: true,
    };
  }
  session.conditions.add("central-pair-stored");
  return {
    status: "configured",
    secrets: names,
    retry: await retryCausalJobs(session, slug, names),
  };
}

/**
 * Settings-only repair: restart only the failed main jobs whose logs show
 * the missing Central secrets, then follow them through gh-manager.
 */
async function retryCausalJobs(session, slug, names) {
  const workflow = session.plan.package.workflow;
  if (!workflow || session.options.noRetry) {
    return { status: "not-run", reason: "no publishing workflow to retry" };
  }
  const github = services(session);
  const repo = await github.rest.request(`/repos/${slug}`);
  const runs = await github.runs.list(slug, {
    workflow,
    branch: repo.default_branch,
    limit: 20,
  });
  const failed = runs
    .filter((run) => run.conclusion === "failure")
    .sort((a, b) => b.id - a.id)[0];
  if (!failed) {
    return { status: "not-needed", reason: "no failed main run" };
  }
  const auth = new RegExp(
    [MISSING_SECRET.source, ...(AUTH_FAILURE_PATTERNS["maven-central"] ?? [])]
      .filter(Boolean)
      .join("|"),
    "i",
  );
  const causal = [];
  for (const job of await github.runs.jobs(slug, failed.id)) {
    if (job.conclusion !== "failure") {
      continue;
    }
    const log = await github.runs.jobLog(slug, job.id).catch(() => null);
    if (
      typeof log === "string" &&
      auth.test(log) &&
      names.some((name) => log.includes(name))
    ) {
      causal.push(job.id);
    }
  }
  if (!causal.length) {
    return {
      status: "unknown",
      reason:
        "no failed job log names the stored Central secrets; unrelated CI was not restarted",
      run: failed.html_url,
    };
  }
  const result = await github.repair.verify({
    repo: slug,
    workflow,
    runId: failed.id,
    jobIds: causal,
    changes: [{ kind: "setting", changed: true, verified: true }],
    timeout: session.options.waitTimeoutMs ?? 600000,
  });
  return {
    status: result.status,
    run: result.runUrl ?? failed.html_url,
    jobs: causal,
  };
}

/** Reads both displayed fields of the Central token dialog; never logged. */
export const READ_CENTRAL_PAIR = `(() => {
  const root = document.querySelector('[role="dialog"]') ?? document.body;
  const text = (element) => (element?.value ?? element?.textContent ?? "").trim();
  const field = (label) => {
    for (const node of root.querySelectorAll("label, dt, th, span, p, div")) {
      if (node.children.length === 0 && new RegExp("^" + label + ":?$", "i").test(text(node))) {
        const target = node.htmlFor ? root.querySelector("#" + CSS.escape(node.htmlFor)) : null;
        const next = target ?? node.nextElementSibling ?? node.parentElement?.nextElementSibling;
        const value = text(next?.querySelector?.("input, code, pre") ?? next);
        if (value && !/\\s/.test(value)) return value;
      }
    }
    return null;
  };
  return { username: field("username"), password: field("password") };
})()`;

async function http(session, url, method = "GET") {
  const fetcher = session.options.fetch ?? globalThis.fetch;
  try {
    const response = await fetcher(url, {
      method,
      headers: { accept: "application/json" },
      signal: AbortSignal.timeout(session.options.timeoutMs ?? 10_000),
    });
    if (session.options.verbose) {
      console.error(`${method} ${url} -> ${response.status}`);
    }
    return {
      status: response.status,
      json:
        method === "GET" && response.ok
          ? await response.json().catch(() => null)
          : null,
    };
  } catch (error) {
    return { status: "transport", reason: error.message };
  }
}

/** Published artifacts under the namespace prove it was verified; nothing else does. */
async function verifyNamespace(session, step) {
  const namespace = step.namespace;
  if (!namespace) {
    return {
      status: "manual-prerequisite",
      reason: "the package coordinates name no namespace",
    };
  }
  const response = await http(
    session,
    `https://search.maven.org/solrsearch/select?q=${encodeURIComponent(`g:"${namespace}"`)}&rows=1&wt=json`,
  );
  if (response.json?.response?.numFound > 0) {
    return {
      status: "verified",
      namespace,
      evidence: "artifacts are published under the namespace",
    };
  }
  return {
    status: "manual-prerequisite",
    namespace,
    reason: `no fresh read-back proves ${namespace} is verified; confirm it at ${step.url}`,
  };
}

function mavenPath(group, artifact, version, extension) {
  return `${MAVEN_REPOSITORY}/${group.replaceAll(".", "/")}/${artifact}/${version}/${artifact}-${version}.${extension}`;
}

/** Signing works only when a published signature exists; presence is not proof. */
async function ensureSigningKey(session, step) {
  const names = (step.secrets ?? []).map((entry) => entry.secret);
  const { present, missing } = await secretPresence(session, names);
  const [group, artifact] = (session.plan.package.name ?? "").split(":");
  const version = session.plan.package.version;
  if (group && artifact && version) {
    const signature = await http(
      session,
      mavenPath(group, artifact, version, "pom.asc"),
      "HEAD",
    );
    if (signature.status === 200) {
      return {
        status: "verified",
        secrets: present,
        evidence: "the exact release has a published signature",
      };
    }
  }
  return {
    status: "manual-prerequisite",
    secrets: present,
    missing,
    reason: missing.length
      ? `${missing.join(" + ")} missing; provide an armored key whose public key is on a keyserver`
      : "signing secrets are present, but no signed release proves the key is valid and published",
  };
}

/** Root POM, Gradle module metadata and every referenced platform coordinate. */
async function verifyCentralPublication(session, step) {
  const { group, artifact, version } = step.coordinates ?? {};
  if (!group || !artifact || !version) {
    return {
      status: "unknown",
      reason: "the exact coordinates are unresolved",
    };
  }
  const root = await http(
    session,
    mavenPath(group, artifact, version, "pom"),
    "HEAD",
  );
  if (root.status !== 200) {
    return {
      status: root.status === 404 ? "unpublished" : "transport-unknown",
      reason: `root POM HTTP ${root.status}`,
    };
  }
  const missing = [];
  const multiplatform = session.plan.package.gradle?.multiplatform;
  let referenced = [];
  if (multiplatform) {
    const module = await http(
      session,
      mavenPath(group, artifact, version, "module"),
    );
    if (!module.json) {
      return {
        status: "partial",
        reason: `Gradle module metadata missing (HTTP ${module.status}); the root POM alone certifies no variant`,
      };
    }
    referenced = (module.json.variants ?? [])
      .map((variant) => variant["available-at"]?.module)
      .filter(Boolean)
      .map((entry) => `${entry.group}:${entry.module}`);
  }
  for (const coordinates of new Set([
    ...(step.variants ?? []),
    ...referenced,
  ])) {
    const [variantGroup, variantArtifact] = coordinates.split(":");
    if (multiplatform && !referenced.includes(coordinates)) {
      missing.push(`${coordinates} (not in module metadata)`);
      continue;
    }
    const pom = await http(
      session,
      mavenPath(variantGroup, variantArtifact, version, "pom"),
      "HEAD",
    );
    if (pom.status !== 200) {
      missing.push(`${coordinates} (HTTP ${pom.status})`);
    }
  }
  return missing.length
    ? {
        status: "partial",
        missing,
        reason: "platform publications are missing",
      }
    : { status: "verified", variants: referenced };
}

const STABLE = (version) =>
  !/^dev-|-dev$|[-.](?:alpha|beta|rc|dev)/i.test(version);

/**
 * Exact stable tag read-back from Packagist's p2 metadata. Package
 * existence or a dev branch never passes.
 */
async function verifyPackagistPackage(session) {
  const item = session.plan.package;
  if (item.manifest !== "composer.json") {
    return {
      status: "unsupported",
      reason: `${item.manifest} is not the canonical root Composer descriptor Packagist reads`,
    };
  }
  const tagged = await http(
    session,
    `https://repo.packagist.org/p2/${item.name}.json`,
  );
  if (tagged.status === 404) {
    return {
      status: "unregistered",
      reason: `${item.name} is not on Packagist`,
    };
  }
  if ([401, 403].includes(tagged.status)) {
    return { status: "auth-rejected", reason: `HTTP ${tagged.status}` };
  }
  if (tagged.status !== 200 || !tagged.json) {
    return {
      status: "transport-unknown",
      reason: tagged.reason ?? `HTTP ${tagged.status}`,
    };
  }
  const versions = (tagged.json.packages?.[item.name] ?? [])
    .map((entry) => entry.version)
    .filter((version) => version && STABLE(version));
  if (!versions.length) {
    const dev = await http(
      session,
      `https://repo.packagist.org/p2/${item.name}~dev.json`,
    );
    return {
      status: dev.json?.packages?.[item.name]?.length
        ? "dev-only"
        : "stable-version-missing",
      reason: "no stable tagged version",
    };
  }
  const wanted = item.version?.replace(/^v/, "");
  if (!wanted) {
    return {
      status: "unknown",
      reason: "the expected version is unresolved",
      versions,
    };
  }
  return versions.some((version) => version.replace(/^v/, "") === wanted)
    ? { status: "verified", version: wanted }
    : {
        status: "stable-version-missing",
        reason: `${wanted} is not published; latest ${versions[0]}`,
      };
}

/** The GitHub hook is invisible to this tool: stays a prerequisite. */
async function packagistHook(session, step) {
  const exact = session.stepResults?.find(
    (entry) => entry.step === "verify-packagist-package",
  );
  return {
    status: "manual-prerequisite",
    reason: `confirm the GitHub integration at ${step.url}${exact?.status === "verified" ? "; the exact tag reached Packagist, but that does not prove the hook" : ""}`,
  };
}

async function packagistUpdateApi(session, step) {
  if (step.state !== "configured") {
    return {
      status: "not-used",
      reason:
        "the workflow does not call the update-package API; the GitHub hook is the only update path",
    };
  }
  const names = (step.secrets ?? []).map((entry) => entry.secret);
  const { present, missing } = await secretPresence(session, names);
  return missing.length
    ? {
        status: "manual-prerequisite",
        missing,
        reason: `${missing.join(" + ")} missing; add the Packagist API token as encrypted secrets`,
      }
    : { status: "configured", secrets: present };
}

/** Pressing Enter is never verification. */
async function manualPrerequisite(session, step) {
  console.log(step.description);
  if (step.url) {
    console.log(`  ${step.url}`);
  }
  return {
    status: "manual-prerequisite",
    reason: "no automated read-back evidence",
  };
}

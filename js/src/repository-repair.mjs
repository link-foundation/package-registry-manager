import { pageFetchScript } from "./crates-api.mjs";
import {
  publisherIdentities,
  publisherMatches,
  repositorySlug,
} from "./repository-identity.mjs";
import { offerManifestRepository } from "./manifest-proposal.mjs";

/** Insert transfer repairs before any release retry or secret cleanup. */
export function repairSteps(packageInfo, context, steps) {
  const ids = new Set(["verify-trusted-publisher", "rerun-release"]);
  const result = steps.filter((step) => !ids.has(step.id));
  if (
    packageInfo.registry !== "npm" &&
    !context.manual &&
    packageInfo.exists_on_registry === true
  ) {
    const before = result.findIndex((step) =>
      ["configure-trusted-publisher", "attach-trusted-publisher"].includes(
        step.id,
      ),
    );
    result.splice(Math.max(0, before), 0, {
      id: "check-repository-publisher",
      title: "Read the current repository's configured publisher",
      kind: "check",
      description:
        "Check the repository, workflow and environment in authenticated settings before attaching a replacement.",
    });
  }
  const repairs = [
    {
      id: "verify-repository-publisher",
      title: "Verify the new repository's trusted publisher",
      kind: "check",
      description:
        "Read registry settings and require the new repository, workflow and environment before removing old trust.",
    },
    {
      id: "remove-old-publisher",
      title: "Remove the old repository's trusted publisher",
      kind: "api",
      description:
        "Remove only publishers naming the old repository, after verifying the replacement; confirm their removal in registry settings.",
      confirm: true,
    },
  ];
  if (
    packageInfo.repository_mismatches?.some(
      (finding) => finding.source === "manifest",
    )
  ) {
    repairs.push({
      id: "fix-manifest-repository",
      title: "Offer the manifest repository URL fix in a reviewed pull request",
      kind: "api",
      description:
        "Create a branch and draft pull request for the repository URL. Pause release retries until it is merged.",
      confirm: true,
    });
  }
  repairs.push({
    id: "rerun-release",
    title: "Re-run failed release jobs after repository repairs",
    kind: "check",
    description:
      "Verify the remote default-branch manifests name the current repository before retrying failed release jobs.",
  });
  if (packageInfo.exists_on_registry !== true) {
    const manifestFix = repairs.find(
      (step) => step.id === "fix-manifest-repository",
    );
    if (manifestFix) {
      result.unshift(manifestFix);
      repairs.splice(repairs.indexOf(manifestFix), 1);
    }
  }
  const insertion = result.findLastIndex((step) =>
    ["configure-trusted-publisher", "attach-trusted-publisher"].includes(
      step.id,
    ),
  );
  result.splice(insertion + 1, 0, ...repairs);
  return result;
}

async function npmPublishers(session) {
  const step = session.plan.steps.find((item) => item.id === "check-trust");
  const result = await session.runProcess(step, false);
  if (result.code !== 0) {
    throw new Error(
      "could not read npm trusted publishers; repository repair stopped",
    );
  }
  return publisherIdentities(result.stdout);
}

async function cratesPublishers(session) {
  const page = await session.automatedPage();
  const publishers = [];
  let query = `?crate=${encodeURIComponent(session.plan.package.name)}`;
  const seen = new Set();
  while (query) {
    if (!query.startsWith("?") || seen.has(query)) {
      throw new Error("invalid crates.io publisher pagination");
    }
    seen.add(query);
    const response = await page.evaluate(
      pageFetchScript(
        "GET",
        `/api/v1/trusted_publishing/github_configs${query}`,
      ),
    );
    if (
      response.status !== 200 ||
      !Array.isArray(response.body?.github_configs)
    ) {
      throw new Error("could not read crates.io trusted publishers");
    }
    publishers.push(...publisherIdentities(response.body));
    query = response.body.meta?.next_page;
  }
  return publishers;
}

/** PyPI exposes configured publishers only in the authenticated settings page. */
export const PYPI_PUBLISHERS_SCRIPT = `(() => [...document.querySelectorAll(".table--publisher-list tbody tr")].flatMap(row => {
  const details = row.querySelector("small");
  const link = details?.querySelector('a[href^="https://github.com/"]');
  const remove = row.querySelector('a[href^="#remove-publisher-"]');
  if (!link || !remove) return [];
  const field = label => {
    const heading = [...details.querySelectorAll("b")].find(item => item.textContent.trim() === label);
    if (!heading) return undefined;
    let value = "";
    for (let node = heading.nextSibling; node && node.nodeName !== "BR"; node = node.nextSibling) {
      if (node.nodeName === "I") return null;
      value += node.textContent;
    }
    return value.trim() || null;
  };
  const workflow = field("Workflow:");
  const environment = field("Environment name:");
  if (!workflow || environment === undefined) return [];
  return [{ id: remove.getAttribute("href").slice(18), repository: link.href, workflow, environment }];
}))()`;

/** Submit the site's existing removal form, retaining its hidden CSRF fields. */
export function pypiRemovalScript(id) {
  return `(async () => {
    const input = [...document.querySelectorAll('input[name="publisher_id"]')].find(input => input.value === ${JSON.stringify(id)});
    const form = input?.closest("form");
    if (!form || form.method.toLowerCase() !== "post" || form.action !== location.href.split("?")[0]) throw new Error("PyPI publisher removal form unavailable");
    const response = await fetch(form.action, { method: "POST", body: new FormData(form), credentials: "same-origin" });
    return { status: response.status };
  })()`;
}

async function pypiPublishers(session) {
  const url = `https://pypi.org/manage/project/${encodeURIComponent(session.plan.package.name)}/settings/publishing/`;
  await session.open(url, true);
  await session.prompt(
    "Sign in and show the project's trusted publishers, then press Enter...",
  );
  const rows = await session.automation?.evaluate(PYPI_PUBLISHERS_SCRIPT);
  if (!Array.isArray(rows)) {
    throw new Error("could not read PyPI trusted publishers");
  }
  return publisherIdentities(rows);
}

async function configuredPublishers(session) {
  switch (session.plan.registry) {
    case "npm":
      return npmPublishers(session);
    case "crates-io":
      return cratesPublishers(session);
    default:
      return pypiPublishers(session);
  }
}

async function verifyPublisher(session) {
  const publishers = await configuredPublishers(session);
  if (
    !publishers.some((publisher) =>
      publisherMatches(publisher, session.plan.trusted_publisher),
    )
  ) {
    throw new Error(
      "registry settings do not list the new repository's workflow and environment; old publishers were kept",
    );
  }
  session.repositoryPublishers = publishers;
  session.conditions.add("repository-publisher-verified");
  session.conditions.delete("trust-missing");
  return publishers;
}

async function removeOldPublishers(session) {
  if (!session.conditions.has("repository-publisher-verified")) {
    throw new Error(
      "verify the replacement publisher before removing old trust",
    );
  }
  const publishers = await verifyPublisher(session);
  const current = repositorySlug(session.plan.repository).toLowerCase();
  const old = new Set(
    publishers
      .filter((publisher) => publisher.repository.toLowerCase() !== current)
      .map((publisher) => publisher.repository.toLowerCase()),
  );
  for (const publisher of publishers.filter((item) =>
    old.has(item.repository.toLowerCase()),
  )) {
    if (!publisher.id || !/^[\w-]+$/.test(publisher.id)) {
      throw new Error("the old publisher has no usable registry identifier");
    }
    if (
      !session.options.yes &&
      !/^y(?:es)?$/i.test(
        await session.prompt(
          `Remove trusted publisher ${publisher.repository} (${publisher.workflow})? [y/N] `,
        ),
      )
    ) {
      throw new Error("old publisher removal declined; release retry stopped");
    }
    if (session.plan.registry === "npm") {
      const command = session.plan.steps.find(
        (step) => step.id === "check-trust",
      ).command;
      const args = [...command.args];
      args[args.indexOf("list")] = "revoke";
      args.push("--id", publisher.id, "--yes");
      const result = await session.runProcess(
        { command: { program: command.program, args } },
        true,
      );
      if (result.code !== 0) {
        throw new Error("npm did not revoke the old publisher");
      }
    } else if (session.plan.registry === "crates-io") {
      const page = await session.automatedPage();
      const result = await page.evaluate(
        pageFetchScript(
          "DELETE",
          `/api/v1/trusted_publishing/github_configs/${publisher.id}`,
        ),
      );
      if (result.status < 200 || result.status >= 300) {
        throw new Error("crates.io did not remove the old publisher");
      }
    } else {
      const result = await session.automation.evaluate(
        pypiRemovalScript(publisher.id),
      );
      if (result.status < 200 || result.status >= 300) {
        throw new Error("PyPI did not remove the old publisher");
      }
    }
  }
  if (
    (await verifyPublisher(session)).some((publisher) =>
      old.has(publisher.repository.toLowerCase()),
    )
  ) {
    throw new Error(
      "the old repository still has a trusted publisher; release retry stopped",
    );
  }
}

/** Execute repairs; a manifest PR pauses setup for review and merge. */
export async function runRepositoryRepair(session, step) {
  if (step.id === "check-repository-publisher") {
    const matches = (await configuredPublishers(session)).some((publisher) =>
      publisherMatches(publisher, session.plan.trusted_publisher),
    );
    if (matches) {
      session.conditions.delete("trust-missing");
    } else {
      session.conditions.add("trust-missing");
    }
    return;
  }
  if (step.id === "verify-repository-publisher") {
    return verifyPublisher(session);
  }
  if (step.id === "remove-old-publisher") {
    return removeOldPublishers(session);
  }
  if (step.id === "fix-manifest-repository") {
    const result = await offerManifestRepository(session.plan, {
      ...session.options,
      prompt: (message) => session.prompt(message),
    });
    if (result) {
      session.outcome = result;
    }
    return;
  }
  // A failed historical run checks out its original SHA. Require its manifests
  // to match, or dispatch a fresh run from the corrected default branch.
  if (!session.conditions.has("repository-publisher-verified")) {
    throw new Error("verify repository repairs before retrying release jobs");
  }
  const expected = repositorySlug(session.plan.repository);
  const result = await session.capture({
    command: {
      program: "gh",
      args: [
        "run",
        "list",
        "--repo",
        expected,
        "--workflow",
        session.plan.trusted_publisher.workflow,
        "--limit",
        "1",
        "--json",
        "databaseId,conclusion,headSha",
      ],
    },
    cwd: ".",
  });
  if (result.code !== 0) {
    throw new Error("could not inspect release jobs after repository repair");
  }
  const [run] = JSON.parse(result.stdout || "[]");
  if (run?.conclusion !== "failure") {
    return;
  }
  const { verifyRemoteManifest } = await import("./manifest-proposal.mjs");
  const corrected =
    Boolean(run.headSha) &&
    (await verifyRemoteManifest(session.plan, session.options, run.headSha));
  if (
    !corrected &&
    !(await verifyRemoteManifest(session.plan, session.options))
  ) {
    throw new Error(
      "merge the manifest repository URL fix before retrying release jobs",
    );
  }
  const command = corrected
    ? {
        program: "gh",
        args: [
          "run",
          "rerun",
          String(run.databaseId),
          "--repo",
          expected,
          "--failed",
        ],
      }
    : {
        program: "gh",
        args: [
          "workflow",
          "run",
          session.plan.trusted_publisher.workflow,
          "--repo",
          expected,
        ],
      };
  await session.command({
    ...step,
    kind: "command",
    command,
    cwd: ".",
    confirm: true,
  });
}

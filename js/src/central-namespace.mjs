import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import path from "node:path";
import centralPage from "./central-page.mjs";
import { githubServices } from "./github.mjs";
import { repositorySlug } from "./repository-identity.mjs";

/** Namespace ownership comes from the authenticated Portal, never public search. */
export async function verifyNamespace(session, step) {
  const namespace = step.namespace;
  if (!namespace) {
    return {
      status: "manual-prerequisite",
      reason: "the package coordinates name no namespace",
    };
  }
  if (
    session.options.noBrowser ||
    session.options.browserOptions?.attach ||
    session.options.browserOptions?.importScope === "full"
  ) {
    return {
      status: "manual-prerequisite",
      namespace,
      reason:
        "authenticated Central ownership read-back requires the dedicated browser",
    };
  }
  const config = JSON.parse(
    await readFile(
      path.join(
        session.options.repository ?? ".",
        ".package-registry-manager.json",
      ),
      "utf8",
    ).catch((e) => {
      if (e.code === "ENOENT") {
        return "{}";
      }
      throw e;
    }),
  );
  const expected = {
    namespace,
    account: config.central?.account,
    organization: config.central?.organization,
  };
  const page = await session.automatedPage(
    { ...session.options.browserOptions, importScope: "domains" },
    true,
  );
  await page.goto("https://central.sonatype.com/publishing/namespaces");
  const inspect = async (action = "read") =>
    page.evaluate(
      `(${centralPage})(${JSON.stringify(expected)},${JSON.stringify(action)})`,
    );
  let state = await inspect();
  const directory = path.join(
    session.options.repository ?? ".",
    ".package-registry-manager",
  );
  const obligation = path.join(directory, "central-namespace-proof.json");
  const save = async (value) => {
    await mkdir(directory, { recursive: true, mode: 0o700 });
    const temporary = `${obligation}.tmp`;
    await writeFile(temporary, JSON.stringify(value), { mode: 0o600 });
    await rename(temporary, obligation);
  };
  const pending = JSON.parse(
    await readFile(obligation, "utf8").catch((e) => {
      if (e.code === "ENOENT") {
        return "null";
      }
      throw e;
    }),
  );
  const github = session.options.github ?? githubServices(session.options);
  const cleanup = async (proof) => {
    if (
      !proof ||
      proof.namespace !== namespace ||
      proof.account !== state.account
    ) {
      return;
    }
    try {
      const repo = await github.rest.request(`/repos/${proof.slug}`, {
        allowNotFound: true,
      });
      if (!repo) {
        await save(null);
        state.cleanup = { status: "complete", repository: proof.slug };
        return;
      }
      if (
        repo.id !== proof.id ||
        repo.owner.id !== proof.owner_id ||
        repo.size !== 0 ||
        repo.default_branch !== proof.default_branch
      ) {
        throw new Error("proof repository changed");
      }
      const branches = await github.rest.request(
        `/repos/${proof.slug}/branches`,
      );
      if (!Array.isArray(branches) || branches.length) {
        throw new Error("proof repository is no longer empty");
      }
      await github.rest.request(`/repos/${proof.slug}`, { method: "DELETE" });
      const gone = await github.rest.request(`/repos/${proof.slug}`, {
        allowNotFound: true,
      });
      if (gone) {
        throw new Error("proof repository deletion unverified");
      }
      await save(null);
      state.cleanup = { status: "complete", repository: proof.slug };
    } catch {
      state.cleanup = {
        status: "pending",
        repository: proof.slug,
        reason:
          "delete the recorded empty proof repository through an authorized GitHub UI or existing permissions; no token scopes were expanded",
      };
    }
  };
  if (state.status === "verified") {
    await cleanup(pending);
    return state;
  }
  if (!["missing", "unverified"].includes(state.status)) {
    return state;
  }
  expected.account = state.account;
  if (
    !session.options.yes &&
    !/^y(?:es)?$/i.test(
      await session.prompt(
        `Register/verify ${namespace} for the signed-in Central account and create its temporary public GitHub proof repository? [y/N] `,
      ),
    )
  ) {
    return {
      status: "interrupted",
      namespace,
      reason: "namespace setup declined",
    };
  }
  if (state.status === "missing") {
    state = await inspect("register");
  }
  const owner = /^io\.github\.([\w-]+)$/.exec(namespace)?.[1];
  const slug = repositorySlug(session.plan.repository);
  const key = state.verification_key;
  if (state.status === "verified") {
    return state;
  }
  if (
    !owner ||
    !slug ||
    owner.toLowerCase() !== slug.split("/")[0].toLowerCase() ||
    !/^[\w-]+$/.test(key ?? "")
  ) {
    return {
      ...state,
      status: "manual-prerequisite",
      reason:
        "the exact namespace GitHub owner or proof challenge is unresolved; complete verification in the official portal",
    };
  }
  const repository = await github.rest.request(`/repos/${slug}`);
  if (
    !Number.isSafeInteger(repository.owner?.id) ||
    repository.owner.id <= 0 ||
    repository.owner.login?.toLowerCase() !== owner.toLowerCase()
  ) {
    return {
      status: "wrong-identity",
      reason: "immutable GitHub namespace owner is unverified",
    };
  }
  const proofSlug = `${repository.owner.login}/${key}`;
  let proof =
    pending?.slug === proofSlug &&
    pending.account === state.account &&
    pending.namespace === namespace
      ? pending
      : null;
  const existing = await github.rest.request(`/repos/${proofSlug}`, {
    allowNotFound: true,
  });
  if (existing && (!proof || proof.id !== existing.id)) {
    return {
      ...state,
      status: "manual-prerequisite",
      reason:
        "proof repository already exists without a matching cleanup obligation; it was not changed",
    };
  }
  if (!existing) {
    // Write the intent first, so an interruption never loses the cleanup obligation.
    await mkdir(directory, { recursive: true, mode: 0o700 });
    proof = {
      namespace,
      account: expected.account,
      slug: proofSlug,
      owner_id: repository.owner.id,
      status: "creation-pending",
    };
    await save(proof);
    const created = await github.rest.request(
      repository.owner.type === "Organization"
        ? `/orgs/${repository.owner.login}/repos`
        : "/user/repos",
      {
        method: "POST",
        body: {
          name: key,
          private: false,
          auto_init: false,
          description: "Temporary Maven Central namespace ownership proof",
        },
      },
    );
    proof = {
      ...proof,
      id: created.id,
      default_branch: created.default_branch,
      status: "cleanup-pending",
    };
    await save(proof);
    if (
      created.owner.id !== repository.owner.id ||
      created.private ||
      created.full_name.toLowerCase() !== proofSlug.toLowerCase()
    ) {
      return { status: "wrong-identity", cleanup: proof };
    }
  }
  state = await inspect("verify");
  const deadline = Date.now() + (session.options.waitTimeoutMs ?? 600000);
  while (
    state.status === "unverified" &&
    state.state === "VERIFYING" &&
    Date.now() < deadline
  ) {
    await new Promise((resolve) =>
      setTimeout(resolve, session.options.pollIntervalMs ?? 1000),
    );
    await page.goto("https://central.sonatype.com/publishing/namespaces");
    state = await inspect();
  }
  if (state.status === "verified") {
    await cleanup(proof);
  } else {
    state.cleanup = { status: "pending", repository: proofSlug };
  }
  return state;
}

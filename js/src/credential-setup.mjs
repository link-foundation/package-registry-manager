import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { githubServices } from "./github.mjs";
import { cycleCredential, secretName } from "./ci-credential-cycle.mjs";
import { AUTH_FAILURE_PATTERNS } from "./publishing-policy.mjs";
import {
  TOKEN_PROVIDERS,
  tokenFormScript,
  READ_TOKEN,
  revokedScript,
} from "./credential-browser.mjs";

/** Provision only when GitHub CI reports a missing or broken credential. */
export async function setupCredential(session) {
  const { plan, options } = session;
  const provider = TOKEN_PROVIDERS[plan.registry];
  if (!provider) {
    throw new Error(`no token provider for ${plan.registry}`);
  }
  const contents = await readFile(
    path.join(options.repository, ".package-registry-manager.json"),
    "utf8",
  ).catch((error) => {
    if (error.code === "ENOENT") {
      return "{}";
    }
    throw error;
  });
  const settings = JSON.parse(contents).tokens?.[plan.registry] ?? {};
  const { github_owner: owner, github_repository: repository } =
    plan.repository;
  if (!owner || !repository) {
    throw new Error("token setup requires GitHub repository coordinates");
  }
  const slug = `${owner}/${repository}`;
  const secret = secretName(
    options.secretName ??
      settings.secret ??
      plan.package.token_secrets?.[0] ??
      provider.secret,
    plan.registry,
    slug,
  );
  if (settings.level && !["repo", "org"].includes(settings.level)) {
    throw new Error("token secret level must be repo or org");
  }
  const github = options.github ?? githubServices(options);
  const scope = { repo: slug };
  const healthOptions = {
    scope,
    failurePatterns: AUTH_FAILURE_PATTERNS[plan.registry] ?? [],
    inputs: settings.verification_workflow
      ? { prm_nonce: `prm-${Date.now()}` }
      : {},
  };
  const stateFile =
    options.browserProfile &&
    path.join(path.dirname(options.browserProfile), "credential-ids.json");
  const key = `${slug}:${secret}`;
  let state = {};
  let page;
  let approved = false;
  let repositorySecretPresent = false;
  const result = await cycleCredential({
    health: () => github.health.health(secret, healthOptions),
    metadata: async () => {
      let metadata = await github.secrets(scope).getMetadata(secret);
      repositorySecretPresent = Boolean(metadata);
      if (!metadata && settings.level !== "repo") {
        try {
          metadata = await github.secrets({ org: owner }).getMetadata(secret);
        } catch (error) {
          if (![403, 404, 422].includes(error.status)) {
            throw error;
          }
        }
      }
      if (stateFile) {
        state = JSON.parse(
          await readFile(stateFile, "utf8").catch((error) => {
            if (error.code === "ENOENT") {
              return "{}";
            }
            throw error;
          }),
        );
      }
      return (
        metadata && {
          ...metadata,
          token_id:
            settings.token_id ?? state[key]?.at(-1) ?? metadata.token_id,
          token_ids: state[key] ?? [],
        }
      );
    },
    create: async () => {
      if (
        options.noBrowser ||
        options.browserOptions?.attach ||
        options.browserOptions?.importScope === "full"
      ) {
        throw new Error(
          "token setup requires a dedicated profile with domain-scoped sign-in import",
        );
      }
      const days = settings.expiry_days ?? 30;
      if (!Number.isInteger(days) || days < 1 || days > 90) {
        throw new Error("token expiry_days must be between 1 and 90");
      }
      if (
        !approved &&
        !options.yes &&
        !/^y(?:es)?$/i.test(
          await session.prompt(
            "Create and store a scoped credential, verify it, then revoke the replaced token? [y/N] ",
          ),
        )
      ) {
        throw new Error("credential creation declined");
      }
      approved = true;
      page ??= await session.automatedPage(
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
      await page.goto(provider.url);
      await page.evaluate(
        tokenFormScript(
          provider,
          `prm-${repository}-${secret}`,
          new Date(Date.now() + days * 86_400_000).toISOString(),
        ),
      );
      await session.prompt(
        `Review the publishing scope (${provider.scope}) and expiry, create the token in this browser, then press Enter (never paste its value)...`,
      );
      return page.evaluate(READ_TOKEN);
    },
    ensure: async (credential, health) => {
      const repos =
        settings.level === "repo"
          ? [slug]
          : (options.secretRepositories?.[`${plan.registry}:${secret}`] ?? [
              slug,
            ]);
      let stored = await github
        .secrets(settings.level === "repo" ? scope : { org: owner })
        .ensure(secret, {
          ...(settings.level === "repo"
            ? {}
            : { repos, visibility: "selected" }),
          health,
          rotateBeforeMs: 0,
          failurePatterns: healthOptions.failurePatterns,
          acquire: async () => ({
            value: credential.value,
            expiresAt: credential.expires_at,
          }),
        });
      if (stored.path === "organization") {
        for (const repo of repos) {
          const override =
            repo === slug
              ? repositorySecretPresent
              : await github.secrets({ repo }).getMetadata(secret);
          if (!override) {
            continue;
          }
          const replacement = await github.secrets({ repo }).ensure(secret, {
            health,
            rotateBeforeMs: 0,
            acquire: async () => ({
              value: credential.value,
              expiresAt: credential.expires_at,
            }),
          });
          if (replacement.valueChanged === false) {
            throw new Error(
              "gh-manager did not replace the repository credential override",
            );
          }
          if (repo === slug) {
            stored = {
              ...replacement,
              path: "repository",
              fallbackReason:
                "An existing repository secret overrides the organization secret; replaced the repository credential too.",
            };
          }
        }
      }
      if (stored.valueChanged === false) {
        throw new Error("gh-manager did not store the replacement credential");
      }
      if (stateFile) {
        state[key] = [...new Set([...(state[key] ?? []), credential.id])];
        await mkdir(path.dirname(stateFile), { recursive: true });
        await writeFile(stateFile, JSON.stringify(state), { mode: 0o600 });
      }
      return stored;
    },
    test: async (stored) => {
      const repos =
        stored && settings.level !== "repo"
          ? (options.secretRepositories?.[`${plan.registry}:${secret}`] ?? [
              slug,
            ])
          : [slug];
      const results = await Promise.all(
        repos.map((repo) =>
          github.health.test(secret, { ...healthOptions, scope: { repo } }),
        ),
      );
      return {
        status: results.some((item) => item.status === "auth-failing")
          ? "auth-failing"
          : results.every((item) => item.status === "ok")
            ? "ok"
            : "unknown",
      };
    },
    revoke: async (id) => {
      await page.goto(provider.url.replace(/\/create$/, ""));
      await session.prompt(
        `Revoke registry token ${id} in this browser, then press Enter...`,
      );
    },
    revoked: (id) => page.evaluate(revokedScript(id)),
  });
  if (stateFile && result.changed) {
    state[key] = [result.token_id];
    await writeFile(stateFile, JSON.stringify(state), { mode: 0o600 });
  }
  session.outcome = {
    status: result.changed ? "configured" : "complete",
    secret,
    ...(result.path ? { secret_scope: result.path } : {}),
    ...(result.fallbackReason
      ? { fallback_reason: result.fallbackReason }
      : {}),
  };
}

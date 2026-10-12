import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { githubServices } from "./github.mjs";
import {
  cycleCredential,
  secretName,
  trackedTokenIds,
  recordTokenId,
  completeRotation,
} from "./ci-credential-cycle.mjs";
import { pageFetchScript, runCratesApiStep } from "./crates-api.mjs";
import {
  listCratesTokens,
  selectCratesToken,
  replacementTokenRequest,
} from "./crates-tokens.mjs";
import { AUTH_FAILURE_PATTERNS } from "./publishing-policy.mjs";

/** Explicit maintenance of existing legacy secrets; ordinary crates setup uses OIDC. */
export async function rotateCratesToken(session) {
  const { options, plan } = session;
  const secret = secretName(
    options.rotateToken,
    "crates-io",
    `${plan.repository.github_owner}/${plan.repository.github_repository}`,
  );
  if (
    options.noBrowser ||
    options.browserOptions?.attach ||
    options.browserOptions?.importScope === "full"
  ) {
    throw new Error(
      "token rotation requires a dedicated profile with domain-scoped sign-in import",
    );
  }
  const settings =
    JSON.parse(
      await readFile(
        path.join(options.repository, ".package-registry-manager.json"),
        "utf8",
      ).catch((error) => {
        if (error.code === "ENOENT") {
          return "{}";
        }
        throw error;
      }),
    ).tokens?.["crates-io"] ?? {};
  if (settings.level && !["repo", "org"].includes(settings.level)) {
    throw new Error("token secret level must be repo or org");
  }
  const github = options.github ?? githubServices(options);
  const owner = plan.repository.github_owner;
  const slug = `${owner}/${plan.repository.github_repository}`;
  if (!owner || !plan.repository.github_repository) {
    throw new Error("token rotation requires GitHub repository coordinates");
  }
  const key = `${settings.level === "repo" ? slug : owner}:${secret}`;
  if (options.rotatedSecrets?.has(key)) {
    const outcome = options.rotatedSecrets.get(key);
    if (!outcome) {
      throw new Error(
        "previous rotation could not be verified; credentials retained",
      );
    }
    session.outcome = outcome;
    return;
  }
  options.rotatedSecrets?.set(key, null);
  const targets =
    settings.level === "repo"
      ? [slug]
      : (options.secretRepositories?.[`crates-io:${secret}`] ?? [slug]);
  const scope = settings.level === "repo" ? { repo: slug } : { org: owner };
  let metadata;
  try {
    metadata = await github.secrets(scope).getMetadata(secret);
  } catch (error) {
    if (![403, 404, 422].includes(error.status)) {
      throw error;
    }
  }
  metadata ??= await github.secrets({ repo: slug }).getMetadata(secret);
  if (!metadata) {
    throw new Error(`existing secret ${secret} was not found`);
  }
  // Keep access unchanged and test consumers beyond the current plan selection too.
  const granted =
    metadata.selected_repositories?.map((repo) => repo.full_name) ?? [];
  const consumers = [...new Set([...targets, ...granted])];
  if (
    scope.org &&
    ["all", "private"].includes(metadata.visibility) &&
    !options.accountScan
  ) {
    throw new Error(
      "rotation of an all/private organization secret requires --org to discover every consumer",
    );
  }
  if (options.accountScan && options.scanErrors) {
    throw new Error("token rotation requires a complete account scan");
  }
  if (
    !options.yes &&
    !/^y(?:es)?$/i.test(
      await session.prompt(
        "Replace the legacy token, verify consumers in CI, then revoke the old token? [y/N] ",
      ),
    )
  ) {
    throw new Error("credential creation declined");
  }
  await runCratesApiStep(session, { id: "crates-sign-in" });
  const page = await session.automatedPage(options.browserOptions, true);
  const previous = selectCratesToken(
    await listCratesTokens(page),
    secret,
    metadata,
    settings.token_id,
  );
  const request = replacementTokenRequest(previous, settings.expiry_days ?? 30);
  const stateFile =
    options.browserProfile &&
    path.join(path.dirname(options.browserProfile), "credential-ids.json");
  const state = stateFile
    ? JSON.parse(
        await readFile(stateFile, "utf8").catch((error) => {
          if (error.code === "ENOENT") {
            return "{}";
          }
          throw error;
        }),
      )
    : {};
  const persist = async () => {
    if (stateFile) {
      await mkdir(path.dirname(stateFile), { recursive: true });
      await writeFile(stateFile, JSON.stringify(state));
    }
  };
  const previousId = String(previous.id);
  const retained = Object.entries(state).some(
    ([entry, ids]) =>
      !consumers.some((repo) => entry === `${repo}:${secret}`) &&
      ids.includes(previousId),
  );
  if (options.verbose) {
    console.error(
      `Rotate ${secret}: token ${previousId}; verify ${consumers.join(", ")}`,
    );
  }
  const result = await cycleCredential(
    {
      health: async () => ({ status: "ok" }),
      metadata: async () => ({
        token_id: retained ? undefined : previousId,
        token_ids: trackedTokenIds(state, consumers, secret),
      }),
      create: async () => {
        const created = await page.evaluate(
          pageFetchScript("PUT", "/api/v1/me/tokens", request),
        );
        const token = created.body?.api_token;
        if (
          created.status !== 200 ||
          !token?.token ||
          token.id === undefined ||
          token.id === null
        ) {
          throw new Error(
            `crates.io token creation failed (${created.status})`,
          );
        }
        return {
          id: String(token.id),
          value: token.token,
          expires_at: token.expired_at ?? request.api_token.expired_at,
        };
      },
      ensure: async (credential) => {
        const broad =
          scope.org && ["all", "private"].includes(metadata.visibility);
        const stored = broad
          ? {
              ...(await github.secrets(scope).set(secret, credential.value, {
                visibility: metadata.visibility,
                expiresAt: credential.expires_at,
              })),
              path: "organization",
            }
          : await github.secrets(scope).ensure(secret, {
              ...(scope.org
                ? { repos: consumers, visibility: "selected" }
                : {}),
              health: { status: "auth-failing" },
              rotateBeforeMs: 0,
              acquire: async () => ({
                value: credential.value,
                expiresAt: credential.expires_at,
              }),
            });
        for (const repo of consumers) {
          if (
            stored.path === "organization" &&
            (await github.secrets({ repo }).getMetadata(secret))
          ) {
            await github.secrets({ repo }).set(secret, credential.value, {
              expiresAt: credential.expires_at,
            });
          }
        }
        if (stored.valueChanged === false) {
          throw new Error(
            "gh-manager did not store the replacement credential",
          );
        }
        recordTokenId(state, consumers, secret, credential.id);
        await persist();
        return stored;
      },
      test: async () => {
        for (const repo of consumers) {
          const result = await github.health.test(secret, {
            scope: { repo },
            failurePatterns: AUTH_FAILURE_PATTERNS["crates-io"],
          });
          if (result.status !== "ok") {
            return { status: "unknown" };
          }
        }
        return { status: "ok" };
      },
      revoke: async (id) => {
        const revoked = await page.evaluate(
          pageFetchScript(
            "DELETE",
            `/api/v1/me/tokens/${encodeURIComponent(id)}`,
          ),
        );
        if (![200, 204].includes(revoked.status)) {
          throw new Error(
            `crates.io token revocation failed (${revoked.status})`,
          );
        }
      },
      revoked: async (id) =>
        !(await listCratesTokens(page)).some(
          (token) => String(token.id) === id,
        ),
    },
    { force: true },
  );
  completeRotation(state, consumers, secret, result.token_id);
  await persist();
  session.outcome = {
    status: "configured",
    secret,
    secret_scope: result.path,
    fallback_reason: result.fallbackReason,
    token_id: result.token_id,
  };
  options.rotatedSecrets?.set(key, session.outcome);
  console.log(
    `Migrate these repositories to crates.io trusted publishing before deleting ${secret}: ${consumers.join(", ")}`,
  );
}

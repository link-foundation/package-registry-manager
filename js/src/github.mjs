import { delay, retryDelay } from "./request-policy.mjs";
import {
  createRestClient,
  resolveToken,
  createRepoManager,
  createRunManager,
  createSecretHealth,
  createSecretManager,
} from "@link-foundation/gh-manager";

/** GitHub-only operations, supplied by the installed gh-manager library. */
export function githubServices(options = {}) {
  const { token } = resolveToken(options);
  if (!token && !options.rest) {
    throw new Error(
      "sign in to GitHub with gh auth login before scanning or setting secrets",
    );
  }
  const rest =
    options.rest ??
    createRestClient({ token, fetch: retryingGithubFetch(options) });
  const log = {
    debug: (message) => {
      if (options.verbose) {
        console.error(message);
      }
    },
  };
  const context = { rest, log };
  return {
    rest,
    repos: createRepoManager(context),
    runs: createRunManager(context),
    health: createSecretHealth(context),
    secrets: (scope) => {
      if (typeof scope.repo === "string") {
        const [owner, name, extra] = scope.repo.split("/");
        if (!owner || !name || extra !== undefined) {
          throw new Error("invalid repository secret scope");
        }
        scope = { ...scope, repo: { owner, name } };
      }
      return createSecretManager({ ...context, scope });
    },
  };
}

/** Retry rate-limited reads before gh-manager consumes response headers. */
export function retryingGithubFetch(options = {}) {
  return async (url, init = {}) => {
    const fetcher = options.githubFetch ?? globalThis.fetch;
    for (let attempt = 0; ; attempt++) {
      const response = await fetcher(url, init);
      if (
        ![403, 429].includes(response.status) ||
        attempt >= 3 ||
        !["GET", "HEAD"].includes(init.method ?? "GET")
      ) {
        return response;
      }
      const reset =
        Number(response.headers?.get("x-ratelimit-reset")) * 1000 - Date.now();
      const wait = Math.max(
        retryDelay(response.headers?.get("retry-after"), attempt),
        Number.isFinite(reset) ? reset : 0,
      );
      if (options.verbose) {
        console.error(
          `GitHub read rate limited (HTTP ${response.status}); retry ${attempt + 1} in ${wait}ms`,
        );
      }
      await response.body?.cancel();
      await (options.sleep ?? delay)(wait);
    }
  };
}

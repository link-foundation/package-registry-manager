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
  const rest = options.rest ?? createRestClient({ token });
  const log = {
    debug: (message) => {
      if (options.verbose) {
        console.error(message);
      }
    },
  };
  const context = { rest, log };
  return {
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

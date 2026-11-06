// Embedded by the Rust CLI. Only protocol messages use stdout; all failures are
// redacted. The browser remains Rust's native Browser Commander session.
import { createInterface } from "node:readline";
import { realpath } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

/** Run the shared Packagist engine with a Rust-owned browser over stdin/stdout. */
export async function runPackagistDriver(sources) {
  const lines = createInterface({ input: process.stdin, crlfDelay: Infinity })[
    Symbol.asyncIterator
  ]();
  const receive = async () => JSON.parse((await lines.next()).value);
  const rpc = async (operation, argument) => {
    process.stdout.write(`${JSON.stringify({ operation, argument })}\n`);
    const answer = await receive();
    if (answer.error) {
      throw new Error("native browser operation failed (output withheld)");
    }
    return answer.value;
  };
  try {
    const input = await receive();
    let gh;
    for (const directory of (process.env.PATH ?? "").split(path.delimiter)) {
      try {
        const binary = await realpath(path.join(directory, "gh-manager"));
        gh = await import(
          pathToFileURL(path.resolve(path.dirname(binary), "../src/index.js"))
            .href
        );
        break;
      } catch {
        /* Continue to the pinned npm-exec package's bin directory. */
      }
    }
    if (!gh) {
      throw new Error("installed gh-manager unavailable");
    }
    const { token } = gh.resolveToken({});
    const rest = gh.createRestClient({ token });
    const context = { rest, log: { debug() {} } };
    const github = {
      rest,
      runs: gh.createRunManager(context),
      repair: gh.createRepairVerifier(context),
      secrets: ({ repo }) => {
        const [owner, name] = repo.split("/");
        return gh.createSecretManager({
          ...context,
          scope: { repo: { owner, name } },
        });
      },
    };
    const { engine, verification } = await loadPackagistEngine(sources);
    const session = {
      plan: input.plan,
      options: { ...input.options, github },
      stepResults: [],
      prompt: (message) => rpc("prompt", message),
      automatedPage: async () => ({
        goto: (url) => rpc("goto", url),
        evaluate: (script) => rpc("evaluate", script),
      }),
    };
    const handlers = {
      "ensure-packagist-package": engine.ensurePackagistPackage,
      "packagist-update-api": engine.ensurePackagistUpdate,
      "packagist-github-hook": engine.ensurePackagistHook,
      "verify-packagist-package": async () => {
        const retry = await verification.retryPackagistJobs(session);
        return {
          ...(await verification.verifyPackagistPublication(session)),
          retry,
        };
      },
    };
    for (const step of input.plan.steps) {
      if (!handlers[step.id]) {
        continue;
      }
      const result = await handlers[step.id](session, step);
      session.stepResults.push({ step: step.id, ...result });
      if (result.stop) {
        break;
      }
    }
    process.stdout.write(
      `${JSON.stringify({ results: session.stepResults })}\n`,
    );
  } catch {
    process.stdout.write(
      `${JSON.stringify({ error: "Packagist engine failed; sensitive output withheld" })}\n`,
    );
  } finally {
    process.stdin.destroy();
  }
}

/** Load the exact embedded modules without requiring an installed npm tree. */
export async function loadPackagistEngine(sources) {
  const data = (source) =>
    `data:text/javascript;base64,${Buffer.from(source).toString("base64")}`;
  const page = data(sources.page);
  const slug =
    "const repositorySlug = (repo) => repo?.github_owner && repo?.github_repository ? `${repo.github_owner}/${repo.github_repository}` : null;";
  const setup = data(
    sources.setup
      .replace(
        'import packagistPage from "./packagist-page.mjs";',
        `import packagistPage from "${page}";`,
      )
      .replace(
        'import { githubServices } from "./github.mjs";',
        'const githubServices = () => { throw new Error("GitHub context required"); };',
      )
      .replace(
        'import { repositorySlug } from "./repository-identity.mjs";',
        slug,
      )
      .replace(
        'import { delay } from "./request-policy.mjs";',
        "const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));",
      ),
  );
  const publication = data(
    sources.publication
      .replace('from "./packagist-setup.mjs"', `from "${setup}"`)
      .replace(
        'import { repositorySlug } from "./repository-identity.mjs";',
        slug,
      )
      .replace(
        'import { delay } from "./request-policy.mjs";',
        "const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));",
      ),
  );
  const engine = await import(setup);
  const verification = await import(publication);
  return { engine, verification };
}

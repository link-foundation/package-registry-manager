import { spawn } from "node:child_process";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { verifyCredentialWorkflow } from "./credential-verification.mjs";
import { needsRotation, rotateCredential } from "./credential-cycle.mjs";
import {
  TOKEN_PROVIDERS,
  tokenFormScript,
  READ_TOKEN,
  revokedScript,
} from "./credential-browser.mjs";

/** Exact argv and stdin transport. Suppress all output from secret mutations. */
export function secretCommand(args, input, cwd) {
  return new Promise((resolve, reject) => {
    const child = spawn("gh-manager", args, {
      cwd,
      stdio: ["pipe", "pipe", "pipe"],
    });
    let output = "";
    child.stdout.on("data", (chunk) => {
      if (input === undefined) {
        output += chunk;
      }
    });
    child.stderr.on("data", () => {});
    child.on("error", () =>
      reject(
        new Error(
          "gh-manager with secret get-metadata/ensure support is required (gh-manager#6)",
        ),
      ),
    );
    child.stdin.on("error", () => {});
    child.on("close", (code) =>
      code === 0
        ? resolve(output)
        : reject(
            new Error(
              "gh-manager secret operation failed; verify installation, permissions and secret support (gh-manager#6)",
            ),
          ),
    );
    child.stdin.end(input ?? "");
  });
}

/** Execute the token lifecycle without putting values in argv, diagnostics or files. */
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
  if (!/^[\w.-]+\.ya?ml$/.test(settings.verification_workflow ?? "")) {
    throw new Error(
      "configure tokens.<registry>.verification_workflow for a reviewed dry-run login workflow before creating credentials",
    );
  }
  const { github_owner: owner, github_repository: repository } =
    plan.repository;
  if (!owner || !repository) {
    throw new Error("token setup requires GitHub repository coordinates");
  }
  const secret =
    settings.secret ?? plan.package.token_secrets?.[0] ?? provider.secret;
  if (!/^[A-Z][A-Z0-9_]*$/.test(secret)) {
    throw new Error("invalid registry secret name");
  }
  if (settings.level && !["repo", "org"].includes(settings.level)) {
    throw new Error("token secret level must be repo or org");
  }
  const scope =
    settings.level === "repo"
      ? ["--repo", `${owner}/${repository}`]
      : ["--org", owner];
  const visibility =
    settings.level === "repo"
      ? []
      : ["--visibility", "selected", "--repos", `${owner}/${repository}`];
  const workflow = await readFile(
    path.join(
      options.repository,
      ".github/workflows",
      settings.verification_workflow,
    ),
    "utf8",
  );
  if (
    !workflow.includes("workflow_dispatch") ||
    !workflow.includes("inputs.prm_nonce") ||
    !workflow.includes("run-name:")
  ) {
    throw new Error(
      "verification workflow requires workflow_dispatch input prm_nonce and run-name containing inputs.prm_nonce",
    );
  }
  const days = settings.expiry_days ?? 30;
  if (!Number.isInteger(days) || days < 1 || days > 90) {
    throw new Error("token expiry_days must be between 1 and 90");
  }
  if (options.noBrowser) {
    throw new Error(
      "registry token creation requires the dedicated automated browser",
    );
  }
  if (
    options.browserOptions?.attach ||
    options.browserOptions?.importScope === "full"
  ) {
    throw new Error(
      "token setup requires a dedicated profile with domain-scoped sign-in import",
    );
  }
  const run = options.secretCommand ?? secretCommand;
  const previous = JSON.parse(
    await run(
      ["secret", "get-metadata", secret, ...scope, "--json"],
      undefined,
      options.repository,
    ),
  );
  const verify = () =>
    verifyCredentialWorkflow(session, settings.verification_workflow);
  if (!needsRotation(previous)) {
    try {
      await verify();
      return;
    } catch {
      previous.valid = false;
    }
  }
  if (
    !options.yes &&
    !/^y(?:es)?$/i.test(
      await session.prompt(
        `Create and store scoped ${secret}, verify it, then revoke the replaced token? [y/N] `,
      ),
    )
  ) {
    throw new Error("credential creation declined");
  }
  if (session.automation) {
    await session.automation.close();
  }
  session.automation = null;
  const page = await session.automatedPage(
    { ...options.browserOptions, importScope: "domains" },
    true,
  );
  const expiry = new Date(Date.now() + days * 86_400_000).toISOString();
  await rotateCredential(previous, {
    create: async () => {
      await page.goto(provider.url);
      await session.prompt(
        `Sign in, then press Enter to fill the narrow publishing scope (${provider.scope})...`,
      );
      await page.evaluate(
        tokenFormScript(provider, `prm-${repository}`, expiry),
      );
      await session.prompt(
        `Review the scope and expiry, create the token in this browser, then press Enter (never paste its value)...`,
      );
      const credential = await page.evaluate(READ_TOKEN);
      if (!credential.expires_at) {
        throw new Error(
          "registry did not provide a verifiable expiry; no secret was stored",
        );
      }
      return credential;
    },
    store: (credential) =>
      run(
        [
          "secret",
          "ensure",
          secret,
          ...scope,
          ...visibility,
          "--expires-at",
          credential.expires_at,
          "--token-id",
          credential.id,
          "--registry",
          plan.registry,
        ],
        credential.value,
        options.repository,
      ),
    verify,
    revoke: async (id) => {
      await page.goto(provider.url.replace(/\/create$/, ""));
      await session.prompt(
        `Revoke registry token ${id} in this browser, then press Enter...`,
      );
    },
    revoked: async (id) => page.evaluate(revokedScript(id)),
  });
  console.log("  Credential verified; replaced token revocation verified.");
}

// Long-lived registry tokens: the repository secrets that trusted publishing
// replaces, and the one-time crates.io first-publish token, whose revocation
// is verified against the crates.io API before setup ends.
import { readFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import { registryTokenSecrets } from "./publishers.mjs";
import { registryEndpoint, USER_AGENT } from "./registry-state.mjs";

/** Where the crates.io first-publish token is revoked. */
export const CRATES_TOKENS_URL = "https://crates.io/settings/tokens";

/**
 * Steps that list the repository's secrets and delete the long-lived
 * registry tokens trusted publishing makes unnecessary, one confirmation each.
 */
export function tokenSecretSteps(packageInfo, slug) {
  const names = registryTokenSecrets(packageInfo.registry);
  return [
    {
      id: "audit-token-secrets",
      title: "Look for leftover long-lived token secrets",
      kind: "check",
      description: `Trusted publishing makes long-lived tokens such as ${names.join(", ")} unnecessary. Organization secrets are not listed here; an organization owner removes them with gh secret delete --org.`,
      command: {
        program: "gh",
        args: ["secret", "list", "--repo", slug, "--json", "name"],
      },
      cwd: ".",
    },
    {
      id: "delete-token-secret",
      title: "Delete the leftover token secrets",
      kind: "command",
      description:
        "Remove each long-lived token secret that no workflow reads any more.",
      command: {
        program: "gh",
        args: ["secret", "delete", "{token_secret}", "--repo", slug],
      },
      when: "token-secret-present",
      cwd: ".",
      confirm: true,
    },
  ];
}

/**
 * Splits the registry token secrets in `gh secret list --json name` output
 * into the ones a workflow still reads, which must be replaced by trusted
 * publishing first, and leftovers that can be deleted.
 */
export function auditTokenSecrets(output, packageInfo) {
  const listed = new Set(
    JSON.parse(String(output || "[]")).map((item) => item.name),
  );
  const present = registryTokenSecrets(packageInfo.registry).filter((name) =>
    listed.has(name),
  );
  const read = new Set(packageInfo.token_secrets ?? []);
  return {
    inUse: present.filter((name) => read.has(name)),
    leftover: present.filter((name) => !read.has(name)),
  };
}

/**
 * Prints which registry token secrets remain and returns the leftovers, the
 * ones no workflow reads any more and that can be deleted.
 */
export function reportTokenSecrets(output, packageInfo) {
  const { inUse, leftover } = auditTokenSecrets(output, packageInfo);
  for (const name of inUse) {
    console.log(
      `  ${name} is still read by a workflow; switch that workflow to trusted publishing before deleting it.`,
    );
  }
  for (const name of leftover) {
    console.log(`  ${name} is no longer needed with trusted publishing.`);
  }
  return leftover;
}

/** Returns cargo's home directory, as cargo resolves it. */
export function cargoHome(env = process.env, home = os.homedir()) {
  return env.CARGO_HOME || path.join(home, ".cargo");
}

/**
 * Reads the crates.io token `cargo login` stored, or `undefined` when cargo
 * keeps it elsewhere (a credential provider) or holds none.
 */
export async function readCargoToken(directory = cargoHome()) {
  for (const name of ["credentials.toml", "credentials"]) {
    const contents = await readFile(path.join(directory, name), "utf8").catch(
      () => "",
    );
    const token = registryToken(contents);
    if (token) {
      return token;
    }
  }
  return undefined;
}

/** Extracts `token` from the `[registry]` table of cargo's credentials. */
export function registryToken(contents) {
  let table = "";
  for (const line of contents.split(/\r?\n/)) {
    const header = /^\s*\[([^\]]+)\]\s*(?:#.*)?$/.exec(line);
    if (header) {
      table = header[1].trim();
      continue;
    }
    const value = /^\s*token\s*=\s*(?:"([^"]*)"|'([^']*)')/.exec(line);
    if (table === "registry" && value) {
      return value[1] ?? value[2];
    }
  }
  return undefined;
}

/**
 * Classifies the crates.io answer to a token on an endpoint only the website
 * session may use: `revoked` when crates.io no longer knows the token,
 * `active` when it still authenticates, `undefined` when unclear.
 */
export function tokenState(status, body) {
  const detail = (body?.errors ?? [])
    .map((error) => String(error?.detail ?? ""))
    .join(" ");
  if (
    (status === 401 || status === 403) &&
    /authentication failed|does not match the format/i.test(detail)
  ) {
    return "revoked";
  }
  if (
    (status >= 200 && status < 300) ||
    /only be performed on the crates\.io website/i.test(detail)
  ) {
    return "active";
  }
  return undefined;
}

/**
 * Asks crates.io whether a token still authenticates. The token is sent only
 * to the crates.io API and never printed.
 */
export async function checkCratesToken(base, token, options = {}) {
  const fetcher = options.fetch ?? globalThis.fetch;
  const url = `${base}/me/tokens`;
  try {
    const response = await fetcher(url, {
      headers: {
        accept: "application/json",
        authorization: token,
        "user-agent": USER_AGENT,
      },
      signal: AbortSignal.timeout(options.timeoutMs ?? 10_000),
    });
    if (options.verbose) {
      console.error(`GET ${url} -> ${response.status}`);
    }
    const body = await response.json().catch(() => undefined);
    return tokenState(response.status, body);
  } catch (error) {
    if (options.verbose) {
      console.error(`GET ${url} failed: ${error.message}`);
    }
    return undefined;
  }
}

/**
 * Confirms that crates.io rejects the first-publish token `cargo login`
 * stored, asking the maintainer to revoke it and re-checking up to
 * `attempts` times. `ask` waits for the maintainer.
 */
export async function verifyTokenRevoked(ask, options = {}) {
  const token = await readCargoToken(cargoHome(options.env ?? process.env));
  if (!token) {
    console.error(
      `warning: cargo holds no readable crates.io token, so its revocation cannot be verified; check ${CRATES_TOKENS_URL}`,
    );
    return;
  }
  const base = registryEndpoint("crates-io", options.env ?? process.env);
  const attempts = options.attempts ?? 3;
  for (let attempt = 1; ; attempt += 1) {
    const state = await checkCratesToken(base, token, options);
    if (state === "revoked") {
      console.log(
        "  crates.io rejects the first-publish token: it is revoked.",
      );
      return;
    }
    if (state === undefined) {
      console.error(
        `warning: crates.io gave no clear answer about the first-publish token; check that it is revoked at ${CRATES_TOKENS_URL}`,
      );
      return;
    }
    if (attempt >= attempts) {
      throw new Error(
        `the first-publish token still authenticates on crates.io; revoke it at ${CRATES_TOKENS_URL}`,
      );
    }
    await ask(
      `crates.io still accepts the first-publish token. Revoke it at ${CRATES_TOKENS_URL}, then press Enter...`,
    );
  }
}

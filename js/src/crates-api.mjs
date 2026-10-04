// The crates.io first publish through the crates.io API (#26): the automated
// browser holds the crates.io session, a short-lived token limited to one
// crate is created from inside the page, handed only to the `cargo publish`
// child process, used to attach the trusted publisher, then revoked and the
// revocation verified. The token never reaches the DOM, the terminal, the
// clipboard, or cargo's credentials file.
import { readFile } from "node:fs/promises";
import path from "node:path";

import { registryEndpoint, USER_AGENT } from "./registry-state.mjs";
import { offerSignInImport, signInDomains } from "./sign-in-import.mjs";
import { checkCratesToken, CRATES_TOKENS_URL } from "./tokens.mjs";

/** Where the automated browser signs in to crates.io. */
export const CRATES_IO_URL = "https://crates.io/";
/** Where a crates.io account verifies its email address. */
export const CRATES_PROFILE_URL = "https://crates.io/settings/profile";
/** How long the first-publish token lives, in milliseconds. */
export const TOKEN_LIFETIME_MS = 60 * 60_000;
/** The only endpoints the first-publish token may call. */
export const TOKEN_SCOPES = ["publish-new", "trusted-publishing"];
/** How long the sign-in waits for the maintainer, in milliseconds. */
export const SIGN_IN_TIMEOUT_MS = 10 * 60_000;

/** The `PUT /api/v1/me/tokens` body: one crate, two endpoints, one hour. */
export function tokenRequest(crate, now = new Date()) {
  return {
    api_token: {
      name: `prm-first-publish-${crate}`,
      crate_scopes: [crate],
      endpoint_scopes: TOKEN_SCOPES,
      expired_at: new Date(now.getTime() + TOKEN_LIFETIME_MS).toISOString(),
    },
  };
}

/** The `POST /api/v1/trusted_publishing/github_configs` body. */
export function githubConfigRequest(crate, publisher) {
  return {
    github_config: {
      crate,
      repository_owner: publisher.organization,
      repository_name: publisher.repository,
      workflow_filename: publisher.workflow,
      environment: publisher.environment || null,
    },
  };
}

/**
 * A script that calls the crates.io API from inside the page with its
 * session cookie and returns `{ status, body }` to the tool, never to the DOM.
 */
export function pageFetchScript(method, apiPath, body) {
  const init = {
    method,
    credentials: "same-origin",
    headers: { accept: "application/json" },
  };
  if (body !== undefined) {
    init.headers["content-type"] = "application/json";
    init.body = JSON.stringify(body);
  }
  return `(async () => {
  const response = await fetch(${JSON.stringify(apiPath)}, ${JSON.stringify(init)});
  const body = await response.json().catch(() => null);
  return { status: response.status, body };
})()`;
}

/** Clicks crates.io's "Log in with GitHub" button; returns whether it found one. */
export const LOGIN_CLICK_SCRIPT = `(() => {
  const button = [...document.querySelectorAll("button, a")].find((element) =>
    /log\\s*in with github/i.test(element.textContent || ""),
  );
  if (button) {
    button.click();
  }
  return Boolean(button);
})()`;

/** The first error detail of a crates.io answer. */
export function errorDetail(body) {
  return String(body?.errors?.[0]?.detail ?? "no error detail");
}

/** Reads the crates.io session of the page from `GET /api/v1/me`. */
export async function readSession(page) {
  const { status, body } = await page.evaluate(
    pageFetchScript("GET", "/api/v1/me"),
  );
  return {
    signedIn: status === 200,
    login: body?.user?.login,
    emailVerified: body?.user?.email_verified === true,
    emailSent: body?.user?.email_verification_sent === true,
  };
}

/**
 * Signs the automated browser in to crates.io: an existing session is kept,
 * otherwise "Log in with GitHub" is clicked and `/me` polled until it answers
 * 200. crates.io lets only accounts with a verified email publish, so an
 * unverified one stops here with the settings link.
 */
export async function signIn(page, options = {}) {
  await page.goto(CRATES_IO_URL);
  let session = await readSession(page);
  const signedInBefore = session.signedIn;
  if (!session.signedIn) {
    console.log(
      "  Not signed in to crates.io; finish the GitHub sign-in in the automated browser.",
    );
    if (!(await page.evaluate(LOGIN_CLICK_SCRIPT))) {
      console.log('  Click "Log in with GitHub" on crates.io.');
    }
    const deadline = Date.now() + (options.timeoutMs ?? SIGN_IN_TIMEOUT_MS);
    const interval = options.pollIntervalMs ?? 2_000;
    while (!session.signedIn) {
      if (Date.now() > deadline) {
        throw new Error("timed out waiting for the crates.io sign-in");
      }
      await new Promise((resolve) => setTimeout(resolve, interval));
      session = await readSession(page);
    }
  }
  console.log(
    `  Signed in to crates.io as ${session.login ?? "your account"}.`,
  );
  if (!session.emailVerified) {
    const sent = session.emailSent
      ? " A verification email was sent; follow its link,"
      : " Add and verify an email address";
    throw new Error(
      `crates.io only lets accounts with a verified email address publish or attach trusted publishers.${sent} at ${CRATES_PROFILE_URL}, then re-run setup`,
    );
  }
  return { ...session, signedInBefore };
}

/** The question asked once before the token is created. */
export function firstPublishQuestion(crate, version, publisher) {
  const release = version ? `${crate} v${version}` : crate;
  const attach = publisher
    ? `, attach ${publisher.workflow} as trusted publisher`
    : "";
  return `Create a 1-hour token limited to crate ${crate} with publish-new + trusted-publishing, publish ${release}${attach}, revoke the token? [y/N] `;
}

/**
 * Reads `version` from the `[package]` table of a Cargo.toml, or `undefined`
 * when it is inherited from the workspace.
 */
export async function crateVersion(directory) {
  const text = await readFile(path.join(directory, "Cargo.toml"), "utf8").catch(
    () => "",
  );
  let table = "";
  for (const line of text.split(/\r?\n/)) {
    const header = /^\s*\[([^\]]+)\]/.exec(line);
    if (header) {
      table = header[1].trim();
      continue;
    }
    const value = /^\s*version\s*=\s*["']([^"']+)["']/.exec(line);
    if (table === "package" && value) {
      return value[1];
    }
  }
  return undefined;
}

/** Calls the crates.io API with the token from the tool, never from the page. */
async function tokenCall(base, token, method, apiPath, body, options) {
  const fetcher = options.fetch ?? globalThis.fetch;
  const url = `${base}${apiPath}`;
  const headers = {
    accept: "application/json",
    authorization: token,
    "user-agent": USER_AGENT,
  };
  if (body !== undefined) {
    headers["content-type"] = "application/json";
  }
  try {
    const response = await fetcher(url, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(options.timeoutMs ?? 30_000),
    });
    if (options.verbose) {
      console.error(`${method} ${url} -> ${response.status}`);
    }
    const parsed = await response.json().catch(() => null);
    return { status: response.status, body: parsed };
  } catch (error) {
    if (options.verbose) {
      console.error(`${method} ${url} failed: ${error.message}`);
    }
    return { status: 0, body: { errors: [{ detail: error.message }] } };
  }
}

/**
 * Attaches the trusted publisher with the page's session cookie, for a crate
 * that already exists or when the token could not. Returns whether it worked.
 */
export async function attachWithSession(page, crate, publisher) {
  const { status, body } = await page.evaluate(
    pageFetchScript(
      "POST",
      "/api/v1/trusted_publishing/github_configs",
      githubConfigRequest(crate, publisher),
    ),
  );
  return reportAttach(status, body);
}

function reportAttach(status, body) {
  if (status >= 200 && status < 300) {
    console.log("  crates.io stored the trusted publisher.");
    return true;
  }
  console.error(
    `warning: crates.io did not attach the trusted publisher (${status}: ${errorDetail(body)})`,
  );
  return false;
}

/**
 * Publishes a crate for the first time through the crates.io API.
 * `publish(env)` runs `cargo publish` with `env` added to its own environment
 * only; `waitForRegistry()` polls until the crate is visible. The token is
 * revoked in `finally`, and the run fails while crates.io still accepts it.
 * Returns `{ attached }`, whether the trusted publisher was attached.
 */
export async function firstPublish({
  page,
  crate,
  publisher,
  publish,
  waitForRegistry,
  now = new Date(),
  env = process.env,
  ...options
}) {
  const created = await page.evaluate(
    pageFetchScript("PUT", "/api/v1/me/tokens", tokenRequest(crate, now)),
  );
  // The token lives only in `secret` and is dropped once revoked.
  const secret = { token: created.body?.api_token?.token };
  if (created.status !== 200 || !secret.token) {
    throw new Error(
      `crates.io did not create the first-publish token (${created.status}: ${errorDetail(created.body)}); nothing was published`,
    );
  }
  const tokenId = created.body.api_token.id;
  console.log(
    "  Created a 1-hour first-publish token; it stays in this process only.",
  );
  const base = registryEndpoint("crates-io", env);
  let attached = false;
  let failure;
  try {
    const result = await publish({ CARGO_REGISTRY_TOKEN: secret.token });
    if (result.code !== 0) {
      throw new Error(`cargo exited with status ${result.code}`);
    }
    await waitForRegistry();
    if (publisher) {
      const answer = await tokenCall(
        base,
        secret.token,
        "POST",
        "/trusted_publishing/github_configs",
        githubConfigRequest(crate, publisher),
        options,
      );
      attached = reportAttach(answer.status, answer.body);
    }
  } catch (error) {
    failure = error;
  }
  try {
    await revoke(page, base, secret.token, tokenId, options);
  } catch (error) {
    throw failure
      ? new Error(`${error.message}; the run failed before: ${failure.message}`)
      : error;
  } finally {
    delete secret.token;
  }
  if (failure) {
    throw failure;
  }
  return { attached };
}

/**
 * Revokes the token with `DELETE /api/v1/tokens/current`, falls back to the
 * page session, and verifies that crates.io rejects it.
 */
async function revoke(page, base, token, tokenId, options) {
  const answer = await tokenCall(
    base,
    token,
    "DELETE",
    "/tokens/current",
    undefined,
    options,
  );
  let state = await checkCratesToken(base, token, options);
  if (state !== "revoked" && tokenId !== undefined) {
    if (options.verbose) {
      console.error(
        `token revocation answered ${answer.status}; revoking through the browser session`,
      );
    }
    await page
      .evaluate(pageFetchScript("DELETE", `/api/v1/me/tokens/${tokenId}`))
      .catch(() => undefined);
    state = await checkCratesToken(base, token, options);
  }
  if (state === "revoked") {
    console.log("  Revoked the first-publish token; crates.io rejects it now.");
    return;
  }
  if (state === undefined) {
    console.error(
      `warning: crates.io gave no clear answer about the first-publish token; check that it is revoked at ${CRATES_TOKENS_URL}`,
    );
    return;
  }
  throw new Error(
    `the first-publish token still authenticates on crates.io; revoke it at ${CRATES_TOKENS_URL}`,
  );
}

/**
 * Runs a crates.io API step of a setup session: `crates-sign-in`,
 * `first-publish`, `attach-trusted-publisher`, or the cleanup step
 * `crates-sign-out`.
 */
export async function runCratesApiStep(session, step) {
  const { options, plan, conditions } = session;
  if (options.noBrowser) {
    throw new Error(
      "the crates.io API steps drive the automated browser; drop --no-browser, or pass --manual for the manual checklist",
    );
  }
  const crate = plan.package.name;
  const domains = signInDomains(plan.registry);
  let page = await session.automatedPage();
  switch (step.id) {
    case "crates-sign-in": {
      await page.goto(CRATES_IO_URL);
      if (!(await readSession(page)).signedIn) {
        // Whether imported or signed in by hand, the sign-in is this run's.
        conditions.add("crates-signed-in");
        if (await offerSignInImport(session, domains)) {
          page = await session.automatedPage();
        }
      }
      await signIn(page, options);
      return;
    }
    case "first-publish": {
      const version = await crateVersion(session.cwd(step));
      const publisher = plan.trusted_publisher;
      if (step.confirm && !options.yes) {
        const answer = await session.prompt(
          firstPublishQuestion(crate, version, publisher),
        );
        if (!/^(?:y|yes)$/i.test(answer)) {
          throw new Error(
            "the first publish was declined; no token was created and nothing was published",
          );
        }
      }
      const { attached } = await firstPublish({
        page,
        crate,
        publisher,
        publish: (env) => session.runProcess(step, true, env),
        waitForRegistry: () =>
          session.wait({ id: "wait-for-registry", url: step.url }),
        env: options.env ?? process.env,
        fetch: options.fetch,
        verbose: options.verbose,
      });
      if (attached) {
        conditions.delete("trust-missing");
      }
      return;
    }
    case "attach-trusted-publisher":
      if (!plan.trusted_publisher) {
        console.log(
          "  No GitHub repository or release workflow is known, so the trusted publisher is configured in the form.",
        );
      } else if (await attachWithSession(page, crate, plan.trusted_publisher)) {
        conditions.delete("trust-missing");
        return;
      }
      conditions.add("trust-api-failed");
      return;
    case "crates-sign-out":
      if (options.keepSession) {
        console.log(
          `Keeping the ${domains.join(" / ")} sign-in in the automated profile (--keep-session); the next run reuses it. No token is kept.`,
        );
      } else if (page.clearCookies) {
        await page.clearCookies(domains);
        console.log(
          `  Removed the ${domains.join(" / ")} sign-in cookies from the automated profile.`,
        );
      }
      return;
    default:
      throw new Error(`no crates.io API step ${step.id}`);
  }
}

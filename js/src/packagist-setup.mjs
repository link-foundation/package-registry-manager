import { readFile } from "node:fs/promises";
import path from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import packagistPage from "./packagist-page.mjs";
import { githubServices } from "./github.mjs";
import { repositorySlug } from "./repository-identity.mjs";
import { delay } from "./request-policy.mjs";

const execute = promisify(execFile);
const PROFILE = "https://packagist.org/profile/";
const NAME = /^[a-z0-9](?:[a-z0-9_.-]*)\/[a-z0-9](?:[a-z0-9_.-]*)$/;
const failed = (status, reason) => ({ status, reason, stop: true });
export const services = (session) =>
  session.options.github ?? githubServices(session.options);
export const repository = (session) =>
  `https://github.com/${repositorySlug(session.plan.repository)}`;

/** Public and authenticated calls never expose response bodies or thrown errors. */
export async function packagistRequest(session, endpoint, credential, body) {
  try {
    const response = await (session.options.fetch ?? globalThis.fetch)(
      `https://packagist.org${endpoint}`,
      {
        method: body ? "POST" : "GET",
        headers: {
          accept: "application/json",
          "user-agent": "package-registry-manager",
          ...(body ? { "content-type": "application/json" } : {}),
          ...(credential
            ? {
                authorization: `Bearer ${credential.username}:${credential.token}`,
              }
            : {}),
        },
        ...(body ? { body: JSON.stringify(body) } : {}),
        credentials: "omit",
        redirect: "error",
        signal: AbortSignal.timeout(session.options.timeoutMs ?? 10000),
      },
    );
    if (session.options.verbose) {
      console.error(
        `Packagist ${body ? "POST" : "GET"} ${endpoint} -> ${response.status}`,
      );
    }
    return {
      status: response.status,
      json: await response.json().catch(() => null),
    };
  } catch {
    return { status: "transport" };
  }
}

/** Fresh login always uses the same page/profile and never auto-approves a grant. */
export async function acquirePackagistCredential(session, main = false) {
  if (session.options.noBrowser) {
    return failed(
      "authentication-required",
      "Packagist profile authentication requires a browser",
    );
  }
  try {
    const page = await session.automatedPage(
      { ...session.options.browserOptions, importScope: "domains" },
      true,
    );
    const read = () => page.evaluate(`(${packagistPage})(${main})`);
    await page.goto(PROFILE);
    let state = await read();
    for (
      let attempt = 0;
      attempt < 2 && state.status !== "authenticated";
      attempt++
    ) {
      // Never replay a stale callback, clear cookies, or discard the user's profile.
      await page.goto("https://packagist.org/login/");
      const login = await read();
      if (login.oauth?.startsWith("https://packagist.org/")) {
        await page.goto(login.oauth);
        for (let poll = 0; poll < 10; poll++) {
          await (session.options.sleep ?? delay)(500);
          state = await read();
          if (state.status !== "authentication-required") {
            break;
          }
        }
        if (state.status === "profile-needed") {
          await page.goto(PROFILE);
        }
      }
      state = await read();
      if (state.status !== "stale-oauth") {
        break;
      }
    }
    if (state.status !== "authenticated") {
      await session.prompt(
        "Complete Packagist login or account registration in this browser, including any required GitHub grant, then press Enter (never paste a token)...",
      );
      await page.goto(PROFILE);
      state = await read();
    }
    if (state.status !== "authenticated" || !state.username || !state.token) {
      return failed(
        "authentication-required",
        "Packagist sign-in, account creation or an unapproved grant still requires user presence",
      );
    }
    return state;
  } catch {
    return failed(
      "transport-unknown",
      "Packagist browser operation failed; sensitive output withheld",
    );
  }
}

function sameRepository(value, expected) {
  return (
    typeof value === "string" &&
    value
      .replace(/\.git\/?$/, "")
      .replace(/\/$/, "")
      .toLowerCase() === expected.toLowerCase()
  );
}

/** Validate once, discover first, submit once only on an authoritative 404. */
export async function ensurePackagistPackage(session) {
  const item = session.plan.package;
  if (
    item.manifest !== "composer.json" ||
    !NAME.test(item.name) ||
    !repositorySlug(session.plan.repository)
  ) {
    return failed(
      "unsupported",
      "Packagist needs a root composer.json, vendor/package name and GitHub repository",
    );
  }
  try {
    const cwd = session.options.repository ?? ".";
    const descriptor = JSON.parse(
      await readFile(path.join(cwd, "composer.json"), "utf8"),
    );
    if (descriptor.name !== item.name) {
      return failed(
        "descriptor-mismatch",
        "root composer.json differs from the inspected package; never rename a published package",
      );
    }
    await (session.options.execFile ?? execute)(
      "composer",
      ["validate", "--strict", "--no-interaction"],
      { cwd, maxBuffer: 1024 * 1024 },
    );
  } catch {
    return failed(
      "validation-failed",
      "root composer.json failed Composer validation (output withheld)",
    );
  }
  const endpoint = `/packages/${item.name}.json`;
  const inspect = async () => {
    const result = await packagistRequest(session, endpoint);
    if (result.status === 200) {
      return sameRepository(
        result.json?.package?.repository,
        repository(session),
      ) && result.json?.package?.name === item.name
        ? { status: "verified", registered: true }
        : failed(
            "repository-mismatch",
            "registered package belongs to another repository; no package was changed",
          );
    }
    return result;
  };
  let result = await inspect();
  if (result.status === "verified" || result.status === "repository-mismatch") {
    return result;
  }
  if (result.status !== 404) {
    return failed(
      "transport-unknown",
      "package registration could not be read authoritatively",
    );
  }
  if (
    !session.options.yes &&
    !/^y(?:es)?$/i.test(
      await session.prompt(
        `Submit ${item.name} from ${repository(session)} once? [y/N] `,
      ),
    )
  ) {
    return failed("interrupted", "package submission declined");
  }
  const credential = await acquirePackagistCredential(session, true);
  if (credential.status !== "authenticated") {
    return credential;
  }
  const submitted = await packagistRequest(
    session,
    "/api/create-package",
    credential,
    { repository: repository(session) },
  );
  credential.token = null;
  // Independent readback also handles a concurrent submit or an unknown POST outcome.
  result = await inspect();
  if (result.status === "verified" || result.status === "repository-mismatch") {
    return result;
  }
  return failed(
    submitted.status === "transport"
      ? "transport-unknown"
      : "submission-rejected",
    "submission has no verified package readback; do not retry blindly",
  );
}

/** Encrypt selected credentials, read presence back, then test the Safe update API. */
export async function ensurePackagistUpdate(session, step) {
  const credential = await acquirePackagistCredential(session);
  if (credential.status !== "authenticated") {
    return credential;
  }
  const github = services(session);
  const slug = repositorySlug(session.plan.repository);
  const names = (step.secrets ?? []).map((entry) => entry.secret);
  if (!slug || names.length !== 2 || new Set(names).size !== 2) {
    return failed(
      "unsupported",
      "exactly two selected Packagist username/token secret names are required",
    );
  }
  try {
    const secrets = github.secrets({ repo: slug });
    // GitHub cannot decrypt existing secrets: write the currently selected Safe
    // pair so the independent test proves the values just encrypted, not names.
    for (const [index, name] of names.entries()) {
      const stored = await secrets.set(
        name,
        index ? credential.token : credential.username,
      );
      if (stored?.verified === false) {
        return failed(
          "storage-failed",
          "gh-manager did not verify encrypted storage",
        );
      }
      if (!(await secrets.getMetadata(name))) {
        return failed(
          "storage-failed",
          "encrypted secret presence readback failed",
        );
      }
    }
    const result = await packagistRequest(
      session,
      "/api/update-package",
      credential,
      { repository: repository(session) },
    );
    if (
      result.status !== 202 ||
      result.json?.status !== "success" ||
      !Array.isArray(result.json.jobs) ||
      !result.json.jobs.length
    ) {
      return failed(
        result.status === "transport" ? "transport-unknown" : "auth-rejected",
        "Packagist did not independently accept the Safe credential update",
      );
    }
    // Held only for this session's hook operation; never included in a result.
    session.packagistCredential = credential;
    return {
      status: "configured",
      secrets: names,
      update: "queued",
      jobs: result.json.jobs.length,
    };
  } catch {
    return failed(
      "storage-failed",
      "encrypted gh-manager storage failed; sensitive output withheld",
    );
  } finally {
    if (session.packagistCredential !== credential) {
      credential.token = null;
    }
  }
}

/** Inspect all pages and preserve every hook except the selected Packagist hook. */
export async function ensurePackagistHook(session) {
  const github = services(session);
  const slug = repositorySlug(session.plan.repository);
  if (!slug) {
    return failed("unsupported", "GitHub repository coordinates are required");
  }
  const base = `/repos/${slug}/hooks`;
  try {
    const hooks = [];
    for (let page = 1; ; page++) {
      const batch = await github.rest.request(
        `${base}?per_page=100&page=${page}`,
      );
      if (!Array.isArray(batch)) {
        return failed(
          "transport-unknown",
          "GitHub hook inspection returned no list",
        );
      }
      hooks.push(...batch);
      if (batch.length < 100) {
        break;
      }
    }
    const candidates = hooks.filter((hook) => {
      try {
        const url = new URL(hook.config?.url);
        return (
          url.hostname === "packagist.org" && url.pathname === "/api/github"
        );
      } catch {
        return false;
      }
    });
    if (candidates.length > 1) {
      return failed(
        "ambiguous-hook",
        "multiple Packagist hooks require explicit selection; none changed",
      );
    }
    const credential =
      session.packagistCredential ??
      (await acquirePackagistCredential(session));
    if (!credential.token) {
      return credential;
    }
    const destination = `https://packagist.org/api/github?username=${encodeURIComponent(credential.username)}`;
    const valid = (hook) =>
      hook?.active === true &&
      hook.config?.url === destination &&
      hook.config?.content_type === "json" &&
      hook.events?.length === 1 &&
      hook.events[0] === "push";
    let hook = candidates[0];
    const body = {
      name: "web",
      active: true,
      events: ["push"],
      config: {
        url: destination,
        content_type: "json",
        insecure_ssl: "0",
        secret: credential.token,
      },
    };
    // A redacted readback cannot prove a stored secret. Test the existing hook
    // first; repair its secret only when settings/delivery fail.
    const test = async (id) => {
      const prior = await github.rest.request(
        `${base}/${id}/deliveries?per_page=100`,
      );
      const seen = new Set((prior ?? []).map((entry) => entry.id));
      await github.rest.request(`${base}/${id}/tests`, { method: "POST" });
      for (let attempt = 0; attempt < 5; attempt++) {
        const deliveries = await github.rest.request(
          `${base}/${id}/deliveries?per_page=100`,
        );
        const fresh = deliveries?.find(
          (entry) => !seen.has(entry.id) && entry.event === "push",
        );
        if (fresh) {
          const detail = await github.rest.request(
            `${base}/${id}/deliveries/${fresh.id}`,
          );
          if (detail.status_code >= 200 && detail.status_code < 300) {
            return true;
          }
          if (detail.status_code) {
            return false;
          }
        }
        await (session.options.sleep ?? delay)(1000);
      }
      return false;
    };
    let delivered = valid(hook) && (await test(hook.id));
    if (!delivered) {
      hook = await github.rest.request(hook ? `${base}/${hook.id}` : base, {
        method: hook ? "PATCH" : "POST",
        body,
      });
      if (!hook?.id) {
        return failed("hook-unverified", "GitHub returned no hook identity");
      }
      delivered = await test(hook.id);
    }
    const readback = await github.rest.request(`${base}/${hook.id}`);
    if (!valid(readback) || !delivered) {
      return failed(
        "hook-unverified",
        "Packagist hook settings or fresh push delivery are unverified",
      );
    }
    return { status: "verified", hook: hook.id, delivery: "accepted" };
  } catch {
    return failed(
      "transport-unknown",
      "GitHub hook operation failed; sensitive output withheld",
    );
  } finally {
    if (session.packagistCredential) {
      session.packagistCredential.token = null;
    }
    delete session.packagistCredential;
  }
}

// Firefox-family browsers use Browser Commander's native WebDriver/BiDi API.
// Its real/CDP launcher and migration target writers do not support them yet.
import { launchWebDriver, readBrowserCookies } from "browser-commander";

import {
  assertDedicatedProfile,
  resolveExecutable,
} from "./browser-catalogue.mjs";
import { ensureProfileIgnored } from "./profile.mjs";
import { IMPORT_CHOICES } from "./sign-in-import.mjs";

/** True only for a requested host or its subdomains, never a substring. */
export function matchesDomains(host, domains) {
  const normalized = host.replace(/^\./, "").toLowerCase();
  return domains.some(
    (domain) => normalized === domain || normalized.endsWith(`.${domain}`),
  );
}

/** The BiDi cookie shape; session cookies omit expiry. */
export function bidiCookie(cookie) {
  const result = {
    name: cookie.name,
    value: { type: "string", value: cookie.value },
    domain: cookie.domain,
    path: cookie.path ?? "/",
    httpOnly: cookie.httpOnly ?? false,
    secure: cookie.secure ?? false,
    sameSite: (cookie.sameSite ?? "Lax").toLowerCase(),
  };
  if (cookie.expires > 0) {
    result.expiry = cookie.expires;
  }
  return result;
}

/** Launches the selected installed Firefox variant and exposes the common page API. */
export async function connectWebDriverAutomation(
  { browser, profile, verbose, domains },
  dependencies = {},
) {
  // Validation happens before profile creation, including programmatic callers.
  if (
    browser.attach ||
    browser.restrictions.length ||
    Object.keys(browser.preferences).length ||
    (browser.import && browser.importScope === "full")
  ) {
    throw new Error(
      `unsupported Firefox launch options for ${browser.channel}; use --browser-import-scope domains without snapshot, preferences or restrictions`,
    );
  }
  if (browser.import && domains.length === 0) {
    throw new Error("a domains import needs the registry's sign-in domains");
  }
  const executablePath = await (
    dependencies.resolveExecutable ?? resolveExecutable
  )({
    channel: browser.channel.toLowerCase(),
    executablePath: browser.executable,
  });
  assertDedicatedProfile(profile);
  await ensureProfileIgnored(profile, { verbose });
  const connection = await (dependencies.launchWebDriver ?? launchWebDriver)({
    browser: "firefox",
    executablePath,
    userDataDir: profile,
    headless: false,
    bidi: true,
    verbose,
  });
  try {
    await connection.page.bidiCommand("storage.getCookies", {});
    if (browser.import && !IMPORT_CHOICES.includes(browser.import.browser)) {
      const read = dependencies.readCookies ?? readBrowserCookies;
      const seen = new Set();
      for (const domain of domains) {
        const cookies = await read({
          browser: browser.import.browser,
          profile: browser.import.profile ?? undefined,
          domainFilter: domain,
          cache: false,
        });
        for (const cookie of cookies) {
          const key = JSON.stringify([cookie.domain, cookie.path, cookie.name]);
          if (!matchesDomains(cookie.domain, domains) || seen.has(key)) {
            continue;
          }
          seen.add(key);
          await connection.page.bidiCommand("storage.setCookie", {
            cookie: bidiCookie(cookie),
          });
        }
      }
    }
  } catch (error) {
    await connection.close().catch(() => {});
    throw new Error(
      `could not prepare ${browser.channel} sign-in cookies (WebDriver BiDi storage is required)`,
      { cause: error },
    );
  }
  return {
    goto: (url) => connection.page.goto(url),
    evaluate: (script) => connection.page.evaluate(script),
    close: () => connection.close(),
    async clearCookies(names) {
      const { cookies } = await connection.page.bidiCommand(
        "storage.getCookies",
        {},
      );
      for (const cookie of cookies) {
        if (matchesDomains(cookie.domain, names)) {
          await connection.page.bidiCommand("storage.deleteCookies", {
            filter: {
              name: cookie.name,
              domain: cookie.domain,
              path: cookie.path,
            },
          });
        }
      }
    },
  };
}

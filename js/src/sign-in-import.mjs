// Reusing a registry sign-in from an installed browser (#26): the automated
// profile stays fresh and clean by default; when a step needs a sign-in it
// does not hold, the tool offers registry-domain cookies, then supported
// saved logins when no cookies exist. Credential values are never logged;
// discovery counts them through Browser Commander.
import * as commander from "browser-commander";
import { mkdtemp, rm, realpath } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { detachedRunner } from "./browser.mjs";

import { catalogueName } from "./browser-catalogue.mjs";
import { detectDefaultBrowser } from "./default-browser.mjs";

/** `--browser-import` values that pick the source browser for the user. */
export const IMPORT_CHOICES = ["default", "auto"];
/** `--browser-import-scope` values. */
export const IMPORT_SCOPES = ["domains", "full"];

/** The domains whose cookies hold each registry's sign-in. */
export const SIGN_IN_DOMAINS = {
  "docker-hub": ["docker.com", "docker.io"],
  "maven-central": ["sonatype.com"],
  "vscode-marketplace": ["dev.azure.com", "login.microsoftonline.com"],
  "open-vsx": ["open-vsx.org", "github.com"],
  "chrome-web-store": ["google.com"],
  rubygems: ["rubygems.org", "github.com"],
  nuget: ["nuget.org", "login.microsoftonline.com"],
  jsr: ["jsr.io", "github.com"],
  "crates-io": ["crates.io", "github.com"],
  npm: ["npmjs.com"],
  pypi: ["pypi.org", "github.com"],
};

/** Every import id from Browser Commander's public catalogue. */
export function importSources() {
  return [...commander.BROWSER_IDS];
}

/** The sign-in domains of a registry, or none. */
export function signInDomains(registry, route = "work") {
  if (registry === "nuget") {
    return [
      "nuget.org",
      route === "personal"
        ? "login.live.com"
        : route === "github"
          ? "github.com"
          : "login.microsoftonline.com",
    ];
  }
  return SIGN_IN_DOMAINS[registry] ?? [];
}

/** The display name of a source browser. */
export function sourceName(browser) {
  return catalogueName(browser);
}

/** The source id of a default-browser display name, when it can be imported. */
export function sourceId(displayName) {
  return importSources().find((id) => sourceName(id) === displayName);
}

/**
 * The migration Browser Commander runs: the whole profile for `full`, or
 * registry-domain cookies, with usable saved logins for the automatic fallback.
 */
export function migrationFor(source, scope, domains) {
  const migration = { browser: source.browser };
  if (source.profile) {
    migration.profile = source.profile;
  }
  if (scope === "domains" && domains.length > 0) {
    migration.include = source.savedLogins
      ? ["cookies", "passwords", "passkeys"]
      : ["cookies"];
    migration.domains = domains;
  }
  return migration;
}

/**
 * Lists the installed browser profiles that hold cookies for any of
 * `domains`, the default browser first. Each entry is
 * `{ browser, profile, label, domains }`; cookie values are discarded.
 */
export async function findSignInSources(domains, options = {}) {
  const list = options.listProfiles ?? commander.listBrowserProfiles;
  const read = options.readCookies ?? commander.readBrowserCookies;
  const preferred = options.defaultSource;
  const browsers = (options.sources ?? importSources()).toSorted(
    (left, right) => Number(right === preferred) - Number(left === preferred),
  );
  const found = [];
  const physical = new Set();
  let accessReported = false;
  const report = async (browser, error) => {
    if (
      browser === "safari" &&
      (options.platform ?? process.platform) === "darwin" &&
      /EPERM|EACCES|operation not permitted|permission denied/i.test(
        `${error.code} ${error.message}`,
      ) &&
      !accessReported
    ) {
      accessReported = true;
      (options.warn ?? console.error)(
        "Safari sign-ins are unreadable: enable Full Disk Access for your terminal app in Privacy & Security, then restart the terminal app and retry.",
      );
      await (
        options.openSettings ??
        (() =>
          detachedRunner()("open", [
            "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles",
          ]))
      )().catch(() => {});
    } else if (options.verbose) {
      console.error(`could not read ${browser} sign-in data: ${error.message}`);
    }
  };
  for (const browser of browsers) {
    const profiles = await list({ browser }).catch(async (error) => {
      await report(browser, error);
      return [];
    });
    for (const profile of profiles) {
      const location = profile.path
        ? await realpath(profile.path).catch(() => path.resolve(profile.path))
        : `${browser}:${profile.name}`;
      if (physical.has(location)) {
        continue;
      }
      physical.add(location);
      if (profile.error) {
        options.diagnostic?.({
          browser,
          profile: profile.name,
          cookies: "unreadable",
          passkey_presence: "unknown",
          provider_availability: "unknown",
          passkey_export: "unsupported",
          authentication: "not-yet-verified",
        });
        await report(browser, new Error(profile.error));
        continue;
      }
      const matched = [];
      let unreadable = false;
      for (const domain of domains) {
        const cookies = await read({
          browser,
          profile: profile.name,
          domainFilter: domain,
          ignoreDecryptionErrors: true,
          cache: false,
        }).catch(async (error) => {
          unreadable = true;
          await report(browser, error);
          return [];
        });
        if (cookies.length > 0) {
          matched.push(domain);
        }
      }
      options.diagnostic?.({
        browser,
        profile: profile.name,
        cookies: unreadable
          ? "unreadable"
          : matched.length
            ? "present"
            : "not-found",
        passkey_presence: "unknown",
        provider_availability: "unknown",
        passkey_export: "unsupported",
        authentication: "not-yet-verified",
      });
      if (matched.length > 0) {
        const name = profile.displayName ?? profile.name;
        found.push({
          browser,
          profile: profile.name,
          label:
            profiles.length > 1
              ? `${sourceName(browser)} (${name})`
              : sourceName(browser),
          domains: matched,
        });
      }
    }
  }
  return found;
}

/** Count usable saved logins through upstream migration into a disposable profile. */
export async function findSavedLoginSources(domains, options = {}) {
  if (
    commander.findBrowserSource(options.targetBrowser ?? "chrome")
      ?.controlProtocol === "bidi"
  ) {
    (options.warn ?? console.error)(
      "Saved-password migration into Firefox is unavailable in Browser Commander; use --browser-channel chrome for saved-login autofill, or sign in in Firefox.",
    );
    return [];
  }
  const list = options.listProfiles ?? commander.listBrowserProfiles;
  const migrate = options.migrateProfile ?? commander.migrateProfile;
  const browsers = (options.sources ?? importSources()).toSorted(
    (a, b) =>
      Number(b === options.defaultSource) - Number(a === options.defaultSource),
  );
  const found = [];
  const physical = new Set();
  const installed = [];
  for (const browser of browsers) {
    const profiles = await list({ browser }).catch(() => []);
    if (profiles.length) {
      installed.push(sourceName(browser));
    }
    for (const profile of profiles) {
      const location = profile.path
        ? await realpath(profile.path).catch(() => path.resolve(profile.path))
        : `${browser}:${profile.name}`;
      if (physical.has(location)) {
        continue;
      }
      physical.add(location);
      const directory = await mkdtemp(
        path.join(os.tmpdir(), "prm-login-probe-"),
      );
      try {
        const report = await migrate({
          from: { browser, profile: profile.name },
          to: directory,
          include: ["passwords", "passkeys"],
          domains,
          targetBrowser: options.targetBrowser ?? "chrome",
        });
        options.diagnostic?.({
          browser,
          profile: profile.name,
          saved_passwords:
            report.migrated.passwords > 0 ? "present" : "unknown",
          passkey_presence:
            report.migrated.passkeys > 0 ? "present" : "unknown",
          provider_availability: "unknown",
          passkey_export: "unsupported",
          authentication: "not-yet-verified",
        });
        if (report.migrated.passwords > 0 || report.migrated.passkeys > 0) {
          found.push({
            browser,
            profile: profile.name,
            label: sourceName(browser),
            domains,
            savedLogins: true,
          });
        }
      } catch {
        options.diagnostic?.({
          browser,
          profile: profile.name,
          saved_passwords: "unreadable",
          passkey_presence: "unknown",
          provider_availability: "unknown",
          passkey_export: "unsupported",
          authentication: "not-yet-verified",
        });
        if (options.verbose) {
          console.error(
            `saved-login import unavailable from ${sourceName(browser)}`,
          );
        }
      } finally {
        await rm(directory, { recursive: true, force: true });
      }
    }
  }
  if (!found.length) {
    (options.warn ?? console.error)(
      `Saved-login export did not produce a usable credential for ${domains.join(", ")} in ${installed.join(", ") || "installed browsers"}. Presence may be unknown or unreadable. Safari saved passwords require an export; passkey metadata presence, provider availability, export support and authentication success are separate. The installed Browser Commander cannot export passkeys. Sign in in the browser.`,
    );
  }
  return found;
}

/** The source id of the system default browser, when it can be imported. */
export async function defaultSource(options = {}) {
  const name =
    options.defaultBrowserName ??
    (await detectDefaultBrowser({ verbose: options.verbose }));
  return name ? sourceId(name) : undefined;
}

/**
 * Asks once which installed browser's sign-in to import, the default browser
 * first; `auto` takes it without asking. Returns the chosen source or
 * `undefined`.
 */
export async function chooseSignInSource(
  sources,
  domains,
  ask,
  auto,
  log = console.log,
) {
  if (sources.length === 0) {
    return undefined;
  }
  const [first] = sources;
  const question = `Import your ${domains.join(" / ")} sign-in from ${first.label} into the automated profile?`;
  if (auto) {
    log(`  ${question} yes (--browser-import auto)`);
    return first;
  }
  if (sources.length > 1) {
    sources.forEach((source, index) =>
      log(`  ${index + 1}. ${source.label} (${source.domains.join(", ")})`),
    );
  }
  const choices = sources.length > 1 ? `[Y/n/1-${sources.length}]` : "[Y/n]";
  const answer = String(await ask(`${question} ${choices} `)).trim();
  if (/^(?:n|no)$/i.test(answer)) {
    return undefined;
  }
  const index = Number(answer);
  return Number.isInteger(index) && index >= 1 && index <= sources.length
    ? sources[index - 1]
    : first;
}

/**
 * Resolves `--browser-import default|auto` to an installed source browser:
 * the system default browser, or for `auto` the first browser with a
 * sign-in for `domains`. Throws when `default` names a browser Browser
 * Commander cannot identify.
 */
export async function resolveImportChoice(choice, domains, options = {}) {
  const preferred = await defaultSource(options);
  if (choice === "default") {
    if (!preferred) {
      throw new Error(
        "--browser-import default: the default browser is unknown or cannot be imported (Browser Commander sources: " +
          `${importSources().join(", ")})`,
      );
    }
    return { browser: preferred, profile: null };
  }
  const discovery = {
    ...options,
    defaultSource: preferred,
  };
  const sources = await findSignInSources(domains, discovery);
  const [found] = sources.length
    ? sources
    : await findSavedLoginSources(domains, discovery);
  return found
    ? {
        browser: found.browser,
        profile: found.profile,
        ...(found.savedLogins ? { savedLogins: true } : {}),
      }
    : null;
}

/**
 * The browser options a launch uses: `--browser-import default|auto` is
 * resolved to an installed browser, except where a step offers the import
 * once it knows the sign-in is missing (`deferred`).
 */
export async function launchBrowser(browser, domains, options = {}) {
  options = { targetBrowser: browser?.channel ?? "chrome", ...options };
  const choice = browser?.import?.browser;
  if (!IMPORT_CHOICES.includes(choice)) {
    return browser;
  }
  const source = options.deferred
    ? null
    : await resolveImportChoice(choice, domains, options);
  if (!source && !options.deferred) {
    console.error(
      `warning: no installed browser holds a sign-in for ${domains.join(", ") || "this registry"}; the automated profile starts without one`,
    );
  }
  return { ...browser, import: source };
}

/**
 * Offers, once, to import the sign-in for `domains` from an installed browser
 * into the automated profile, then relaunches it with the import. Only a
 * dedicated profile is offered the import, and only without a named
 * `--browser-import`, which already migrated at launch. Returns whether the
 * sign-in was imported.
 */
export async function offerSignInImport(session, domains, verifySession) {
  const browser = session.options.browserOptions;
  const choice = browser?.import?.browser;
  if (
    !browser ||
    browser.attach ||
    (choice && !IMPORT_CHOICES.includes(choice))
  ) {
    return false;
  }
  const options = {
    targetBrowser: browser.channel,
    verbose: session.options.verbose,
    diagnostic: (item) => {
      (session.signInEvidence ??= []).push(item);
      (session.options.log ?? console.log)(
        JSON.stringify({ local_credential: item }),
      );
    },
    ...session.options.importDiscovery,
  };
  const preferred = await defaultSource(options);
  let sources = await findSignInSources(domains, {
    ...options,
    defaultSource: preferred,
  });
  const attempt = async (candidates) => {
    const selected = await chooseSignInSource(
      candidates,
      domains,
      (message) => session.prompt(message),
      Boolean(choice),
      session.options.log ?? console.log,
    );
    if (!selected) {
      return false;
    }
    for (const source of [
      selected,
      ...candidates.filter((candidate) => candidate !== selected),
    ]) {
      // Browser Commander #189: password import replaces Login Data. Never import
      // into a successful or caller-selected target; each attempt gets a fresh one.
      const profile = await mkdtemp(path.join(os.tmpdir(), "prm-signin-"));
      (session.importProfiles ??= []).push(profile);
      session.options.browserProfile = profile;
      try {
        await session.reconnect({
          ...browser,
          import: {
            browser: source.browser,
            profile: source.profile,
            ...(source.savedLogins ? { savedLogins: true } : {}),
          },
        });
      } catch {
        (session.options.log ?? console.log)(
          `Import from ${source.label} was unavailable; trying another candidate.`,
        );
        continue;
      }
      if (!verifySession || (await verifySession())) {
        (session.options.log ?? console.log)(
          `  Imported ${domains.join(" / ")} from ${source.label}${verifySession ? "; signed-in identity verified" : "; authentication remains unverified"}.`,
        );
        return true;
      }
    }
    return false;
  };
  if (await attempt(sources)) {
    return true;
  }
  sources = await findSavedLoginSources(domains, {
    ...options,
    defaultSource: preferred,
  });
  return attempt(sources);
}

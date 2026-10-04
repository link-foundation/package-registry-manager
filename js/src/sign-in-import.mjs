// Reusing a registry sign-in from an installed browser (#26): the automated
// profile stays fresh and clean by default; when a step needs a sign-in it
// does not hold, the tool offers to import only the cookies of the registry's
// sign-in domains from a browser that has them. Cookie values are never
// printed, logged, or cached: they are only counted.
import * as commander from "browser-commander";

import { detectDefaultBrowser } from "./default-browser.mjs";

/** Browser Commander's cookie sources, should a release stop exporting them. */
const FALLBACK_SOURCES = ["chrome", "edge", "brave", "chromium", "firefox"];
/** `--browser-import` values that pick the source browser for the user. */
export const IMPORT_CHOICES = ["default", "auto"];
/** `--browser-import-scope` values. */
export const IMPORT_SCOPES = ["domains", "full"];

/** The domains whose cookies hold each registry's sign-in. */
export const SIGN_IN_DOMAINS = {
  "crates-io": ["crates.io", "github.com"],
  npm: ["npmjs.com"],
  pypi: ["pypi.org", "github.com"],
};

/** Display names of the source browsers, as the default-browser query names them. */
const SOURCE_NAMES = {
  chrome: "Google Chrome",
  edge: "Microsoft Edge",
  brave: "Brave",
  chromium: "Chromium",
  firefox: "Firefox",
};

/**
 * Every browser `--browser-import` can read, from Browser Commander. Safari,
 * Opera, Vivaldi, and Arc follow once Browser Commander reads them
 * (link-foundation/browser-commander#114).
 */
export function importSources() {
  return [...(commander.SUPPORTED_COOKIE_BROWSERS ?? FALLBACK_SOURCES)];
}

/** The sign-in domains of a registry, or none. */
export function signInDomains(registry) {
  return SIGN_IN_DOMAINS[registry] ?? [];
}

/** The display name of a source browser. */
export function sourceName(browser) {
  return SOURCE_NAMES[browser] ?? browser;
}

/** The source id of a default-browser display name, when it can be imported. */
export function sourceId(displayName) {
  const entry = Object.entries(SOURCE_NAMES).find(
    ([, name]) => name === displayName,
  );
  return entry && importSources().includes(entry[0]) ? entry[0] : undefined;
}

/**
 * The migration Browser Commander runs: the whole profile for `full`, or
 * only the cookies of `domains`.
 */
export function migrationFor(source, scope, domains) {
  const migration = { browser: source.browser };
  if (source.profile) {
    migration.profile = source.profile;
  }
  if (scope === "domains" && domains.length > 0) {
    migration.include = ["cookies"];
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
  for (const browser of browsers) {
    const profiles = await list({ browser }).catch(() => []);
    for (const profile of profiles) {
      const matched = [];
      for (const domain of domains) {
        const cookies = await read({
          browser,
          profile: profile.path,
          domainFilter: domain,
          ignoreDecryptionErrors: true,
          cache: false,
        }).catch((error) => {
          if (options.verbose) {
            console.error(
              `could not read ${browser} cookies for ${domain}: ${error.message}`,
            );
          }
          return [];
        });
        if (cookies.length > 0) {
          matched.push(domain);
        }
      }
      if (matched.length > 0) {
        const name = profile.displayName ?? profile.name;
        found.push({
          browser,
          profile: profile.path,
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
export async function chooseSignInSource(sources, domains, ask, auto) {
  if (sources.length === 0) {
    return undefined;
  }
  const [first] = sources;
  const question = `Import your ${domains.join(" / ")} sign-in from ${first.label} into the automated profile?`;
  if (auto) {
    console.log(`  ${question} yes (--browser-import auto)`);
    return first;
  }
  if (sources.length > 1) {
    sources.forEach((source, index) =>
      console.log(
        `  ${index + 1}. ${source.label} (${source.domains.join(", ")})`,
      ),
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
 * Commander cannot read yet, such as Safari.
 */
export async function resolveImportChoice(choice, domains, options = {}) {
  const preferred = await defaultSource(options);
  if (choice === "default") {
    if (!preferred) {
      throw new Error(
        "--browser-import default: the default browser cannot be imported yet (Browser Commander reads " +
          `${importSources().join(", ")}; see link-foundation/browser-commander#114)`,
      );
    }
    return { browser: preferred, profile: null };
  }
  const [found] = await findSignInSources(domains, {
    ...options,
    defaultSource: preferred,
  });
  return found ? { browser: found.browser, profile: found.profile } : null;
}

/**
 * The browser options a launch uses: `--browser-import default|auto` is
 * resolved to an installed browser, except where a step offers the import
 * once it knows the sign-in is missing (`deferred`).
 */
export async function launchBrowser(browser, domains, options = {}) {
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
export async function offerSignInImport(session, domains) {
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
    verbose: session.options.verbose,
    ...session.options.importDiscovery,
  };
  const preferred = await defaultSource(options);
  const sources =
    choice === "default"
      ? [preferred].filter(Boolean).map((id) => ({
          browser: id,
          profile: null,
          label: sourceName(id),
          domains,
        }))
      : await findSignInSources(domains, {
          ...options,
          defaultSource: preferred,
        });
  const source = await chooseSignInSource(
    sources,
    domains,
    (message) => session.prompt(message),
    Boolean(choice),
  );
  if (!source) {
    return false;
  }
  await session.reconnect({
    ...browser,
    import: { browser: source.browser, profile: source.profile },
  });
  console.log(
    `  Imported the ${domains.join(" / ")} sign-in from ${source.label}.`,
  );
  return true;
}

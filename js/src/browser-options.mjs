import path from "node:path";

import {
  findBrowserSource,
  LAUNCH_RESTRICTION_PRESETS,
  LAUNCH_RESTRICTIONS,
} from "browser-commander";

import { installedDescription, validateChannel } from "./browser-catalogue.mjs";
import {
  IMPORT_CHOICES,
  IMPORT_SCOPES,
  importSources,
} from "./sign-in-import.mjs";

/**
 * What `--browser-import` accepts: every browser Browser Commander reads,
 * plus `default` (the system default browser) and `auto` (accept the
 * sign-in import offer, the default browser first).
 */
export function importBrowsers() {
  return [...importSources(), ...IMPORT_CHOICES];
}
/** How `--browser-attach` reaches the user's own browser. */
export const ATTACH_MODES = ["snapshot", "extension"];

/**
 * Every launch restriction and preset `--browser-restriction` accepts, from
 * Browser Commander's shared catalogue.
 */
export function restrictionNames() {
  return [
    ...LAUNCH_RESTRICTIONS.map((restriction) => restriction.id),
    ...Object.keys(LAUNCH_RESTRICTION_PRESETS),
  ];
}

/**
 * Validates the browser options of the automated profile. Nothing changes by
 * default: a fresh dedicated profile fills forms. `--browser-import` migrates
 * a real profile into it, and `--browser-attach` uses the user's own browser
 * instead, as a temporary snapshot or through the companion extension.
 */
export function parseBrowserOptions({
  channel = "chrome",
  executable,
  importFrom,
  importScope,
  attach,
  preferences = [],
  restrictions = [],
  profileGiven = false,
} = {}) {
  const options = {
    channel: attach === "extension" ? channel : validateChannel(channel),
    executable: executable ? path.resolve(executable) : null,
    import: importFrom ? parseImport(importFrom) : null,
    importScope: parseImportScope(importScope, importFrom),
    attach: attach ? parseAttach(attach) : null,
    preferences: parsePreferences(preferences),
    restrictions: validateRestrictions(restrictions),
  };
  if (options.attach && options.import) {
    throw new Error(
      "--browser-attach cannot be combined with --browser-import",
    );
  }
  if (options.attach && profileGiven) {
    throw new Error(
      "--browser-attach cannot be combined with --browser-profile",
    );
  }
  if (
    options.attach?.mode === "extension" &&
    (options.executable ||
      options.restrictions.length > 0 ||
      Object.keys(options.preferences).length > 0)
  ) {
    throw new Error(
      "--browser-attach extension cannot be combined with --browser-executable, --browser-pref, or --browser-restriction",
    );
  }
  return options;
}

/** Parses `<browser>[:profile]`, `default`, or `auto`. */
export function parseImport(spec, discovery = {}) {
  const separator = spec.indexOf(":");
  const browser = separator === -1 ? spec : spec.slice(0, separator);
  const profile = separator === -1 ? null : spec.slice(separator + 1);
  const choice = IMPORT_CHOICES.includes(browser);
  const source = findBrowserSource(browser);
  if ((!source && !choice) || profile === "" || (choice && profile !== null)) {
    throw new Error(
      `--browser-import must be <${importSources().join("|")}>[:profile], default, or auto. ${installedDescription(discovery)}`,
    );
  }
  return { browser: source?.id ?? browser, profile };
}

/**
 * Parses `--browser-import-scope`: `full` migrates the whole profile and
 * `domains` only the registry's sign-in cookies. Without it a named browser
 * is migrated fully, as before, and `default`, `auto`, and the sign-in offer
 * import only the sign-in domains.
 */
export function parseImportScope(scope, importFrom) {
  if (scope === undefined || scope === null) {
    const named = importFrom && !IMPORT_CHOICES.includes(importFrom);
    return named ? "full" : "domains";
  }
  if (!IMPORT_SCOPES.includes(scope)) {
    throw new Error("--browser-import-scope must be full or domains");
  }
  return scope;
}

/** Parses `snapshot`, `snapshot:<profile>`, or `extension`. */
export function parseAttach(spec) {
  if (spec === "extension") {
    return { mode: "extension" };
  }
  const match = /^snapshot(?::(.+))?$/.exec(spec);
  if (!match) {
    throw new Error(
      "--browser-attach must be 'snapshot', 'snapshot:<profile>', or 'extension'",
    );
  }
  return { mode: "snapshot", profile: match[1] ?? null };
}

/**
 * Builds the preferences object from `key=value` pairs. Dotted keys nest, and
 * a value is JSON when it parses as JSON, otherwise a string.
 */
export function parsePreferences(pairs) {
  const preferences = {};
  for (const pair of pairs) {
    const separator = pair.indexOf("=");
    const key = separator === -1 ? "" : pair.slice(0, separator);
    if (!/^[A-Za-z0-9_-]+(?:\.[A-Za-z0-9_-]+)*$/.test(key)) {
      throw new Error(`--browser-pref must be key=value, got '${pair}'`);
    }
    const parts = key.split(".");
    const last = parts.pop();
    let target = preferences;
    for (const part of parts) {
      if (!isObject(target[part])) {
        target[part] = {};
      }
      target = target[part];
    }
    target[last] = preferenceValue(pair.slice(separator + 1));
  }
  return preferences;
}

function preferenceValue(text) {
  try {
    const value = JSON.parse(text);
    return Number.isFinite(value) || typeof value !== "number" ? value : text;
  } catch {
    return text;
  }
}

function isObject(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Rejects a restriction or preset that is not in the shared catalogue. */
export function validateRestrictions(names) {
  const known = restrictionNames();
  for (const name of names) {
    if (!known.includes(name)) {
      throw new Error(
        `unknown --browser-restriction '${name}'; choose from ${known.join(", ")}`,
      );
    }
  }
  return [...new Set(names)];
}

/** The installed browser a snapshot copies for a launch channel. */
export function snapshotBrowser(channel) {
  return findBrowserSource(channel)?.id ?? channel;
}

/** Describes where forms are filled, for the browser prerequisite. */
export function automatedDescription({
  channel = "chrome",
  profile,
  attach,
  import: source,
} = {}) {
  if (attach?.mode === "extension") {
    return "your own browser through the Browser Commander extension";
  }
  if (attach?.mode === "snapshot") {
    return `a temporary snapshot of your ${snapshotBrowser(channel)} profile ${attach.profile ?? "Default"}`;
  }
  const location = profile ? ` at ${profile}` : "";
  const imported = source
    ? ` with data imported from ${source.browser}${source.profile ? `:${source.profile}` : ""}`
    : "";
  return `the automated ${channel} profile${location}${imported}`;
}

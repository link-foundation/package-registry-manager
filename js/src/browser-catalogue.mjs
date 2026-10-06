import { constants, existsSync, realpathSync } from "node:fs";
import { access } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import { BROWSER_IDS, findBrowserSource } from "browser-commander";

/** Catalogue entries reached through Browser Commander's public exports. */
export const browserSources = () => BROWSER_IDS.map(findBrowserSource);

/** Catalogue channels with a launch control protocol. */
export function launchChannels() {
  return browserSources()
    .filter((source) => ["cdp", "bidi"].includes(source.controlProtocol))
    .flatMap((source) => [source.id, ...(source.aliases ?? [])]);
}

/** A readable name from the catalogue's application path or canonical alias. */
export function catalogueName(id) {
  const source = findBrowserSource(id);
  if (!source) {
    return id;
  }
  const app = source.executables?.darwin?.[0]?.match(/\/([^/]+)\.app\//)?.[1];
  // Preserve the established short name of Brave.
  if (app) {
    return app === "Brave Browser" ? "Brave" : app;
  }
  const label = [source.id, ...(source.aliases ?? [])]
    .map((name) => name.replace(/^apple-/, ""))
    .reduce((longest, name) => (name.length > longest.length ? name : longest));
  return label
    .replace(/^apple-/, "")
    .split("-")
    .map((word) =>
      word.length <= 2
        ? word.toUpperCase()
        : word[0].toUpperCase() + word.slice(1),
    )
    .join(" ")
    .replace("Duckduckgo", "DuckDuckGo");
}

/** Installed executables or existing profile roots; never reads cookie data. */
export function installedBrowsers(options = {}) {
  const exists = options.exists ?? existsSync;
  const platform = options.platform ?? process.platform;
  const environment = options.environment ?? process.env;
  const discovery = {
    ...options,
    platform,
    environment:
      platform === "win32"
        ? Object.fromEntries(
            Object.entries(environment).map(([key, value]) => [
              key.toUpperCase(),
              value,
            ]),
          )
        : environment,
  };
  return browserSources()
    .filter((source) =>
      installedCandidates(source, discovery).some((candidate) =>
        exists(candidate),
      ),
    )
    .map((source) => source.id);
}

function installedCandidates(
  source,
  {
    platform = process.platform,
    homeDir = os.homedir(),
    environment = process.env,
  },
) {
  const pathApi = platform === "win32" ? path.win32 : path.posix;
  const variables = {
    home: homeDir,
    config: environment.XDG_CONFIG_HOME ?? pathApi.join(homeDir, ".config"),
    appSupport: pathApi.join(homeDir, "Library", "Application Support"),
    appData: environment.APPDATA ?? pathApi.join(homeDir, "AppData", "Roaming"),
    localAppData:
      environment.LOCALAPPDATA ?? pathApi.join(homeDir, "AppData", "Local"),
    programFiles: environment.PROGRAMFILES,
    programFilesX86: environment["PROGRAMFILES(X86)"],
  };
  const templates = [
    ...(source.executables?.[platform] ?? []),
    ...(source.roots?.[platform] ?? []),
  ];
  const candidates = templates.flatMap((template) => {
    const match = /^\{(\w+)\}\/?(.*)$/.exec(template);
    if (!match) {
      return [template];
    }
    const base = variables[match[1]];
    return base === undefined
      ? []
      : [pathApi.join(base, ...match[2].split("/"))];
  });
  for (const directory of (environment.PATH ?? "")
    .split(platform === "win32" ? ";" : ":")
    .filter(Boolean)) {
    for (const name of source.executableNames ?? []) {
      candidates.push(
        pathApi.join(directory, platform === "win32" ? `${name}.exe` : name),
      );
    }
  }
  return candidates;
}

/** Discovery information included when an import or channel is invalid. */
export function installedDescription(options = {}) {
  const found = installedBrowsers(options);
  return `Installed browsers found: ${found.join(", ") || "none"}.`;
}

/** Reject unavailable engines before creating an automated profile. */
export function validateChannel(channel) {
  const known = findBrowserSource(channel);
  if (["cdp", "bidi"].includes(known?.controlProtocol)) {
    return channel;
  }
  const reason = known
    ? `${channel} has no launch control protocol supported by this manager${known.family === "safari" ? " (Safari setup: https://github.com/link-foundation/browser-commander/issues/126)" : ""}`
    : `unknown --browser-channel '${channel}'`;
  throw new Error(
    `${reason}; choose from ${launchChannels().join(", ")}. ${installedDescription()}`,
  );
}

/** Resolves the selected installed binary using catalogue paths and PATH. */
export async function resolveExecutable(
  { channel, executablePath },
  discovery = {},
) {
  const platform = discovery.platform ?? process.platform;
  const environment = discovery.environment ?? process.env;
  const options = {
    ...discovery,
    platform,
    environment:
      platform === "win32"
        ? Object.fromEntries(
            Object.entries(environment).map(([key, value]) => [
              key.toUpperCase(),
              value,
            ]),
          )
        : environment,
  };
  const source = findBrowserSource(channel);
  const candidates = executablePath
    ? [executablePath]
    : installedCandidates({ ...source, roots: {} }, options);
  for (const candidate of candidates) {
    try {
      await (discovery.access ?? access)(candidate, constants.X_OK);
      return candidate;
    } catch {
      // Continue through the catalogue's executable paths and PATH names.
    }
  }
  throw new Error(
    `could not find an installed ${channel} browser; use --browser-executable`,
  );
}

/** Refuses a source profile even when reached through a symlink. */
export function assertDedicatedProfile(profile, discovery = {}) {
  const physical = (file) => {
    try {
      return realpathSync(file);
    } catch (error) {
      if (error.code !== "ENOENT") {
        throw error;
      }
      return path.join(physical(path.dirname(file)), path.basename(file));
    }
  };
  const requested = physical(path.resolve(profile));
  const roots = browserSources().flatMap((source) =>
    installedCandidates(
      {
        ...source,
        executables: {},
        executableNames: [],
      },
      discovery,
    ),
  );
  for (const root of roots) {
    const relative = path.relative(physical(path.resolve(root)), requested);
    if (
      relative === "" ||
      (!relative.startsWith(`..${path.sep}`) &&
        relative !== ".." &&
        !path.isAbsolute(relative))
    ) {
      throw new Error(
        "automation requires a dedicated profile, not a browser default profile; use --browser-import to reuse sign-in",
      );
    }
  }
}

import { existsSync } from "node:fs";
import os from "node:os";
import path from "node:path";

import { BROWSER_IDS, findBrowserSource } from "browser-commander";

/** Catalogue entries reached through Browser Commander's public exports. */
export const browserSources = () => BROWSER_IDS.map(findBrowserSource);

/** The installed channels supported by Browser Commander's real launcher. */
export function launchChannels() {
  return browserSources()
    .filter((source) => source.controlProtocol === "cdp")
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
  return browserSources()
    .filter((source) =>
      installedCandidates(source, options).some((candidate) =>
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
  if (launchChannels().includes(channel)) {
    return channel;
  }
  const known = findBrowserSource(channel);
  const reason = known
    ? `${channel} requires ${known.controlProtocol ?? known.family} control, which browser-commander's real launcher does not support yet (https://github.com/link-foundation/browser-commander/issues/114)`
    : `unknown --browser-channel '${channel}'`;
  throw new Error(
    `${reason}; choose from ${launchChannels().join(", ")}. ${installedDescription()}`,
  );
}

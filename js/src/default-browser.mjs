import { exec } from "command-stream";

import { browserSources, catalogueName } from "./browser-catalogue.mjs";

/** The command that reports the default browser, as an exact argument vector. */
export function defaultBrowserQuery(platform = process.platform) {
  if (platform === "darwin") {
    return {
      program: "defaults",
      args: [
        "read",
        "com.apple.LaunchServices/com.apple.launchservices.secure",
        "LSHandlers",
      ],
    };
  }
  if (platform === "win32") {
    return {
      program: "reg",
      args: [
        "query",
        "HKCU\\Software\\Microsoft\\Windows\\Shell\\Associations\\UrlAssociations\\https\\UserChoice",
        "/v",
        "ProgId",
      ],
    };
  }
  return { program: "xdg-settings", args: ["get", "default-web-browser"] };
}

/**
 * Extracts the browser id from the query's output: the https handler's bundle
 * id on macOS (Safari when none is set), the desktop entry on Linux, and the
 * ProgId on Windows.
 */
export function defaultBrowserId(output, platform = process.platform) {
  const text = output ?? "";
  if (platform === "darwin") {
    for (const scheme of ["https", "http"]) {
      for (const block of handlerEntries(text)) {
        const handles = new RegExp(
          `LSHandlerURLScheme\\s*=\\s*"?${scheme}"?\\s*;`,
        ).test(block);
        const role = /LSHandlerRoleAll\s*=\s*"?([\w.-]+)"?\s*;/.exec(block);
        if (handles && role) {
          return role[1];
        }
      }
    }
    return "com.apple.safari";
  }
  if (platform === "win32") {
    return /\bProgId\s+REG_SZ\s+(\S+)/i.exec(text)?.[1];
  }
  return text.trim() || undefined;
}

/**
 * The top-level dictionaries of macOS's LSHandlers array, without their nested
 * dictionaries such as LSHandlerPreferredVersions.
 */
function handlerEntries(text) {
  const entries = [];
  let depth = 0;
  let entry = "";
  for (const char of text) {
    if (char === "{") {
      depth += 1;
      entry = depth === 1 ? "" : entry;
    } else if (char === "}") {
      depth -= 1;
      if (depth === 0) {
        entries.push(entry);
      }
    } else if (depth === 1) {
      entry += char;
    }
  }
  return entries;
}

/** A readable browser name for an id, or undefined when it is unknown. */
export function browserName(id) {
  if (!id) {
    return undefined;
  }
  const needle = id.trim().toLowerCase();
  const sources = browserSources();
  const known =
    sources.find((source) =>
      Object.values(source.default ?? {})
        .flat()
        .some((candidate) => needle === candidate.toLowerCase()),
    ) ??
    sources.find(
      (source) =>
        (source.default?.win32 ?? []).some((candidate) =>
          needle.startsWith(candidate.toLowerCase()),
        ) ||
        (needle.endsWith(".desktop") &&
          (source.executableNames ?? []).some((name) =>
            ["-", "_", "."].some((separator) =>
              needle.startsWith(`${name.toLowerCase()}${separator}`),
            ),
          )),
    );
  if (known) {
    return catalogueName(known.id);
  }
  // Legacy ProgIds absent from the upstream catalogue remain recognizable.
  if (/^safariurl/i.test(id)) {
    return catalogueName("safari");
  }
  if (/^chromiumhtm/i.test(id)) {
    return catalogueName("chromium");
  }
  if (/^ie\.http/i.test(id)) {
    return "Internet Explorer";
  }
  return undefined;
}

/**
 * Names the user's default browser, or returns undefined when the platform
 * does not say. `run(program, args)` resolves to `{ code, stdout }`.
 */
export async function detectDefaultBrowser({
  platform = process.platform,
  run = captureOutput,
  verbose = false,
} = {}) {
  const query = defaultBrowserQuery(platform);
  try {
    const result = await run(query.program, query.args);
    if (result.code !== 0 && platform !== "darwin") {
      return undefined;
    }
    // Without any handler entry, macOS answers with an error and uses Safari.
    // command-stream captures stdout as a string-like object, not a string.
    const output = result.code === 0 ? String(result.stdout ?? "") : "";
    return browserName(defaultBrowserId(output, platform));
  } catch (error) {
    if (verbose) {
      console.error(`could not detect the default browser: ${error.message}`);
    }
    return undefined;
  }
}

async function captureOutput(program, args) {
  return exec(program, args, { capture: true, mirror: false, stdin: "ignore" });
}

/**
 * The exact argument vector that opens `url` in a chosen application:
 * `open -a <app>` on macOS, and the application itself elsewhere.
 */
export function openWithCommand(url, app, platform = process.platform) {
  if (!/^https?:\/\/\S+$/i.test(url)) {
    throw new Error(`refusing to open a non-web URL: ${url}`);
  }
  return platform === "darwin"
    ? { program: "open", args: ["-a", app, url] }
    : { program: app, args: [url] };
}

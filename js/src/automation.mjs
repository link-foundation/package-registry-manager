import { cp } from "node:fs/promises";
import path from "node:path";

import {
  attachViaExtension,
  EXTENSION_DIRECTORY,
  launchRealBrowser,
} from "browser-commander";

import { snapshotBrowser } from "./browser-options.mjs";
import { defaultBrowserProfile, ensureProfileIgnored } from "./profile.mjs";
import { IMPORT_CHOICES, migrationFor } from "./sign-in-import.mjs";

/** How long `--browser-attach extension` waits for the extension to connect. */
export const EXTENSION_TIMEOUT_MS = 300_000;

/** Where the companion extension is written for Chrome's "Load unpacked". */
export function relayExtensionDirectory() {
  return path.join(path.dirname(defaultBrowserProfile()), "relay-extension");
}

/**
 * The `launchRealBrowser` options for the automated browser: the dedicated
 * profile by default, optionally seeded from a real profile, or a temporary
 * snapshot of the user's own profile with `--browser-attach snapshot`. With
 * the `domains` import scope only the cookies of `domains`, the registry's
 * sign-in domains, are imported.
 */
export function launchOptions(browser, profile, verbose = false, domains = []) {
  const options = {
    engine: "playwright",
    channel: browser.channel,
    headless: false,
    verbose,
  };
  if (browser.executable) {
    options.executablePath = browser.executable;
  }
  if (browser.restrictions.length > 0) {
    options.restrictions = browser.restrictions;
  }
  if (Object.keys(browser.preferences).length > 0) {
    options.preferences = browser.preferences;
  }
  if (browser.attach?.mode === "snapshot") {
    options.attach = {
      mode: "snapshot",
      browser: snapshotBrowser(browser.channel),
      profile: browser.attach.profile ?? "Default",
    };
    return options;
  }
  options.userDataDir = profile;
  // `default` and `auto` are resolved to an installed browser before launch.
  if (browser.import && !IMPORT_CHOICES.includes(browser.import.browser)) {
    options.migrateFrom = migrationFor(
      browser.import,
      browser.importScope ?? "full",
      domains,
    );
  }
  return options;
}

/** The instructions printed while waiting for the companion extension. */
export function extensionInstructions(directory) {
  return [
    `Waiting up to ${EXTENSION_TIMEOUT_MS / 60_000} minutes for the Browser Commander extension in your own browser.`,
    'If it is not installed, open chrome://extensions, turn on Developer mode, click "Load unpacked", and choose:',
    `  ${directory}`,
  ].join("\n");
}

/**
 * Connects the browser that fills forms and returns a page with `goto`,
 * `evaluate`, `close`, and, for a launched profile, `clearCookies(domains)`.
 */
export async function connectAutomation({
  browser,
  profile,
  verbose = false,
  domains = [],
}) {
  if (browser.attach?.mode === "extension") {
    return connectExtension();
  }
  if (!browser.attach) {
    await ensureProfileIgnored(profile, { verbose });
  }
  const connection = await launchRealBrowser(
    launchOptions(browser, profile, verbose, domains),
  );
  return {
    goto: (url) => connection.page.goto(url),
    evaluate: (script) => connection.page.evaluate(script),
    close: () => connection.browser.close(),
    async clearCookies(names) {
      for (const name of names) {
        const escaped = name.replaceAll(".", "\\.");
        await connection.page
          .context()
          .clearCookies({ domain: new RegExp(`(?:^|\\.)${escaped}$`) });
      }
    },
  };
}

async function connectExtension() {
  const directory = relayExtensionDirectory();
  await cp(EXTENSION_DIRECTORY, directory, { recursive: true });
  const relay = await attachViaExtension({
    timeoutMs: EXTENSION_TIMEOUT_MS,
    onListening: () => console.log(extensionInstructions(directory)),
  });
  let session = null;
  return {
    async goto(url) {
      if (session) {
        await session.send("Page.navigate", { url });
      } else {
        const { tabId } = await relay.newTab(url);
        session = await relay.session(tabId);
      }
    },
    async evaluate(expression) {
      const { result, exceptionDetails } = await session.send(
        "Runtime.evaluate",
        { expression, returnByValue: true, awaitPromise: true },
      );
      if (exceptionDetails) {
        throw new Error(
          `the page script failed: ${exceptionDetails.exception?.description ?? exceptionDetails.text}`,
        );
      }
      return result.value;
    },
    async close() {
      await session?.detach().catch(() => {});
      await relay.close();
    },
  };
}

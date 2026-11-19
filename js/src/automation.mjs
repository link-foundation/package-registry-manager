import { cp } from "node:fs/promises";
import path from "node:path";

import {
  attachViaExtension,
  EXTENSION_DIRECTORY,
  launchRealBrowser,
  findBrowserSource,
} from "browser-commander";

import { connectWebDriverAutomation } from "./webdriver-automation.mjs";

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
export function launchOptions(
  browser,
  profile,
  verbose = false,
  domains = [],
  headless = false,
) {
  const options = {
    engine:
      findBrowserSource(browser.channel)?.controlProtocol === "cdp"
        ? "playwright"
        : "selenium",
    channel: browser.channel,
    headless,
    verbose,
  };
  if (browser.executable) {
    options.executablePath = browser.executable;
  }
  const restrictions = [...browser.restrictions];
  if (!headless && !restrictions.includes("start-maximized")) {
    restrictions.push("start-maximized");
  }
  if (restrictions.length > 0) {
    options.restrictions = restrictions;
  }
  const chromium =
    findBrowserSource(browser.channel)?.controlProtocol === "cdp";
  if (chromium) {
    options.disableFeatures = ["ChromeWhatsNewUI", "PromotionalTabs"];
    const preferences = JSON.parse(JSON.stringify(browser.preferences));
    preferences.browser ??= {};
    preferences.browser.split_view_drag_and_drop_enabled ??= false;
    options.preferences = preferences;
  } else if (Object.keys(browser.preferences).length > 0) {
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
 * Whether a context page is the page the caller is working with. Playwright
 * can hand back a different wrapper for the same tab, so compare the URL too;
 * closing the working page would abort the flow with “Target page, context or
 * browser has been closed”.
 */
export function isSamePage(candidate, page) {
  if (candidate === page) {
    return true;
  }
  try {
    const url = typeof page?.url === "function" ? page.url() : "";
    return Boolean(url) && candidate?.url?.() === url;
  } catch {
    return false;
  }
}

/**
 * Closes every tab in the automated context except the working page. A
 * persistent profile can restore earlier tabs, and each restored about:blank
 * would otherwise accumulate across runs.
 */
export async function closeExtraPages(page) {
  const pages = page?.context?.()?.pages?.() ?? [];
  let closed = 0;
  for (const candidate of pages) {
    if (isSamePage(candidate, page)) {
      continue;
    }
    try {
      await candidate.close();
      closed += 1;
    } catch {
      // A tab can close itself while this loop runs; keep the working page.
    }
  }
  return closed;
}

/**
 * Maximizes the visible Chromium window through CDP. A profile that saved a
 * small window placement ignores the launch switch, so set the bounds after
 * the browser is ready.
 */
export async function maximizeWindow(page) {
  const client = await page?.context?.()?.newCDPSession?.(page);
  if (!client) {
    return false;
  }
  try {
    const { windowId } = await client.send("Browser.getWindowForTarget");
    await client.send("Browser.setWindowBounds", {
      windowId,
      bounds: { windowState: "maximized" },
    });
    return true;
  } finally {
    await client.detach?.().catch(() => {});
  }
}

/**
 * Connects the browser that fills forms and returns a page with `goto`,
 * `evaluate`, `close`, and, for a launched profile, `clearCookies(domains)`.
 */
export async function connectAutomation(
  { browser, profile, verbose = false, domains = [], headless = false },
  dependencies = {},
) {
  if (browser.attach?.mode === "extension") {
    return connectExtension();
  }
  if (findBrowserSource(browser.channel)?.controlProtocol === "bidi") {
    return connectWebDriverAutomation(
      { browser, profile, verbose, domains, headless },
      dependencies,
    );
  }
  if (!browser.attach) {
    await ensureProfileIgnored(profile, { verbose });
  }
  const connection = await (
    dependencies.launchRealBrowser ?? launchRealBrowser
  )(launchOptions(browser, profile, verbose, domains, headless));
  await closeExtraPages(connection.page);
  if (!headless) {
    await maximizeWindow(connection.page).catch(() => {});
  }
  return {
    headless,
    goto: (url) => connection.page.goto(url),
    click: (selector) => connection.page.click(selector),
    pid: () => connection.pid?.(),
    evaluate: (script) => connection.page.evaluate(script),
    url: () => connection.page.url(),
    pages: () => connection.page.context().pages(),
    close: () => connection.close(),
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
  let currentUrl = "";
  const api = {
    async goto(url) {
      currentUrl = url;
      if (session) {
        await session.send("Page.navigate", { url });
      } else {
        const { tabId } = await relay.newTab(url);
        session = await relay.session(tabId);
      }
    },
    url: () => currentUrl,
    pages: () => [api],
    async click(selector) {
      await api.evaluate(`(() => {
        const element = document.querySelector(${JSON.stringify(selector)});
        if (!element) throw new Error('selector not found: ' + ${JSON.stringify(selector)});
        element.click();
      })()`);
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
      await session?.send("Page.close").catch(() => {});
      await session?.detach().catch(() => {});
      await relay.close();
    },
  };
  return api;
}

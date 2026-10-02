// Launches the automated profile the way `setup` does and prints the parity
// report: run with `xvfb-run -a node experiments/browser-parity-smoke.mjs`.
import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import {
  launchRealBrowser,
  measureParity,
} from "../js/node_modules/browser-commander/src/index.js";

import { launchOptions } from "../js/src/automation.mjs";
import { parseBrowserOptions } from "../js/src/browser-options.mjs";

const profile = await mkdtemp(path.join(os.tmpdir(), "prm-parity-"));
const session = await launchRealBrowser(
  launchOptions(parseBrowserOptions({ channel: "chrome", restrictions: process.argv.slice(2) }), profile, true),
);
try {
  await session.page.goto("about:blank");
  console.log("webdriver:", await session.page.evaluate("navigator.webdriver"));
  const report = await measureParity({ session });
  console.log(
    JSON.stringify(
      {
        commandLine: report.commandLine,
        unlisted: report.unlisted,
        ok: report.ok,
      },
      null,
      2,
    ),
  );
} finally {
  await session.browser.close();
  await rm(profile, { recursive: true, force: true });
}

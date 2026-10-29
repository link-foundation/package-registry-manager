import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  KEEP_SESSION_COOKIES_SWITCH,
  launchRealBrowser,
  measureParity,
} from "browser-commander";

import { launchOptions } from "../src/automation.mjs";
import { parseBrowserOptions } from "../src/browser-options.mjs";

// Launches a real browser, so it runs only when PRM_BROWSER_SMOKE=1 (CI runs
// it under xvfb-run with Google Chrome installed).
const smoke = process.env.PRM_BROWSER_SMOKE === "1";

test(
  "launches the automated profile like a browser started by hand",
  {
    skip: !smoke && "set PRM_BROWSER_SMOKE=1 to launch Chrome",
    timeout: 180_000,
  },
  async () => {
    const profile = await mkdtemp(path.join(os.tmpdir(), "prm-parity-"));
    const session = await launchRealBrowser(
      launchOptions(parseBrowserOptions(), profile),
    );
    try {
      await session.page.goto("about:blank");
      assert.equal(await session.page.evaluate("navigator.webdriver"), false);
      // browser-commander adds the one extra switch for a persistent
      // profile so its session cookies, and with them sign-ins, survive a
      // restart. PRM wants that, so it counts as requested.
      const report = await measureParity({
        session,
        args: [KEEP_SESSION_COOKIES_SWITCH],
      });
      assert.deepEqual(
        report.commandLine.extra,
        [KEEP_SESSION_COOKIES_SWITCH],
        report.commandLine.launched,
      );
      assert.deepEqual(report.unlisted, []);
      assert.equal(report.ok, true);
    } finally {
      await session.browser.close();
      await rm(profile, { recursive: true, force: true });
    }
  },
);

test(
  "headless API sessions retain saved cookies across profile reuse (#59)",
  { skip: !smoke, timeout: 180_000 },
  async () => {
    const { createServer } = await import("node:http");
    const { connectAutomation } = await import("../src/automation.mjs");
    const server = createServer((req, res) => {
      if (req.url === "/login") {
        res.setHeader(
          "Set-Cookie",
          "prm_session=saved; Path=/; Max-Age=300; HttpOnly",
        );
      }
      res.end(req.headers.cookie ?? "no session");
    });
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
    const base = `http://127.0.0.1:${server.address().port}`;
    const profile = await mkdtemp(path.join(os.tmpdir(), "prm-quiet-smoke-"));
    let session;
    try {
      session = await connectAutomation({
        browser: parseBrowserOptions(),
        profile,
        domains: ["127.0.0.1"],
      });
      await session.goto(`${base}/login`);
      await session.close();
      session = null;
      session = await connectAutomation({
        browser: parseBrowserOptions(),
        profile,
        domains: ["127.0.0.1"],
        headless: true,
      });
      await session.goto(`${base}/api`);
      assert.equal(session.headless, true);
      assert.equal(
        await session.evaluate("document.body.innerText"),
        "prm_session=saved",
      );
    } finally {
      await session?.close();
      await rm(profile, { recursive: true, force: true });
      await new Promise((resolve) => server.close(resolve));
    }
  },
);

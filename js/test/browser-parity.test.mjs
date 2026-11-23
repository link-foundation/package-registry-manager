import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
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
      // Allow PRM's window and startup settings and browser-commander's
      // session cookie persistence switch.
      const requestedSwitches = [
        "--disable-features=ChromeWhatsNewUI,PromotionalTabs",
        KEEP_SESSION_COOKIES_SWITCH,
        "--start-maximized",
      ];
      const report = await measureParity({
        session,
        args: requestedSwitches,
      });
      assert.deepEqual(
        report.commandLine.extra,
        requestedSwitches,
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

test(
  "reattaches to the dedicated Chromium tab and uses trusted npm approval clicks",
  { skip: !smoke, timeout: 90_000 },
  async (t) => {
    const { connectAutomation } = await import("../src/automation.mjs");
    const { watchNpmApproval } = await import("../src/npm-approval.mjs");
    const profile = await mkdtemp(path.join(os.tmpdir(), "prm-npm-cdp-"));
    t.after(() => rm(profile, { recursive: true, force: true }));
    const options = { browser: parseBrowserOptions(), profile, headless: true };
    const original = await connectAutomation(options);
    try {
      assert.equal(original.reused, false);
      await original.evaluate("globalThis.prmReuse = 42");
      const attached = await connectAutomation(options, {
        launchRealBrowser: () =>
          assert.fail("the running browser must be reused"),
      });
      try {
        assert.equal(attached.reused, true);
        assert.equal(await attached.evaluate("globalThis.prmReuse"), 42);
        const context = attached.pages()[0].context();
        await context.route("https://www.npmjs.com/auth/cli/*", (route) =>
          route.fulfill({
            contentType: "text/html",
            body: `<meta charset="utf-8">
              <main style="font: 16px system-ui; padding: 20px 24px; max-width: 540px">
              <h1>Security key approval</h1>
              <label><input type="checkbox" id="cooldownOptin_didOptForCooldown">
              Skip two-factor checks for the next five minutes</label>
              <button>Use security key</button>
              <p id="status">Waiting for approval</p>
              </main>
              <script>
                const box = document.querySelector('input');
                const button = document.querySelector('button');
                box.addEventListener('click', (event) => {
                  if (!event.isTrusted) event.preventDefault();
                });
                button.addEventListener('click', (event) => {
                  if (event.isTrusted && box.checked) {
                    document.querySelector('#status').textContent = 'Something went wrong. Please try again later.';
                  }
                });
              </script>`,
          }),
        );
        await attached.goto("https://www.npmjs.com/auth/cli/first");
        await attached.evaluate("document.querySelector('input').click()");
        assert.equal(
          await attached.evaluate("document.querySelector('input').checked"),
          false,
        );
        const capture = async (state) => {
          if (process.env.PRM_NPM_APPROVAL_SCREENSHOTS === "1") {
            await attached
              .pages()[0]
              .locator("main")
              .screenshot({
                path: path.join(
                  import.meta.dirname,
                  "../../docs/screenshots",
                  `npm-approval-${state}.png`,
                ),
              });
          }
        };
        await capture("before");
        const failure = await watchNpmApproval(attached, {
          timeoutMs: 5_000,
          pollIntervalMs: 10,
          log: () => {},
        });
        assert.match(failure, /Something went wrong/);
        assert.equal(
          await attached.evaluate("document.querySelector('input').checked"),
          true,
        );
        await capture("after");
        assert.equal(attached.pages().length, 1);
        await attached.goto("https://www.npmjs.com/auth/cli/fresh");
        assert.equal(attached.pages().length, 1);
        assert.match(
          await original.evaluate("location.href"),
          /\/auth\/cli\/fresh$/,
        );
      } finally {
        await attached.close();
      }
      assert.match(
        await original.evaluate("location.href"),
        /\/auth\/cli\/fresh$/,
      );
      await writeFile(
        path.join(profile, ".prm-cdp-endpoint"),
        "ws://127.0.0.1:1/devtools/browser/stale\n",
      );
      const recovered = await connectAutomation(options, {
        launchRealBrowser: () =>
          assert.fail("recover the active browser from its launch arguments"),
      });
      try {
        assert.equal(recovered.reused, true);
        assert.match(recovered.url(), /\/auth\/cli\/fresh$/);
      } finally {
        await recovered.close();
      }
    } finally {
      await original.close();
    }
  },
);

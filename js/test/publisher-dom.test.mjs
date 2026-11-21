import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { chromium } from "playwright";
import { SetupSession } from "../src/setup.mjs";
import {
  loginDeviceScript,
  prepareLoginDevice,
  publisherConfirmationScript,
  signInScript,
} from "../src/publisher-form.mjs";

test(
  "PyPI existing-project form submits without a pending-project field (#50)",
  {
    skip:
      process.env.PRM_BROWSER_SMOKE !== "1" &&
      "set PRM_BROWSER_SMOKE=1 with Chrome installed",
    timeout: 30_000,
  },
  async () => {
    const browser = await chromium.launch({
      channel: "chrome",
      headless: true,
    });
    try {
      const page = await browser.newPage();
      const html = (
        await readFile(
          new URL("../../experiments/issue-54/publisher.html", import.meta.url),
          "utf8",
        )
      )
        .replaceAll("pending-github-publisher-form", "github-publisher-form")
        .replace(
          '<label for="project_name">PyPI Project Name</label><input id="project_name" name="project_name" required>',
          "",
        );
      await page.setContent(html);
      const config = {
        project: "prm-example",
        organization: "acme",
        repository: "example",
        workflow: "release.yml",
      };
      const session = new SetupSession(
        {
          registry: "pypi",
          mode: "attach",
          package: { name: config.project },
          trusted_publisher: config,
          steps: [],
        },
        {
          automation: page,
          waitTimeoutMs: 5000,
          pollIntervalMs: 10,
          prompt: () => {
            throw Error("must not read stdin");
          },
        },
      );
      await session.prefill();
      assert.equal(
        await page.locator("#publishers tr td").first().textContent(),
        "GitHub",
      );
      assert.ok(!session.conditions.has("trust-missing"));
    } finally {
      await browser.close();
    }
  },
);

test(
  "PyPI delayed login submits only the GitHub form and confirms the saved identity (#50)",
  {
    skip:
      process.env.PRM_BROWSER_SMOKE !== "1" &&
      "set PRM_BROWSER_SMOKE=1 with Chrome installed",
    timeout: 30_000,
  },
  async () => {
    const browser = await chromium.launch({
      channel: "chrome",
      headless: true,
    });
    try {
      const page = await browser.newPage();
      await page.setContent("<h1>Sign in to PyPI</h1>");
      const html = await readFile(
        new URL("../../experiments/issue-54/publisher.html", import.meta.url),
        "utf8",
      );
      const config = {
        project: "prm-example",
        organization: "acme",
        repository: "example",
        workflow: "release.yml",
      };
      const session = new SetupSession(
        {
          registry: "pypi",
          package: { name: config.project },
          trusted_publisher: config,
          steps: [],
        },
        {
          yes: true,
          pollIntervalMs: 10,
          waitTimeoutMs: 5000,
          prompt: () => {
            throw Error("must not read stdin");
          },
          automation: page,
        },
      );
      const prefill = session.prefill();
      await new Promise((resolve) => setTimeout(resolve, 100));
      await page.setContent(html);
      await prefill;
      assert.equal(await page.locator("#google_project").inputValue(), "");
      assert.equal(await page.locator("#publishers tr").count(), 1);
      assert.ok(!session.conditions.has("trust-missing"));
      assert.equal(
        (
          await page.evaluate(
            publisherConfirmationScript({
              ...config,
              organization: "wrong-owner",
            }),
          )
        ).confirmed,
        false,
      );
      await page.locator("#pending-github-publisher-form").evaluate((form) => {
        const error = form.ownerDocument.createElement("ul");
        error.className = "form-errors";
        error.textContent = "Project already exists";
        form.append(error);
      });
      assert.match(
        (await page.evaluate(publisherConfirmationScript(config))).error,
        /Project already exists/,
      );
    } finally {
      await browser.close();
    }
  },
);

test(
  "PyPI prefilled sign-in is submitted once without reading credentials",
  {
    skip:
      process.env.PRM_BROWSER_SMOKE !== "1" &&
      "set PRM_BROWSER_SMOKE=1 with Chrome installed",
    timeout: 30_000,
  },
  async () => {
    const browser = await chromium.launch({
      channel: "chrome",
      headless: true,
    });
    const form = (withValues) => `<!doctype html>
      <form action="/account/login/" method="post">
        <input name="username" autocomplete="username" ${withValues ? 'value="alice"' : ""}>
        <input name="password" type="password" ${withValues ? 'value="not-a-real-password"' : ""}>
        <input type="submit" value="Log in" disabled>
      </form>
      <script>
        globalThis.window.submits = 0;
        document.querySelector("form").addEventListener("submit", (event) => {
          event.preventDefault();
          globalThis.window.submits += 1;
        });
        for (const input of document.querySelectorAll("input")) {
          input.addEventListener("input", () => {
            document.querySelector("input[type=submit]").disabled =
              !(document.querySelector('input[name="username"]').value &&
                document.querySelector('input[type="password"]').value);
          });
        }
      </script>`;
    try {
      const page = await browser.newPage();
      await page.route("https://pypi.example/prefilled", (route) =>
        route.fulfill({ contentType: "text/html", body: form(true) }),
      );
      await page.goto("https://pypi.example/prefilled");
      const first = await page.evaluate(signInScript());
      assert.equal(first.present, true);
      assert.equal(first.prefilled, true);
      assert.equal(first.submitted, true);
      assert.equal("password" in first, false);
      assert.equal(await page.evaluate(() => globalThis.window.submits), 1);
      const second = await page.evaluate(signInScript());
      assert.equal(second.submitted, false);
      assert.equal(second.alreadySubmitted, true);
      assert.equal(second.failed, false);
      assert.equal(await page.evaluate(() => globalThis.window.submits), 1);
      await page.evaluate(() => {
        const error = globalThis.document.createElement("ul");
        error.className = "form-errors";
        error.textContent = "Invalid login attempt.";
        globalThis.document.body.append(error);
      });
      assert.equal((await page.evaluate(signInScript())).failed, true);
      await page.evaluate(() => {
        globalThis.document.querySelector(".form-errors").textContent =
          "Invalid or expired two factor login.";
      });
      assert.equal((await page.evaluate(signInScript())).failed, true);

      const empty = await browser.newPage();
      await empty.route("https://pypi.example/empty", (route) =>
        route.fulfill({ contentType: "text/html", body: form(false) }),
      );
      await empty.goto("https://pypi.example/empty");
      const notPrefilled = await empty.evaluate(signInScript());
      assert.equal(notPrefilled.present, true);
      assert.equal(notPrefilled.prefilled, false);
      assert.equal(notPrefilled.submitted, false);
      assert.equal(await empty.evaluate(() => globalThis.window.submits), 0);
    } finally {
      await browser.close();
    }
  },
);

test(
  "PyPI 2FA checks remember-device and finds confirmation in another tab",
  {
    skip:
      process.env.PRM_BROWSER_SMOKE !== "1" &&
      "set PRM_BROWSER_SMOKE=1 with Chrome installed",
    timeout: 30_000,
  },
  async () => {
    const browser = await chromium.launch({
      channel: "chrome",
      headless: true,
    });
    try {
      const context = await browser.newContext();
      const page = await context.newPage();
      await page.setContent(`
        <form id="totp-auth-form">
          <label>
            <input type="checkbox" name="remember_device" value="true">
            Remember this device for 30 days
          </label>
        </form>
      `);
      const state = {};
      await prepareLoginDevice(page, { state });
      assert.equal(
        await page.locator('input[name="remember_device"]').isChecked(),
        true,
      );
      assert.equal(state.remembered, true);

      const confirmation = await context.newPage();
      await confirmation.route("https://pypi.example/confirmed", (route) =>
        route.fulfill({
          contentType: "text/html",
          body: "Your login has been confirmed and this device is now recognized.",
        }),
      );
      await confirmation.goto("https://pypi.example/confirmed");
      assert.equal(
        (await confirmation.evaluate(loginDeviceScript())).confirmed,
        true,
      );
      await prepareLoginDevice(page, { state });
      assert.equal(state.confirmed, true);
      assert.equal(confirmation.isClosed(), true);
    } finally {
      await browser.close();
    }
  },
);

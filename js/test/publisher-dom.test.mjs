import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { chromium } from "playwright";
import { SetupSession } from "../src/setup.mjs";
import { publisherConfirmationScript } from "../src/publisher-form.mjs";

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

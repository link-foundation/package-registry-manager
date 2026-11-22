import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { withFreshLinks } from "../src/approvals.mjs";
import { runInteractive } from "../src/auth-urls.mjs";
import { npmApprovalState, watchNpmApproval } from "../src/npm-approval.mjs";

test(
  "npm uses native clicks, checks cooldown readback and retries the command",
  { timeout: 10_000 },
  async (t) => {
    const cwd = await mkdtemp(path.join(os.tmpdir(), "prm-npm-approval-"));
    t.after(() => rm(cwd, { recursive: true, force: true }));
    let attempt = 0;
    const links = [];
    const messages = [];
    const clicks = [];
    const result = await withFreshLinks(
      () => {
        const current = ++attempt;
        return runInteractive(
          {
            program: process.execPath,
            args: [
              "-e",
              `
          const fs = require('node:fs');
          console.log('Authenticate your account at:');
          console.log('https://www.npmjs.com/auth/cli/${current}');
          if (${current} === 2) {
            console.error('npm error 404 Not Found - GET https://registry.npmjs.org/-/v1/done?authId=fake');
            process.exitCode = 1;
          } else {
            const timer = setInterval(() => {
              if (fs.existsSync('approved')) {
                console.log('Trust configuration created successfully');
                clearInterval(timer);
              }
            }, 10);
            setTimeout(() => {
              if (!fs.existsSync('approved')) process.exitCode = 1;
              clearInterval(timer);
            }, 2_000).unref();
          }
        `,
            ],
          },
          {
            cwd,
            mirror: false,
            watchApproval: true,
            trustCreate: true,
            watchStderr: true,
            async onUrl(url, kind, signal) {
              assert.equal(kind, "approve");
              links.push(url);
              if (current === 2) {
                return undefined;
              }
              let cooldown = false;
              let boxClicks = 0;
              let pollsAfterKey = 0;
              let pressed = false;
              const page = {
                async evaluate(script) {
                  assert.equal(script, npmApprovalState);
                  if (current === 1 && pressed && ++pollsAfterKey === 3) {
                    return {
                      failure:
                        "the approval page reported Something went wrong",
                    };
                  }
                  return { cooldown, key: true };
                },
                async click(selector) {
                  clicks.push([current, selector]);
                  if (selector.startsWith("#")) {
                    // The first native click does not survive a page re-render.
                    cooldown = ++boxClicks === 2;
                  } else {
                    assert.equal(cooldown, true);
                    assert.equal(pressed, false);
                    pressed = true;
                    if (current === 3) {
                      await writeFile(path.join(cwd, "approved"), "");
                    }
                  }
                },
              };
              return watchNpmApproval(page, {
                signal,
                pollIntervalMs: 5,
                log: () => {},
              });
            },
          },
        );
      },
      { log: (message) => messages.push(message) },
    );

    assert.equal(result.code, 0);
    assert.equal(result.trustStored, true);
    assert.equal(attempt, 3);
    assert.equal(new Set(links).size, 3);
    assert.match(messages[0], /Something went wrong.*fresh one/);
    assert.match(messages[1], /approval session ended.*fresh one/);
    for (const number of [1, 3]) {
      assert.deepEqual(
        clicks
          .filter(([current]) => current === number)
          .map(([, selector]) => selector),
        [
          "#cooldownOptin_didOptForCooldown",
          "#cooldownOptin_didOptForCooldown",
          "[data-prm-npm-security-key]",
        ],
      );
    }
  },
);

test("a nonzero command exit after a link retries and a zero exit stops", async () => {
  let count = 0;
  const result = await withFreshLinks(
    async () => ({ code: ++count === 1 ? 17 : 0, approvalLink: true }),
    { log: () => {} },
  );
  assert.equal(result.code, 0);
  assert.equal(count, 2);
});

test("browser failures after npm has stored the trusted publisher cannot trigger a retry", async () => {
  let count = 0;
  const result = await withFreshLinks(async () => {
    count += 1;
    return {
      code: 1,
      approvalLink: true,
      approvalPageFailure: "Something went wrong",
      trustStored: true,
    };
  });
  assert.equal(result.trustStored, true);
  assert.equal(count, 1);
});

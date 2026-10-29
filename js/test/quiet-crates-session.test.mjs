import assert from "node:assert/strict";
import { test } from "node:test";
import { launchOptions } from "../src/automation.mjs";
import { parseBrowserOptions } from "../src/browser-options.mjs";
import { runCratesApiStep } from "../src/crates-api.mjs";

test("crates API launch can reuse a saved profile headlessly (#59)", () => {
  assert.equal(
    launchOptions(
      parseBrowserOptions(),
      "/tmp/prm-quiet",
      false,
      ["crates.io"],
      true,
    ).headless,
    true,
  );
});

test("saved valid crates session stays quiet; missing login escalates visibly (#59)", async () => {
  for (const [valid, emailVerified] of [
    [true, true],
    [false, true],
    [true, false],
  ]) {
    let visible = 0;
    const launches = [];
    const page = {
      goto: async () => {},
      evaluate: async () => ({
        status: 200,
        body: { user: { login: "maintainer", email_verified: emailVerified } },
      }),
    };
    let reads = 0;
    const initial = {
      ...page,
      evaluate: async () =>
        ++reads === 1 && !valid ? { status: 401, body: {} } : page.evaluate(),
    };
    const session = {
      options: { yes: true },
      plan: { registry: "crates-io", package: { name: "demo" } },
      conditions: new Set(),
      automatedPage: async (...args) => {
        launches.push(args[2]);
        return initial;
      },
      visiblePage: async () => {
        visible++;
        return page;
      },
      prompt: async () => "skip",
    };
    if (emailVerified) {
      await runCratesApiStep(session, { id: "crates-sign-in" });
    } else {
      await assert.rejects(
        runCratesApiStep(session, { id: "crates-sign-in" }),
        /verified email/,
      );
    }
    assert.equal(launches[0], true);
    assert.equal(visible, valid && emailVerified ? 0 : 1);
  }
});

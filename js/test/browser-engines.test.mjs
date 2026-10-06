import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { BROWSER_IDS, findBrowserSource } from "browser-commander";

import { connectAutomation, launchOptions } from "../src/automation.mjs";
import { defaultBrowserProfile } from "../src/profile.mjs";
import {
  assertDedicatedProfile,
  launchChannels,
  resolveExecutable,
} from "../src/browser-catalogue.mjs";
import { parseBrowserOptions } from "../src/browser-options.mjs";
import { pageFetchScript } from "../src/crates-api.mjs";

for (const id of BROWSER_IDS) {
  const source = findBrowserSource(id);
  if (!source.controlProtocol) {
    continue;
  }
  test(`accepts launchable ${id} and forwards its engine`, () => {
    for (const channel of [id, ...(source.aliases ?? [])]) {
      assert.ok(launchChannels().includes(channel));
      const options = launchOptions(
        parseBrowserOptions({ channel }),
        "/profile",
      );
      assert.equal(options.channel, channel);
      assert.equal(
        options.engine,
        source.controlProtocol === "cdp" ? "playwright" : "selenium",
      );
    }
  });
}

test("BiDi launcher fills pages, imports sign-in cookies and clears only registry domains", async () => {
  const profile = await mkdtemp(path.join(os.tmpdir(), "prm-bidi-"));
  const calls = [];
  const cookies = [
    {
      name: "session",
      value: "private",
      domain: ".npmjs.com",
      path: "/",
      httpOnly: true,
      secure: true,
      sameSite: "Lax",
      expires: -1,
    },
    { name: "other", value: "private", domain: "unrelated.test", path: "/" },
    { name: "lookalike", value: "private", domain: "evilnpmjs.com", path: "/" },
  ];
  try {
    const page = await connectAutomation(
      {
        browser: parseBrowserOptions({
          channel: "librewolf",
          importFrom: "chrome:Profile 1",
          importScope: "domains",
        }),
        profile,
        domains: ["npmjs.com"],
      },
      {
        resolveExecutable: async (options) => {
          assert.equal(options.channel, "librewolf");
          return "/stub/librewolf";
        },
        readCookies: async (options) => {
          assert.equal(options.browser, "chrome");
          assert.equal(options.profile, "Profile 1");
          assert.equal(options.domainFilter, "npmjs.com");
          assert.equal(options.cache, false);
          return [cookies[0]];
        },
        launchWebDriver: async (options) => {
          assert.equal(options.browser, "firefox");
          assert.equal(options.bidi, true);
          assert.equal(options.executablePath, "/stub/librewolf");
          assert.equal(options.userDataDir, profile);
          return {
            close: async () => calls.push(["close"]),
            page: {
              goto: async (url) => calls.push(["goto", url]),
              evaluate: async (script) => {
                calls.push(["evaluate", script]);
                return { status: 200, body: {} };
              },
              bidiCommand: async (method, params) => {
                calls.push([method, params]);
                return { cookies };
              },
            },
          };
        },
        launchRealBrowser: async () => assert.fail("BiDi must use WebDriver"),
      },
    );
    await page.goto("https://npmjs.com/");
    const script = pageFetchScript("GET", "/api/v1/me");
    assert.deepEqual(await page.evaluate(script), { status: 200, body: {} });
    await page.clearCookies(["npmjs.com"]);
    await page.close();
    assert.equal(
      calls.filter(([method]) => method === "storage.setCookie").length,
      1,
    );
    const deleted = calls.filter(
      ([method]) => method === "storage.deleteCookies",
    );
    assert.equal(deleted.length, 1);
    assert.equal(deleted[0][1].filter.domain, ".npmjs.com");
    assert.ok(
      calls.some(
        ([method, value]) => method === "evaluate" && value === script,
      ),
    );
    assert.deepEqual(calls.at(-1), ["close"]);
  } finally {
    await rm(profile, { recursive: true, force: true });
  }
});

test("Firefox imports default to domains and unsupported capabilities explain the limit", () => {
  assert.equal(
    parseBrowserOptions({ channel: "firefox", importFrom: "chrome" })
      .importScope,
    "domains",
  );
  for (const extra of [
    { attach: "snapshot" },
    { restrictions: ["no-extensions"] },
    { preferences: ["a=1"] },
    { importFrom: "chrome", importScope: "full" },
  ]) {
    assert.throws(
      () => parseBrowserOptions({ channel: "firefox", ...extra }),
      /unavailable for firefox/,
    );
  }
});

test("CDP uses the public close function and keeps page context cookie cleanup", async () => {
  const profile = await mkdtemp(path.join(os.tmpdir(), "prm-cdp-"));
  let closed = false;
  let domain;
  try {
    const page = await connectAutomation(
      { browser: parseBrowserOptions(), profile },
      {
        launchRealBrowser: async (options) => {
          assert.equal(options.engine, "playwright");
          return {
            close: async () => {
              closed = true;
            },
            page: {
              context: () => ({
                clearCookies: async (filter) => {
                  domain = filter.domain;
                },
              }),
            },
          };
        },
      },
    );
    await page.clearCookies(["npmjs.com"]);
    assert.ok(domain.test(".npmjs.com"));
    assert.ok(!domain.test("evilnpmjs.com"));
    await page.close();
    assert.ok(closed);
  } finally {
    await rm(profile, { recursive: true, force: true });
  }
});

test("default profiles keep Firefox variants separate from Chromium", () => {
  const chrome = defaultBrowserProfile();
  assert.equal(defaultBrowserProfile({ channel: "vivaldi" }), chrome);
  assert.equal(
    defaultBrowserProfile({ channel: "firefox" }),
    `${chrome}-firefox`,
  );
  assert.equal(
    defaultBrowserProfile({ channel: "librewolf" }),
    `${chrome}-librewolf`,
  );
});

test("refuses a default Firefox profile before opening it", () => {
  assert.throws(
    () =>
      assertDedicatedProfile(path.resolve("fake-home/.mozilla/firefox/abc"), {
        homeDir: path.resolve("fake-home"),
        environment: {},
      }),
    /dedicated profile/,
  );
});

test("selected Firefox executable resolves Windows catalogue paths without reading profiles", async () => {
  const executable = await resolveExecutable(
    { channel: "librewolf" },
    {
      platform: "win32",
      environment: { ProgramFiles: "D:\\Programs" },
      homeDir: "C:\\Users\\test",
      access: async (candidate) => {
        if (candidate !== "D:\\Programs\\LibreWolf\\librewolf.exe") {
          throw new Error("not installed");
        }
      },
    },
  );
  assert.equal(executable, "D:\\Programs\\LibreWolf\\librewolf.exe");
});

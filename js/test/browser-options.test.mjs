import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";

import {
  extensionInstructions,
  launchOptions,
  relayExtensionDirectory,
} from "../src/automation.mjs";
import {
  automatedDescription,
  parseBrowserOptions,
  restrictionNames,
  snapshotBrowser,
} from "../src/browser-options.mjs";
import { defaultBrowserProfile } from "../src/profile.mjs";

const PROFILE = path.resolve("automation-profile");

test("keeps a fresh dedicated profile by default", () => {
  const browser = parseBrowserOptions();
  assert.deepEqual(browser, {
    channel: "chrome",
    executable: null,
    import: null,
    importScope: "domains",
    attach: null,
    preferences: {},
    restrictions: [],
  });
  assert.deepEqual(launchOptions(browser, PROFILE), {
    engine: "playwright",
    channel: "chrome",
    headless: false,
    verbose: false,
    userDataDir: PROFILE,
  });
});

test("passes the executable, preferences, and restrictions to the launch", () => {
  const browser = parseBrowserOptions({
    channel: "msedge",
    executable: "bin/edge",
    preferences: [
      "intl.accept_languages=en-US",
      "browser.show_home_button=true",
      "session.restore_on_startup=4",
      'download.default_directory="/tmp/x=y"',
    ],
    restrictions: ["no-extensions", "legacy-defaults", "no-extensions"],
  });
  assert.deepEqual(launchOptions(browser, PROFILE, true), {
    engine: "playwright",
    channel: "msedge",
    headless: false,
    verbose: true,
    executablePath: path.resolve("bin/edge"),
    restrictions: ["no-extensions", "legacy-defaults"],
    preferences: {
      intl: { accept_languages: "en-US" },
      browser: { show_home_button: true },
      session: { restore_on_startup: 4 },
      download: { default_directory: "/tmp/x=y" },
    },
    userDataDir: PROFILE,
  });
});

test("imports a real profile into the dedicated profile on request", () => {
  for (const [spec, migrateFrom] of [
    ["chrome", { browser: "chrome" }],
    ["edge:Profile 1", { browser: "edge", profile: "Profile 1" }],
    ["brave:Default", { browser: "brave", profile: "Default" }],
    [
      "firefox:abc.default-release",
      { browser: "firefox", profile: "abc.default-release" },
    ],
  ]) {
    const options = launchOptions(
      parseBrowserOptions({ importFrom: spec }),
      PROFILE,
    );
    assert.deepEqual(options.migrateFrom, migrateFrom, spec);
    assert.equal(options.userDataDir, PROFILE, spec);
  }
});

test("attaches to a snapshot of the user's own profile", () => {
  for (const [channel, attach, expected] of [
    ["chrome", "snapshot", { browser: "chrome", profile: "Default" }],
    [
      "msedge-beta",
      "snapshot:Profile 2",
      { browser: "edge-beta", profile: "Profile 2" },
    ],
    ["brave", "snapshot", { browser: "brave", profile: "Default" }],
  ]) {
    const options = launchOptions(
      parseBrowserOptions({ channel, attach }),
      PROFILE,
    );
    assert.deepEqual(options.attach, { mode: "snapshot", ...expected });
    assert.equal(options.userDataDir, undefined, "a snapshot is temporary");
  }
  assert.equal(snapshotBrowser("chromium"), "chromium");
  assert.equal(snapshotBrowser("msedge-canary"), "edge-canary");
});

test("rejects invalid or conflicting browser options", () => {
  for (const [options, message] of [
    [{ importFrom: "netscape" }, /--browser-import must be/],
    [{ importFrom: "chrome:" }, /--browser-import must be/],
    [{ attach: "remote" }, /--browser-attach must be/],
    [{ attach: "snapshot:" }, /--browser-attach must be/],
    [{ preferences: ["novalue"] }, /--browser-pref must be key=value/],
    [{ preferences: ["a..b=1"] }, /--browser-pref must be key=value/],
    [{ preferences: ["=1"] }, /--browser-pref must be key=value/],
    [{ restrictions: ["nope"] }, /unknown --browser-restriction 'nope'/],
    [
      { attach: "snapshot", importFrom: "chrome" },
      /cannot be combined with --browser-import/,
    ],
    [
      { attach: "snapshot", profileGiven: true },
      /cannot be combined with --browser-profile/,
    ],
    [
      { attach: "extension", executable: "chrome" },
      /extension cannot be combined/,
    ],
    [
      { attach: "extension", restrictions: ["no-sync"] },
      /extension cannot be combined/,
    ],
    [
      { attach: "extension", preferences: ["a=1"] },
      /extension cannot be combined/,
    ],
  ]) {
    assert.throws(() => parseBrowserOptions(options), message);
  }
});

test("accepts every restriction and preset of the shared catalogue", () => {
  const names = restrictionNames();
  assert.ok(names.includes("no-extensions"));
  assert.ok(names.includes("legacy-defaults"));
  assert.deepEqual(
    parseBrowserOptions({ restrictions: names }).restrictions,
    names,
  );
});

test("later preferences override earlier ones", () => {
  const { preferences } = parseBrowserOptions({
    preferences: ["a=1", "a.b=2", "a.c=[1]", "d=null", "e=text", "f=1e999"],
  });
  assert.deepEqual(preferences, {
    a: { b: 2, c: [1] },
    d: null,
    e: "text",
    f: "1e999",
  });
});

test("describes where forms are filled", () => {
  assert.equal(automatedDescription(), "the automated chrome profile");
  assert.equal(
    automatedDescription({
      channel: "brave",
      profile: "/p",
      import: { browser: "firefox", profile: null },
    }),
    "the automated brave profile at /p with data imported from firefox",
  );
  assert.equal(
    automatedDescription({ attach: { mode: "snapshot", profile: null } }),
    "a temporary snapshot of your chrome profile Default",
  );
  assert.equal(
    automatedDescription({ attach: { mode: "extension" } }),
    "your own browser through the Browser Commander extension",
  );
});

test("writes the companion extension next to the dedicated profile", () => {
  const directory = relayExtensionDirectory();
  assert.equal(path.dirname(directory), path.dirname(defaultBrowserProfile()));
  const instructions = extensionInstructions(directory);
  assert.match(instructions, /chrome:\/\/extensions/);
  assert.match(instructions, /Load unpacked/);
  assert.ok(instructions.endsWith(`  ${directory}`));
  assert.equal(
    extensionInstructions("/x"),
    "Waiting up to 5 minutes for the Browser Commander extension in your own browser.\n" +
      'If it is not installed, open chrome://extensions, turn on Developer mode, click "Load unpacked", and choose:\n' +
      "  /x",
  );
});

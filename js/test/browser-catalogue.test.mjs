import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { BROWSER_IDS, findBrowserSource } from "browser-commander";

import {
  installedBrowsers,
  launchChannels,
} from "../src/browser-catalogue.mjs";
import { launchOptions } from "../src/automation.mjs";
import {
  parseBrowserOptions,
  parseImport,
  snapshotBrowser,
} from "../src/browser-options.mjs";
import { browserName } from "../src/default-browser.mjs";
import { importSources, sourceId, sourceName } from "../src/sign-in-import.mjs";

test("accepts every upstream import id with and without a profile", () => {
  assert.deepEqual(importSources(), [...BROWSER_IDS]);
  for (const id of BROWSER_IDS) {
    assert.deepEqual(parseImport(id), { browser: id, profile: null });
    assert.deepEqual(parseImport(`${id}:Profile 1`), {
      browser: id,
      profile: "Profile 1",
    });
    for (const alias of findBrowserSource(id).aliases ?? []) {
      assert.equal(parseImport(alias).browser, id);
    }
  }
});

test("names every catalogue default identifier and round-trips import names", () => {
  for (const id of BROWSER_IDS) {
    const name = sourceName(id);
    assert.equal(sourceId(name), id, name);
    for (const identifiers of Object.values(
      findBrowserSource(id).default ?? {},
    )) {
      for (const identifier of identifiers) {
        assert.equal(browserName(identifier), name, identifier);
        assert.equal(browserName(identifier.toUpperCase()), name, identifier);
      }
    }
  }
  for (const [id, name] of Object.entries({
    yandex: "Yandex",
    "opera-gx": "Opera GX",
    librewolf: "LibreWolf",
    waterfox: "Waterfox",
    zen: "Zen",
    floorp: "Floorp",
  })) {
    assert.equal(sourceName(id), name);
  }
});

test("preserves legacy and packaged default-browser identifiers", () => {
  for (const [identifier, name] of [
    ["FirefoxURL308046B0AF4A39CB", "Firefox"],
    ["VivaldiHTM.123", "Vivaldi"],
    ["ChromeHTML-hash", "Google Chrome"],
    ["brave-browser_brave.desktop", "Brave"],
    ["SafariURL", "Safari"],
    ["ChromiumHTM.123", "Chromium"],
    ["IE.HTTP", "Internet Explorer"],
  ]) {
    assert.equal(browserName(identifier), name, identifier);
  }
});

test("lists all imports and supported launch channels in CLI help", () => {
  const help = execFileSync(
    process.execPath,
    [fileURLToPath(new URL("../src/cli.mjs", import.meta.url)), "--help"],
    { encoding: "utf8" },
  );
  for (const id of [...BROWSER_IDS, ...launchChannels()]) {
    assert.ok(help.includes(id), id);
  }
});

test("preserves the selected Chromium target and its snapshot profile", () => {
  for (const id of launchChannels()) {
    const options = parseBrowserOptions({ channel: id, importFrom: "firefox" });
    const launch = launchOptions(options, "/profile");
    assert.equal(launch.channel, id);
    assert.equal(launch.migrateFrom.browser, "firefox");
    assert.equal(snapshotBrowser(id), findBrowserSource(id).id);
  }
});

test("unknown imports report every supported id and installed browser discovery", () => {
  assert.throws(
    () => parseImport("netscape"),
    (error) => {
      for (const id of BROWSER_IDS) {
        assert.ok(error.message.includes(id), id);
      }
      assert.match(error.message, /Installed browsers found:/);
      return true;
    },
  );
});

test("reports discovered executable and profile roots without reading cookies", () => {
  const discovery = {
    platform: "linux",
    homeDir: "/users/test",
    environment: { XDG_CONFIG_HOME: "/profiles", PATH: "/browsers" },
    exists: (candidate) =>
      ["/profiles/vivaldi", "/browsers/yandex-browser"].includes(candidate),
  };
  assert.deepEqual(installedBrowsers(discovery), ["vivaldi", "yandex"]);
  assert.throws(
    () => parseImport("netscape", discovery),
    /Installed browsers found: vivaldi, yandex\./,
  );
  assert.throws(
    () => parseImport("netscape", { ...discovery, exists: () => false }),
    /Installed browsers found: none\./,
  );
});

test("accepts every launchable id and alias and rejects unsupported engines", () => {
  for (const channel of launchChannels()) {
    assert.equal(parseBrowserOptions({ channel }).channel, channel);
    assert.equal(
      parseBrowserOptions({ channel: channel.toUpperCase() }).channel,
      channel.toUpperCase(),
    );
  }
  for (const channel of ["safari", "duckduckgo"]) {
    assert.throws(
      () => parseBrowserOptions({ channel }),
      /no launch control protocol/,
    );
  }
  assert.throws(
    () => parseBrowserOptions({ channel: "netscape" }),
    /unknown --browser-channel/,
  );
});

test("extension attachment does not require a real-launcher channel", () => {
  assert.equal(
    parseBrowserOptions({ channel: "firefox", attach: "extension" }).channel,
    "firefox",
  );
});

test("discovers macOS protected Safari roots and Windows roaming profiles", () => {
  assert.deepEqual(
    installedBrowsers({
      platform: "darwin",
      homeDir: "/users/test",
      environment: {},
      exists: (candidate) =>
        candidate ===
        "/users/test/Library/Containers/com.apple.Safari/Data/Library",
    }),
    ["safari"],
  );
  assert.deepEqual(
    installedBrowsers({
      platform: "win32",
      homeDir: "C:\\Users\\test",
      environment: { APPDATA: "D:\\Roaming" },
      exists: (candidate) =>
        candidate === "D:\\Roaming\\Opera Software\\Opera GX Stable",
    }),
    ["opera-gx"],
  );
  assert.deepEqual(
    installedBrowsers({
      platform: "win32",
      homeDir: "C:\\Users\\test",
      environment: { ProgramFiles: "D:\\Programs", Path: "D:\\Browsers" },
      exists: (candidate) =>
        [
          "D:\\Browsers\\vivaldi.exe",
          "D:\\Programs\\Naver\\Naver Whale\\Application\\whale.exe",
          "D:\\Programs\\360\\360se6\\360se.exe",
          "D:\\Programs\\Tencent\\QQBrowser\\QQBrowser.exe",
        ].includes(candidate),
    }),
    ["vivaldi", "whale", "360se", "qq"],
  );
});

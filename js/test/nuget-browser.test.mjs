import assert from "node:assert/strict";
import { test } from "node:test";
import {
  findSignInSources,
  offerSignInImport,
  signInDomains,
} from "../src/sign-in-import.mjs";

test("physical browser aliases are inspected once and NuGet domains follow the chosen route", async () => {
  let reads = 0;
  const sources = await findSignInSources(["nuget.org"], {
    sources: ["chrome", "chromium"],
    listProfiles: async () => [{ name: "Default", path: "/same/profile" }],
    readCookies: async () => {
      reads += 1;
      return [{}];
    },
  });
  assert.equal(sources.length, 1);
  assert.equal(reads, 1);
  assert.deepEqual(signInDomains("nuget", "personal"), [
    "nuget.org",
    "login.live.com",
  ]);
  assert.deepEqual(signInDomains("nuget", "github"), [
    "nuget.org",
    "github.com",
  ]);
});

test("expired cookie candidates continue to saved login in fresh targets without overwriting an existing profile", async () => {
  const targets = [];
  const session = {
    options: {
      browserOptions: { channel: "chrome", import: { browser: "auto" } },
      browserProfile: "/preserve/target",
      log: () => {},
      importDiscovery: {
        sources: ["chrome"],
        defaultBrowserName: "Chrome",
        listProfiles: async () => [{ name: "Default", path: "/source" }],
        readCookies: async () => [{}],
        migrateProfile: async () => ({
          migrated: { passwords: 1, passkeys: 0 },
        }),
      },
    },
    async reconnect(browser) {
      targets.push(this.options.browserProfile);
      this.saved = browser.import.savedLogins;
    },
  };
  const imported = await offerSignInImport(
    session,
    ["nuget.org"],
    async () => session.saved === true,
  );
  assert.equal(imported, true);
  assert.equal(targets.length, 2);
  assert.ok(targets.every((target) => target !== "/preserve/target"));
  assert.notEqual(targets[0], targets[1]);
  const { rm } = await import("node:fs/promises");
  for (const target of session.importProfiles) {
    await rm(target, { recursive: true, force: true });
  }
});

test("local credential diagnostics separate unreadable stores, unknown passkey presence and authentication", async () => {
  const evidence = [];
  await findSignInSources(["nuget.org"], {
    sources: ["chrome"],
    listProfiles: async () => [
      { name: "Default", path: "/unreadable/profile" },
    ],
    readCookies: async () => {
      throw new Error("access denied");
    },
    diagnostic: (item) => evidence.push(item),
  });
  assert.equal(evidence[0].cookies, "unreadable");
  assert.equal(evidence[0].passkey_presence, "unknown");
  assert.equal(evidence[0].authentication, "not-yet-verified");
  assert.equal(evidence[0].passkey_export, "unsupported");
});

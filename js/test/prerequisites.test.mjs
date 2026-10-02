import assert from "node:assert/strict";
import { test } from "node:test";

import {
  githubAuth,
  planPrerequisites,
  renderPrerequisites,
  resolveTrustNpm,
  trustNpmSpec,
  twoFactorMode,
} from "../src/prerequisites.mjs";

test("runs npm trust with npm 12 only on Node.js versions npm 12 supports", () => {
  for (const version of ["v22.22.2", "v24.15.0", "v24.21.0", "v26.0.0"]) {
    assert.equal(trustNpmSpec(version), "npm@^12", version);
  }
  for (const version of ["v20.19.4", "v22.22.1", "v24.14.9", "v25.9.0", null]) {
    assert.equal(trustNpmSpec(version), "npm@^11.10", String(version));
  }
});

test("resolves the trust npm from the matching dist-tag", () => {
  const tags = { latest: "12.2.0", "next-11": "11.21.0", "next-12": "12.2.0" };
  assert.equal(resolveTrustNpm("npm@^12", tags), "12.2.0");
  assert.equal(resolveTrustNpm("npm@^11.10", tags), "11.21.0");
  assert.equal(resolveTrustNpm("npm@^11.10", { latest: "12.2.0" }), null);
  assert.equal(resolveTrustNpm("npm@^12", null), null);
});

test("reads the 2FA mode from npm profile get --json", () => {
  assert.equal(
    twoFactorMode('{"tfa":{"pending":false,"mode":"auth-and-writes"}}'),
    "auth-and-writes",
  );
  assert.equal(twoFactorMode('{"tfa":false}'), null);
  assert.equal(
    twoFactorMode('{"tfa":{"pending":true,"mode":"auth-only"}}'),
    null,
  );
  assert.throws(() => twoFactorMode("not json"));
});

test("summarizes gh auth status without credentials", () => {
  const output = JSON.stringify({
    hosts: {
      "github.com": [
        {
          state: "success",
          active: true,
          login: "octo",
          scopes: "gist, repo, workflow",
        },
      ],
    },
  });
  assert.deepEqual(githubAuth(output), {
    login: "octo",
    scopes: ["gist", "repo", "workflow"],
  });
  assert.deepEqual(githubAuth('{"hosts":{}}'), { login: null, scopes: [] });
  assert.equal(githubAuth("gh: unknown flag --json"), null);
});

test("lists manual prerequisites before an npm plan", () => {
  const plan = {
    registry: "npm",
    steps: [{ id: "audit", command: { program: "gh", args: [] } }],
  };
  const environment = {
    offline: false,
    node: "v20.19.4",
    npm: "10.8.2",
    trustNpm: "npm@^11.10",
    trustNpmVersion: "11.21.0",
    twoFactor: { state: "off" },
    github: { state: "signed-in", login: "octo", scopes: ["gist"] },
  };
  const items = planPrerequisites(plan, environment, { mode: "none" });
  assert.deepEqual(
    items.map((item) => [item.id, item.ok]),
    [
      ["node", true],
      ["npm", true],
      ["npm-trust", true],
      ["npm-2fa", false],
      ["gh", false],
      ["browser", null],
    ],
  );
  assert.deepEqual(renderPrerequisites(items), [
    "  prerequisites:",
    "    - Node.js: v20.19.4; needs ^20.17.0 || >=22.9.0",
    "    - npm: 10.8.2; needs installed",
    "    - npm for npm trust: npm@^11.10, resolves to 11.21.0; needs npm 11.10 or newer; npm 12 only on Node.js ^22.22.2 || ^24.15.0 || >=26.0.0",
    "    - npm two-factor authentication: off; needs enabled at https://docs.npmjs.com/configuring-two-factor-authentication/; npm trust requires it [action needed]",
    "    - GitHub CLI: signed in as octo (scopes: gist); needs signed in with the repo scope, for gh secret and gh run [action needed]",
    "    - Browser: none; URLs are printed (--no-browser); needs signed in to the registry, or ready to sign in",
  ]);
  assert.deepEqual(planPrerequisites({ ...plan, steps: [] }, environment), []);
  assert.deepEqual(planPrerequisites(plan, undefined), []);
});

test("describes where browser pages open", () => {
  const plan = {
    registry: "npm",
    steps: [{ id: "audit", command: { program: "gh", args: [] } }],
  };
  const environment = {
    offline: false,
    twoFactor: { state: "unknown" },
    github: { state: "missing" },
  };
  const describe = (browser) =>
    renderPrerequisites(planPrerequisites(plan, environment, browser)).at(-1);
  assert.equal(
    describe({ mode: "default", channel: "chrome" }),
    "    - Browser: your default browser; forms open in the automated chrome profile; needs signed in to the registry, or ready to sign in",
  );
  assert.equal(
    describe({
      mode: "default",
      channel: "msedge",
      attach: { mode: "snapshot", profile: "Work" },
    }),
    "    - Browser: your default browser; forms open in a temporary snapshot of your edge profile Work; needs signed in to the registry, or ready to sign in",
  );
  assert.equal(
    describe({
      mode: "automated",
      channel: "brave",
      profile: "/state/browser-profile",
      import: { browser: "chrome", profile: "Default" },
    }),
    "    - Browser: the automated brave profile at /state/browser-profile with data imported from chrome:Default; needs signed in to the registry, or ready to sign in",
  );
  assert.equal(
    describe({ mode: "automated", attach: { mode: "extension" } }),
    "    - Browser: your own browser through the Browser Commander extension; needs signed in to the registry, or ready to sign in",
  );
});

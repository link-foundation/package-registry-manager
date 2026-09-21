import assert from "node:assert/strict";
import { test } from "node:test";

import {
  removeAlwaysAuthEntries,
  sanitizeNpmUserConfig,
} from "../scripts/sanitize-npm-userconfig.mjs";

const quietLogger = { log() {}, warn() {} };

test("removes only deprecated always-auth entries", () => {
  const result = removeAlwaysAuthEntries(
    [
      "//registry.npmjs.org/:_authToken=${NODE_AUTH_TOKEN}",
      "always-auth=true",
      "legacy-peer-deps=false",
      "",
    ].join("\n"),
  );
  assert.deepEqual(result, {
    content: [
      "//registry.npmjs.org/:_authToken=${NODE_AUTH_TOKEN}",
      "legacy-peer-deps=false",
      "",
    ].join("\n"),
    removed: true,
  });
});

test("updates the setup-node user configuration", () => {
  const path = "/tmp/npm-userconfig-test";
  const files = new Map([[path, "always-auth=false\nfund=false\n"]]);
  const result = sanitizeNpmUserConfig({
    env: { NPM_CONFIG_USERCONFIG: path },
    logger: quietLogger,
    fileExists: (candidate) => files.has(candidate),
    readFile: (candidate) => files.get(candidate),
    writeFile: (candidate, content) => files.set(candidate, content),
  });
  assert.deepEqual(result, { path, removed: true, skipped: false });
  assert.equal(files.get(path), "fund=false\n");
});

test("skips when setup-node did not create a user configuration", () => {
  assert.deepEqual(sanitizeNpmUserConfig({ env: {}, logger: quietLogger }), {
    path: "",
    removed: false,
    skipped: true,
  });
});

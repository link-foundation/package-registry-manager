import assert from "node:assert/strict";
import { test } from "node:test";

import {
  NPM_MIN_VERSION,
  compareVersions,
  ensureSupportedNpm,
  isVersionAtLeast,
} from "../scripts/setup-npm.mjs";

const quietLogger = { log() {}, warn() {} };

test("compares npm semantic versions numerically", () => {
  assert.equal(compareVersions("11.10.0", "11.9.9"), 1);
  assert.equal(compareVersions(NPM_MIN_VERSION, NPM_MIN_VERSION), 0);
  assert.equal(compareVersions("11.5.0", NPM_MIN_VERSION), -1);
  assert.equal(isVersionAtLeast("11.12.1", NPM_MIN_VERSION), true);
});

test("upgrades an old npm and verifies the resulting version", () => {
  const calls = [];
  let versionChecks = 0;
  const runner = (program, args) => {
    calls.push([program, ...args]);
    if (program === "npm" && args[0] === "--version") {
      versionChecks += 1;
      return {
        status: 0,
        stdout: versionChecks === 1 ? "10.9.4\n" : "11.8.0\n",
        stderr: "",
      };
    }
    return { status: 0, stdout: "", stderr: "" };
  };

  assert.equal(
    ensureSupportedNpm({ nodeVersion: "v24.0.0", runner, logger: quietLogger }),
    "11.8.0",
  );
  assert.deepEqual(calls[1], ["npm", "install", "--global", "npm@11"]);
});

test("fails before invoking npm on an unsupported Node.js runtime", () => {
  assert.throws(
    () =>
      ensureSupportedNpm({
        nodeVersion: "v20.19.0",
        runner: () => assert.fail("npm must not run"),
        logger: quietLogger,
      }),
    /Node\.js 22\.14\.0 or later/u,
  );
});

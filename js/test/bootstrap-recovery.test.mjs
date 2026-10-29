import assert from "node:assert/strict";
import { test } from "node:test";
import {
  bootstrapNames,
  verifyBootstrapRecovery,
} from "../src/bootstrap-recovery.mjs";

test("bootstrap diagnostic records exact missing names without classifying build failures", () => {
  assert.deepEqual(
    bootstrapNames(
      "npm owner setup required for gh-upload-file, gh-upload-log: first publish each missing package",
    ),
    ["gh-upload-file", "gh-upload-log"],
  );
  assert.deepEqual(bootstrapNames("build failed"), []);
});

test("bootstrap recovery refuses missing names and unknown trust", async () => {
  const session = {
    bootstrapNames: ["alias"],
    plan: { package: {} },
    options: { probePackage: async () => ({ exists: false }) },
  };
  await assert.rejects(
    verifyBootstrapRecovery(session),
    /unresolved for alias/,
  );
  session.options.probePackage = async () => ({ exists: true });
  session.capture = async () => ({ code: 0, stdout: "[]" });
  await assert.rejects(
    verifyBootstrapRecovery(session),
    /publisher is not verified/,
  );
});

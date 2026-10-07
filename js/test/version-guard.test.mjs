import assert from "node:assert/strict";
import { test } from "node:test";
import { probeVersionGuard } from "../../experiments/issue-39/version-guard-probe.mjs";

test(
  "generated npm and cargo guards skip existing versions and fail closed (#38)",
  { timeout: 15000 },
  async () => {
    const cases = await probeVersionGuard();
    assert.equal(cases.length, 10);
  },
);

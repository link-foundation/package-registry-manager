import assert from "node:assert/strict";
import { test } from "node:test";

import { classifyRegistryLookup } from "../scripts/publish-to-npm.mjs";

test("registry lookup distinguishes published, missing, and unknown states", () => {
  assert.equal(
    classifyRegistryLookup(
      { status: 0, stdout: '"1.2.3"\n', stderr: "" },
      "1.2.3",
    ),
    "published",
  );
  assert.equal(
    classifyRegistryLookup(
      { status: 1, stdout: "", stderr: "npm error code E404" },
      "1.2.3",
    ),
    "missing",
  );
  assert.equal(
    classifyRegistryLookup(
      { status: 1, stdout: "", stderr: "network timeout" },
      "1.2.3",
    ),
    "unknown",
  );
});

import assert from "node:assert/strict";
import { test } from "node:test";
import { fallbackSteps } from "../src/workflow-fallback.mjs";

test("fallback labels preserve names containing Unicode line separators", () => {
  for (const registry of ["npm", "pypi"]) {
    const title = `Release\u2028package`;
    const spacing = " ".repeat(32);
    const [oidc, fallback] = fallbackSteps(
      [`      - name:${spacing}"${title}"`, "        run: publish-package"],
      registry,
      new Set(),
      6,
      0,
      (lines) => [...lines],
    );
    assert.ok(oidc.includes(`      - name:${spacing}"${title} (OIDC)"`));
    assert.ok(
      fallback.includes(`      - name:${spacing}"${title} (token fallback)"`),
    );
  }
});

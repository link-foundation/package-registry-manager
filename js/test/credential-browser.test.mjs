import assert from "node:assert/strict";
import { test } from "node:test";
import vm from "node:vm";
import {
  tokenFormScript,
  READ_TOKEN,
  revokedScript,
} from "../src/credential-browser.mjs";

function evaluate(script, fields) {
  return vm.runInNewContext(script, {
    document: { querySelector: (selector) => fields[selector] },
    Event: class {
      constructor(type) {
        this.type = type;
      }
    },
  });
}

test("credential forms fill only the offered publishing scope and finite expiry", () => {
  const field = (tagName, options) => ({
    tagName,
    options,
    value: "original",
    dispatchEvent() {},
  });
  const name = field("INPUT");
  const scope = field("SELECT", [
    { value: "read", textContent: "Read" },
    { value: "write", textContent: "Read & Write" },
  ]);
  const expiry = field("INPUT");
  const fields = {
    'input[name="name"]': name,
    'select[name="scope"]': scope,
    'input[type="date"]': expiry,
  };
  const script = tokenFormScript(
    { scope: "Read & Write" },
    "prm-tool",
    "2026-11-07T00:00:00Z",
  );
  assert.deepEqual(Array.from(evaluate(script, fields).filled), [
    "name",
    "scope",
    "expires",
  ]);
  assert.equal(name.value, "prm-tool");
  assert.equal(scope.value, "write");
  assert.equal(expiry.value, "2026-11-07");
  scope.options = [{ value: "admin", textContent: "All access" }];
  scope.value = "original";
  assert.deepEqual(Array.from(evaluate(script, fields).filled), [
    "name",
    "expires",
  ]);
  assert.equal(scope.value, "original");
});

test("one-time token values carry verified identity and normalized expiry", () => {
  const result = evaluate(READ_TOKEN, {
    'input[data-testid="token-value"]': { value: "one-time-value" },
    "[data-token-id]": { dataset: { tokenId: "new" } },
    'input[name="expires_at"]': { value: "2026-11-07" },
  });
  assert.equal(result.value, "one-time-value");
  assert.equal(result.id, "new");
  assert.equal(result.expires_at, "2026-11-07T00:00:00Z");
});

test("a missing token list cannot prove revocation", () => {
  const script = revokedScript("old");
  assert.equal(evaluate(script, {}), false);
  const list = '[data-testid="tokens-list"], [data-token-list]';
  assert.equal(
    evaluate(script, {
      [list]: { querySelectorAll: () => [{ dataset: { tokenId: "old" } }] },
    }),
    false,
  );
  assert.equal(
    evaluate(script, {
      [list]: { querySelectorAll: () => [{ dataset: { tokenId: "new" } }] },
    }),
    true,
  );
});

test("unbounded token identifiers cannot prove revocation", () => {
  assert.equal(
    evaluate(revokedScript("x".repeat(257)), {
      '[data-testid="tokens-list"], [data-token-list]': {
        querySelectorAll: () => [],
      },
    }),
    false,
  );
});

test("registry token identifiers remain data when they contain script text", () => {
  const id = 'old"; globalThis.injected = true; // </script> λ😀';
  assert.equal(
    evaluate(revokedScript(id), {
      '[data-testid="tokens-list"], [data-token-list]': {
        querySelectorAll: () => [{ dataset: { tokenId: id } }],
      },
    }),
    false,
  );
});

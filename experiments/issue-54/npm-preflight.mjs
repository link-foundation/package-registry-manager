// Read-only, finite live checks: no login, approval or publication.
import assert from "node:assert/strict";
import { checkNamePolicy } from "../../js/src/npm-policy.mjs";
for (const name of ["gh-upload", "lodahsx", "js-on-stream"]) {
  await assert.rejects(checkNamePolicy(name), /may be refused as too similar/);
  console.log(`${name}: refused before authentication`);
}

// Run with: node --max-old-space-size=64 experiments/issue-39/publisher-regex-probe.mjs
// Finite hostile input and a VM execution budget bound the backtracking probe.
import assert from "node:assert/strict";
import vm from "node:vm";
import { JOB_PATTERNS } from "../../js/src/publishers.mjs";

const pattern = JOB_PATTERNS.get("maven-central");
for (const [text, expected] of [
  [`mvn ${"-! -".repeat(24)}x`, false],
  ["mvn --batch-mode -DskipTests deploy", true],
  ["mvn -B deploy", true],
  ["mvn --batch-mode verify", false],
]) {
  assert.equal(vm.runInNewContext("pattern.test(text)", { pattern, text }, { timeout: 250 }), expected);
}
console.log("Four bounded Maven publisher cases passed.");

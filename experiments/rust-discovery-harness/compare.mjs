// Compares the offline inspection of the JavaScript and Rust discovery code
// (publisher detection, skipped manifests) on real repositories.
// Usage: node experiments/rust-discovery-harness/compare.mjs [repository...]
import { execFile } from "node:child_process";
import path from "node:path";
import { isDeepStrictEqual, promisify } from "node:util";

import { inspectRepository } from "../../js/src/discovery.mjs";

const run = promisify(execFile);
const harness = import.meta.dirname;
const root = path.resolve(harness, "../..");
await run("cargo", ["build", "--quiet", "--example", "inspect"], { cwd: harness });
const binary = path.join(harness, "target/debug/examples/inspect");

const repositories = process.argv.slice(2);
if (repositories.length === 0) {
  repositories.push(
    root,
    path.join(root, "tests/fixtures/polyglot"),
    path.join(root, "tests/fixtures/pipeline-template"),
  );
}
let failures = 0;
for (const repository of repositories) {
  const fromJs = JSON.parse(
    JSON.stringify(await inspectRepository(repository, { includeSkipped: true })),
  );
  const fromRust = JSON.parse((await run(binary, [repository])).stdout);
  // JSON.stringify drops undefined; Rust writes null for a missing release workflow.
  fromJs.repository.release_workflow ??= null;
  const same = isDeepStrictEqual(fromJs, fromRust);
  failures += same ? 0 : 1;
  console.log(`${same ? "same" : "DIFFERENT"}: ${repository}`);
  if (!same) {
    console.log("js:", JSON.stringify(fromJs, null, 2));
    console.log("rust:", JSON.stringify(fromRust, null, 2));
  }
}
process.exitCode = failures ? 1 : 0;

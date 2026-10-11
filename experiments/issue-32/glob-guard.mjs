import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import os from "node:os";
import path from "node:path";

// Resolve through the application's dependency tree, even from experiments/.
const require = createRequire(new URL("../../js/package.json", import.meta.url));
const nested = (depth) => "{".repeat(depth) + "a,b" + "}".repeat(depth);
if (!process.argv.includes("--guard")) {
  // Retain the original bounded reproducer for the unguarded fast-glob path.
  const dependencyRequire = createRequire(require.resolve("command-stream"));
  const shellRequire = createRequire(dependencyRequire.resolve("shelljs"));
  const glob = shellRequire("fast-glob");
  glob.sync(nested(1000)); // Deliberately finite; invoke only with stack/heap limits.
} else {
  // command-stream 2.0.2 supplies its own iterative glob implementation;
  // resolving shelljs would test an unrelated transitive dependency instead.
  const { glob } = require("command-stream/zx");
  const cwd = await mkdtemp(path.join(os.tmpdir(), "prm-glob-guard-"));
  try {
    await writeFile(path.join(cwd, "a"), "");
    await writeFile(path.join(cwd, "b"), "");
    assert.deepEqual(glob.sync("{a,b}", { cwd }), ["a", "b"]);
    assert.deepEqual(glob.sync(nested(1000), { cwd }), []);
    assert.deepEqual(glob.sync("*", { cwd, ignore: nested(1000) }), ["a", "b"]);
    assert.deepEqual(glob.sync("(".repeat(1000) + "*", { cwd }), []);
    // Fourteen binary groups exceed the library's 10,000-pattern budget.
    const product = "{a,b}".repeat(14);
    assert.throws(() => glob.sync(product, { cwd }), /10000 patterns/);
    assert.throws(() => glob.sync("*", { cwd, ignore: product }), /10000 patterns/);
    console.log("installed glob guard passed");
  } finally {
    await rm(cwd, { recursive: true, force: true });
  }
}

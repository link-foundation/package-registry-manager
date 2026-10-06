import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";

// Resolve through the application's dependency tree, even from experiments/.
const require = createRequire(new URL("../../js/package.json", import.meta.url));
if (process.argv.includes("--guard")) {
  await import(pathToFileURL(require.resolve("command-stream")).href);
}
const dependencyRequire = createRequire(require.resolve("command-stream"));
const shellRequire = createRequire(dependencyRequire.resolve("shelljs"));
const glob = shellRequire("fast-glob");
const nested = (depth) => "{".repeat(depth) + "a,b" + "}".repeat(depth);
if (!process.argv.includes("--guard")) {
  glob.sync(nested(1000)); // Deliberately finite; invoke only with stack/heap limits.
} else {
  assert.throws(() => glob.sync(nested(101)), /100 levels/);
  assert.throws(() => glob.sync("*", { ignore: nested(101) }), /100 levels/);
  assert.throws(() => glob.sync("(".repeat(101)), /100 levels/);
  assert.throws(() => glob.sync("x".repeat(10001)), /10000 characters/);
  console.log("installed glob guard passed");
}

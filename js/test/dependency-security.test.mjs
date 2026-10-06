import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import test from "node:test";

test(
  "installed command-stream guards the one exempted glob advisory",
  { timeout: 10000 },
  () => {
    const result = spawnSync(
      process.execPath,
      [
        "--max-old-space-size=64",
        "--stack-size=256",
        fileURLToPath(
          new URL("../../experiments/issue-32/glob-guard.mjs", import.meta.url),
        ),
        "--guard",
      ],
      { encoding: "utf8", maxBuffer: 64 * 1024, timeout: 5000 },
    );
    assert.ifError(result.error);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /installed glob guard passed/);
  },
);

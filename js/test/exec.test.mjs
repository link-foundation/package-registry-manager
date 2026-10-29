import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { exec } from "../src/exec.mjs";

test("captured commands preserve literal arguments and output newlines", async () => {
  const args = [
    "value with spaces",
    'literal & "quotes"',
    "%PATH%",
    "$(exit 99)",
  ];
  const result = await exec(process.execPath, [
    "-e",
    "process.stdout.write(JSON.stringify(process.argv.slice(1))+'\\n')",
    ...args,
  ]);
  assert.equal(result.code, 0);
  assert.equal(result.stdout, `${JSON.stringify(args)}\n`);
  assert.equal(result.stderr, "");
});

test("captured command failures retain their exit code and stderr", async () => {
  const result = await exec(process.execPath, [
    "-e",
    "process.stderr.write('diagnostic\\n');process.exitCode=42",
  ]);
  assert.equal(result.code, 42);
  assert.equal(result.stdout, "");
  assert.equal(result.stderr, "diagnostic\n");
});

test(
  "captured commands resolve Windows batch launchers on PATH",
  { skip: process.platform !== "win32" },
  async () => {
    const directory = await mkdtemp(path.join(tmpdir(), "prm-cmd-"));
    try {
      await writeFile(
        path.join(directory, "prm-fixture.cmd"),
        `@echo off\r\n"${process.execPath}" "%~dp0args.mjs" %*\r\n`,
      );
      await writeFile(
        path.join(directory, "args.mjs"),
        "process.stdout.write(JSON.stringify(process.argv.slice(2)))",
      );
      const pathKey =
        Object.keys(process.env).find((key) => key.toLowerCase() === "path") ??
        "Path";
      const args = ["value with spaces", "literal & value"];
      const result = await exec("prm-fixture", args, {
        env: {
          ...process.env,
          [pathKey]: `${directory}${path.delimiter}${process.env[pathKey]}`,
        },
      });
      assert.equal(result.code, 0, result.stderr);
      assert.deepEqual(JSON.parse(result.stdout), args);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  },
);

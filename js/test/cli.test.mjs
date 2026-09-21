import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { promisify } from "node:util";

import { isDirectExecution } from "../src/cli.mjs";

const execute = promisify(execFile);
const here = path.dirname(fileURLToPath(import.meta.url));
const cli = path.resolve(here, "../src/cli.mjs");
const fixture = path.resolve(here, "../../tests/fixtures/polyglot");

test("recognizes the CLI entry point as a portable file URL", () => {
  assert.equal(isDirectExecution(pathToFileURL(cli).href, cli), true);
  assert.equal(
    isDirectExecution(pathToFileURL(cli).href, `${cli}.different`),
    false,
  );
});

test("inspect emits machine-readable output", async () => {
  const { stdout } = await execute(process.execPath, [
    cli,
    "--repository",
    fixture,
    "--format",
    "json",
    "inspect",
  ]);
  const inspection = JSON.parse(stdout);
  assert.equal(inspection.schema_version, 1);
  assert.equal(inspection.packages.length, 7);
  assert.equal(inspection.repository.github_owner, "acme");
  assert.equal(inspection.repository.github_repository, "polyglot");
});

test("setup is safe by default", async () => {
  const { stdout } = await execute(process.execPath, [
    cli,
    "--repository",
    fixture,
    "setup",
    "--registry",
    "npm",
  ]);
  assert.match(stdout, /Dry run only/);
});

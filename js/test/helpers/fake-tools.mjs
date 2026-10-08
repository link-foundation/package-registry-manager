import { chmod, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { executePlan, executePlans } from "../../src/setup.mjs";

const FAKE_TOOL = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../../../tests/fixtures/fake-tools/fake-tool.cjs",
);

export async function installFakeTools(directory) {
  const bin = path.join(directory, "bin");
  await mkdir(bin, { recursive: true });
  for (const tool of [
    "npm",
    "npx",
    "git",
    "gh",
    "open",
    "xdg-open",
    "defaults",
    "xdg-settings",
  ]) {
    const file = path.join(bin, tool);
    await writeFile(
      file,
      `#!${process.execPath}\n${await readFile(FAKE_TOOL, "utf8")}`,
    );
    await chmod(file, 0o755);
  }
  return bin;
}

export async function runMockSetup(plan, registry, repository, overrides = {}) {
  const lines = [];
  const originalLog = console.log;
  const originalPath = process.env.PATH;
  const state = process.env.FAKE_STATE;
  // Clear the previous run before starting children. A detached opener may
  // append after the final snapshot, so preserve that log for the polling tests.
  await rm(path.join(state, "log.jsonl"), { force: true });
  process.env.PATH = `${path.join(state, "bin")}${path.delimiter}${originalPath}`;
  console.log = (...values) => lines.push(values.join(" "));
  try {
    const execute = Array.isArray(plan) ? executePlans : executePlan;
    await execute(plan, {
      repository,
      execute: true,
      yes: true,
      noBrowser: true,
      verbose: false,
      pollIntervalMs: 1,
      // Acknowledge simulated OIDC verification; browser steps wait for Enter.
      prompt: async (message) => (message.endsWith("[y/N] ") ? "y" : ""),
      fetch: async (url) => {
        const found = registry(url);
        return {
          status: found ? 200 : 404,
          ok: Boolean(found),
          json: async () => found,
        };
      },
      ...overrides,
    });
  } catch (error) {
    error.lines = lines;
    throw error;
  } finally {
    console.log = originalLog;
    process.env.PATH = originalPath;
  }
  const log = (await readFile(path.join(state, "log.jsonl"), "utf8"))
    .trim()
    .split("\n")
    .map((line) => JSON.parse(line));
  return { lines, log };
}

export async function readLog(state) {
  const file = path.join(state, "log.jsonl");
  const text = await readFile(file, "utf8").catch(() => "");
  return text
    .trim()
    .split("\n")
    .filter(Boolean)
    .map((line) => JSON.parse(line));
}

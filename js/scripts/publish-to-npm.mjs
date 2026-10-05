#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { appendFileSync, readFileSync } from "node:fs";
import { setTimeout as sleep } from "node:timers/promises";

import { isDirectExecution } from "../src/direct-execution.mjs";

const packageRoot = new URL("../", import.meta.url);

/**
 * Seconds to wait between visibility polls after `npm publish` exits 0: a
 * short first wait, doubling to one poll a minute, about ten minutes in all.
 * A new version can take several minutes to appear on the registry (#28).
 */
export const VISIBILITY_DELAYS = [
  5, 10, 20, 40, 60, 60, 60, 60, 60, 60, 60, 60, 60, 60,
];

/**
 * `npm view` arguments for one registry lookup. npm caches packuments, and the
 * pre-publish lookup would otherwise answer every later poll with the stale,
 * pre-publish document, so each lookup asks the registry again (#28).
 */
export function registryLookupArgs(spec) {
  return ["view", spec, "version", "--json", "--prefer-online"];
}

export function classifyRegistryLookup(result, expectedVersion) {
  if (result.status === 0) {
    const version = result.stdout.trim().replaceAll('"', "");
    return version === expectedVersion ? "published" : "unexpected";
  }
  const diagnostic = `${result.stdout}\n${result.stderr}`;
  return /(?:E404|404 Not Found)/u.test(diagnostic) ? "missing" : "unknown";
}

function npm(args) {
  return spawnSync("npm", args, {
    cwd: packageRoot,
    encoding: "utf8",
    env: process.env,
    stdio: args[0] === "publish" ? "inherit" : "pipe",
  });
}

function setOutput(name, value) {
  if (process.env.GITHUB_OUTPUT) {
    appendFileSync(process.env.GITHUB_OUTPUT, `${name}=${value}\n`);
  }
}

/**
 * Poll the registry until `version` is visible. Returns whether it became
 * visible; running out of attempts is not an error because `npm publish`
 * already reported success.
 */
export async function waitForVisibility(
  spec,
  version,
  {
    run = npm,
    delays = VISIBILITY_DELAYS,
    wait = (seconds) => sleep(seconds * 1000),
    log = console.log,
  } = {},
) {
  const attempts = delays.length + 1;
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    const state = classifyRegistryLookup(
      run(registryLookupArgs(spec)),
      version,
    );
    if (state === "published") {
      log(`${spec} is visible on the npm registry (attempt ${attempt}).`);
      return true;
    }
    if (attempt < attempts) {
      const delay = delays[attempt - 1];
      log(
        `${spec} is not visible yet (${state}, attempt ${attempt}/${attempts}); retrying in ${delay}s.`,
      );
      await wait(delay);
    }
  }
  return false;
}

export function totalWaitSeconds(delays = VISIBILITY_DELAYS) {
  return delays.reduce((sum, delay) => sum + delay, 0);
}

async function main() {
  const manifest = JSON.parse(
    readFileSync(new URL("package.json", packageRoot), "utf8"),
  );
  const spec = `${manifest.name}@${manifest.version}`;
  const lookup = npm(registryLookupArgs(spec));
  const state = classifyRegistryLookup(lookup, manifest.version);

  if (state === "published") {
    console.log(`${spec} is already published; nothing to do.`);
    setOutput("published", "false");
    setOutput("published_version", manifest.version);
    return;
  }
  if (state !== "missing") {
    throw new Error(
      `Could not determine whether ${spec} exists; refusing to publish. ${lookup.stderr.trim()}`,
    );
  }

  const publish = npm(["publish", "--access", "public", "--provenance"]);
  if (publish.status !== 0) {
    throw new Error(`npm publish failed with exit status ${publish.status}`);
  }
  if (!(await waitForVisibility(spec, manifest.version))) {
    console.log(
      `::warning title=npm registry lag::npm publish succeeded, but ${spec} was not visible from the npm registry after ${totalWaitSeconds()} seconds. Check with: npm view ${spec} --prefer-online`,
    );
  }
  setOutput("published", "true");
  setOutput("published_version", manifest.version);
}

if (isDirectExecution(import.meta.url, process.argv[1])) {
  main().catch((error) => {
    console.error(`::error title=npm publication failed::${error.message}`);
    process.exitCode = 1;
  });
}

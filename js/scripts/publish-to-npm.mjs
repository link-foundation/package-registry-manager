#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { appendFileSync, readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

const packageRoot = new URL("../", import.meta.url);

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

async function waitForVisibility(spec, version) {
  for (let attempt = 1; attempt <= 12; attempt += 1) {
    const result = npm(["view", spec, "version", "--json"]);
    if (classifyRegistryLookup(result, version) === "published") {
      return;
    }
    if (attempt < 12) {
      await new Promise((resolve) => setTimeout(resolve, 10_000));
    }
  }
  throw new Error(
    `${spec} was not visible from the npm registry after 120 seconds`,
  );
}

async function main() {
  const manifest = JSON.parse(
    readFileSync(new URL("package.json", packageRoot), "utf8"),
  );
  const spec = `${manifest.name}@${manifest.version}`;
  const lookup = npm(["view", spec, "version", "--json"]);
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
  await waitForVisibility(spec, manifest.version);
  setOutput("published", "true");
  setOutput("published_version", manifest.version);
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  main().catch((error) => {
    console.error(`::error title=npm publication failed::${error.message}`);
    process.exitCode = 1;
  });
}

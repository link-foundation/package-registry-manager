#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import process from "node:process";
import { pathToFileURL } from "node:url";

export const NPM_MIN_VERSION = "11.5.1";
export const NODE_MIN_VERSION = "22.14.0";

export function parseVersion(version) {
  const match = String(version)
    .trim()
    .match(
      /^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/u,
    );
  if (!match) {
    throw new Error(`invalid semantic version: ${version}`);
  }
  return {
    major: Number(match[1]),
    minor: Number(match[2]),
    patch: Number(match[3]),
    prerelease: match[4] ?? "",
  };
}

export function compareVersions(leftVersion, rightVersion) {
  const left = parseVersion(leftVersion);
  const right = parseVersion(rightVersion);
  for (const key of ["major", "minor", "patch"]) {
    if (left[key] !== right[key]) {
      return left[key] > right[key] ? 1 : -1;
    }
  }
  if (left.prerelease === right.prerelease) {
    return 0;
  }
  if (!left.prerelease) {
    return 1;
  }
  if (!right.prerelease) {
    return -1;
  }
  return left.prerelease > right.prerelease ? 1 : -1;
}

export function isVersionAtLeast(version, minimumVersion) {
  return compareVersions(version, minimumVersion) >= 0;
}

function run(program, args, options = {}) {
  return spawnSync(program, args, {
    encoding: "utf8",
    ...options,
  });
}

function readNpmVersion(runner) {
  const result = runner("npm", ["--version"]);
  if (result.error || result.status !== 0) {
    throw new Error(
      `npm --version failed: ${result.error?.message ?? result.stderr}`,
    );
  }
  return result.stdout.trim();
}

export function ensureSupportedNpm({
  nodeVersion = process.version,
  runner = run,
  logger = console,
} = {}) {
  if (!isVersionAtLeast(nodeVersion, NODE_MIN_VERSION)) {
    throw new Error(
      `Node.js ${NODE_MIN_VERSION} or later is required for npm OIDC trusted publishing; found ${nodeVersion}`,
    );
  }

  let npmVersion = readNpmVersion(runner);
  logger.log(`Current Node.js version: ${nodeVersion}`);
  logger.log(`Current npm version: ${npmVersion}`);
  if (isVersionAtLeast(npmVersion, NPM_MIN_VERSION)) {
    return npmVersion;
  }

  const strategies = [
    ["npm", ["install", "--global", "npm@11"]],
    ["npx", ["--yes", "npm@11", "install", "--global", "npm@11"]],
    ["corepack", ["prepare", "npm@11", "--activate"]],
  ];
  for (const [program, args] of strategies) {
    logger.log(`Trying ${program} ${args.join(" ")}...`);
    const result = runner(program, args, { stdio: "inherit" });
    if (!result.error && result.status === 0) {
      npmVersion = readNpmVersion(runner);
      if (isVersionAtLeast(npmVersion, NPM_MIN_VERSION)) {
        logger.log(`Updated npm version: ${npmVersion}`);
        return npmVersion;
      }
    }
    logger.warn(`${program} strategy did not install a supported npm version.`);
  }

  throw new Error(
    `could not update npm to ${NPM_MIN_VERSION} or later for OIDC trusted publishing`,
  );
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  try {
    ensureSupportedNpm();
  } catch (error) {
    console.error(`::error title=npm setup failed::${error.message}`);
    process.exitCode = 1;
  }
}

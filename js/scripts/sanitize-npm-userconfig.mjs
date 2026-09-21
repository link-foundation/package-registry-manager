#!/usr/bin/env node

import { existsSync, readFileSync, writeFileSync } from "node:fs";
import process from "node:process";
import { pathToFileURL } from "node:url";

const ALWAYS_AUTH_LINE = /^[^\S\r\n]*always-auth[^\S\r\n]*=.*(?:\r?\n|$)/gimu;

export function removeAlwaysAuthEntries(content) {
  const sanitized = String(content).replace(ALWAYS_AUTH_LINE, "");
  return { content: sanitized, removed: sanitized !== content };
}

export function sanitizeNpmUserConfig({
  env = process.env,
  logger = console,
  fileExists = existsSync,
  readFile = readFileSync,
  writeFile = writeFileSync,
} = {}) {
  const userConfigPath = env.NPM_CONFIG_USERCONFIG ?? "";
  if (!userConfigPath) {
    logger.log("NPM_CONFIG_USERCONFIG is not set; no npm config to clean.");
    return { path: "", removed: false, skipped: true };
  }
  if (!fileExists(userConfigPath)) {
    logger.warn(`npm user config does not exist: ${userConfigPath}`);
    return { path: userConfigPath, removed: false, skipped: true };
  }

  const original = readFile(userConfigPath, "utf8");
  const result = removeAlwaysAuthEntries(original);
  if (result.removed) {
    writeFile(userConfigPath, result.content);
    logger.log(`Removed deprecated always-auth entry from ${userConfigPath}.`);
  } else {
    logger.log(`No deprecated always-auth entry found in ${userConfigPath}.`);
  }
  return { path: userConfigPath, removed: result.removed, skipped: false };
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  sanitizeNpmUserConfig();
}

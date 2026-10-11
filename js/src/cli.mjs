#!/usr/bin/env node

import { isDirectExecution } from "./direct-execution.mjs";
import { supportsNode, nodeRequirement } from "./node-version.mjs";

export { isDirectExecution };

// Dynamic import keeps Node 22-only dependencies behind the runtime check.
const supported = supportsNode(process.versions.node);
const implementation = supported ? await import("./cli-main.mjs") : null;

export async function main(args = process.argv.slice(2)) {
  if (!supported) {
    throw new Error(nodeRequirement(process.versions.node));
  }
  return implementation.main(args);
}

if (isDirectExecution(import.meta.url, process.argv[1])) {
  if (!supported) {
    process.stderr.write(`${nodeRequirement(process.versions.node)}\n`);
    process.exitCode = 1;
  } else {
    main().catch((error) => {
      process.stderr.write(`error: ${error.message}\n`);
      process.exitCode = 1;
    });
  }
}

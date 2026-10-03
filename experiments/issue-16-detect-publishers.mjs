// Prints which workflow and jobs publish each trusted-publishing registry.
// Usage: node experiments/issue-16-detect-publishers.mjs [repository]
import path from "node:path";

import { detectPublisher, tokenSecrets } from "../js/src/publishers.mjs";
import { readWorkflows } from "../js/src/workflows.mjs";

const root = path.resolve(process.argv[2] ?? ".");
const workflows = await readWorkflows(root);
for (const registry of ["npm", "crates-io", "pypi"]) {
  console.log(registry, JSON.stringify(await detectPublisher(root, workflows, registry)));
  console.log("  token secrets:", JSON.stringify(tokenSecrets(workflows, registry)));
}

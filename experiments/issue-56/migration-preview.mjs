// Preview the deterministic multi-file conversion without touching GitHub or registries.
import { writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { inspectRepository } from "../../js/src/discovery.mjs";
import { readWorkflows } from "../../js/src/workflows.mjs";
import { workflowMigrationProposal } from "../../js/src/workflow-migration.mjs";

const root = fileURLToPath(new URL("../../tests/fixtures/token-rotation", import.meta.url));
const inspection = await inspectRepository(root);
const proposal = await workflowMigrationProposal(root, inspection.packages, await readWorkflows(root));
const output = `${JSON.stringify(proposal.files, null, 2)}\n`;
if (process.argv.includes("--record")) await writeFile(`${root}/expected.json`, output);
else process.stdout.write(output);

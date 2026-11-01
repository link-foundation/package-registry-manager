// Regenerates a fixture's expected-inspection.json from the JS port (review the diff!).
import { writeFileSync } from "node:fs";
import { inspectRepository } from "../../js/src/discovery.mjs";
const dir = process.argv[2];
const inspection = await inspectRepository(dir);
inspection.repository.root = "<ROOT>";
writeFileSync(`${dir}/expected-inspection.json`, JSON.stringify(inspection, null, 2) + "\n");

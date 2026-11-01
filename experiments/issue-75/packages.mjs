import { inspectRepository } from "../../js/src/discovery.mjs";
const inspection = await inspectRepository(process.argv[2] ?? "/tmp/ln-src");
const only = process.argv.slice(3);
for (const p of inspection.packages.filter((p) => !only.length || only.includes(p.registry))) console.log(JSON.stringify(p, null, 1));

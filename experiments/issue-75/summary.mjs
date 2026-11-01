// Compact identity summary used by the links-notation fixture contract.
import { inspectRepository } from "../../js/src/discovery.mjs";
export async function summary(dir) {
  const inspection = await inspectRepository(dir);
  return inspection.packages.map((p) => ({
    registry: p.registry,
    name: p.name,
    version: p.version ?? null,
    manifest: p.manifest,
    publishable: p.publishable,
    workflow: p.workflow ?? null,
    workflow_jobs: p.workflow_jobs ?? [],
    workflow_callees: p.workflow_callees ?? [],
    publishing_policy: p.publishing_policy ?? null,
  }));
}
if (process.argv[2]) console.log(JSON.stringify(await summary(process.argv[2]), null, 2));

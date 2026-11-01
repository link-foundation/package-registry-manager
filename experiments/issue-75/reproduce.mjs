// Usage: node experiments/issue-75/reproduce.mjs <links-notation checkout>
import { inspectRepository } from '../../js/src/discovery.mjs';
const root = process.argv[2];
const inspection = await inspectRepository(root, { includeSkipped: true });
for (const p of inspection.packages ?? []) {
  console.log(JSON.stringify({ registry: p.registry, name: p.name, version: p.version, manifest: p.manifest ?? p.path, workflow: p.workflow ?? null, jobs: p.workflow_jobs, policy: p.publishing_policy, warnings: p.warnings, skip: p.skipReason ?? p.reason }));
}
for (const s of inspection.skipped ?? []) console.log('SKIP', JSON.stringify(s));

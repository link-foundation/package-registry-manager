import { mkdtemp, mkdir, writeFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { inspectRepository } from "../../js/src/discovery.mjs";
import { buildPlans } from "../../js/src/plan.mjs";
const root = await mkdtemp(path.join(os.tmpdir(), "prm-issue-33-"));
try {
  const files = {
    ".git/config":
      '[remote "origin"]\nurl = https://github.com/acme/polyglot.git\n',
    "js/package.json": '{"name":"tool","version":"1.0.0"}',
    "rust/Cargo.toml": '[package]\nname="tool"\nversion="1.0.0"\n',
    "python/pyproject.toml":
      '[project]\nname="tool"\nversion="1.0.0"\nrequires-python=">=3.13"\n',
    "scripts/publish-to-pypi.mjs":
      "await $`cd python && python -m twine upload dist/*`;\n",
    ".github/workflows/release.yml":
      "on: workflow_dispatch\njobs:\n  release:\n    permissions:\n      id-token: write\n    steps:\n      - run: node scripts/publish-to-pypi.mjs\n",
  };
  for (const [name, contents] of Object.entries(files)) {
    await mkdir(path.dirname(path.join(root, name)), { recursive: true });
    await writeFile(path.join(root, name), contents);
  }
  const inspection = await inspectRepository(root);
  console.log(
    JSON.stringify({ inspection, plans: buildPlans(inspection) }, null, 2),
  );
} finally {
  await rm(root, { recursive: true, force: true });
}

// Regenerates a fixture's expected-inspection.json from the JavaScript
// implementation; review the diff, then check the Rust tests agree.
// Usage: node experiments/regenerate-expected-inspection.mjs <fixture> <owner/repo>
import { cp, mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import { inspectRepository } from "../js/src/discovery.mjs";

const [fixture, slug] = process.argv.slice(2);
const temporary = await mkdtemp(path.join(os.tmpdir(), "expected-"));
const repository = path.join(temporary, path.basename(fixture));
await cp(path.resolve(fixture), repository, { recursive: true });
await mkdir(path.join(repository, ".git"), { recursive: true });
await writeFile(
  path.join(repository, ".git/config"),
  `[remote "origin"]\n\turl = https://github.com/${slug}.git\n`,
);
const inspection = await inspectRepository(repository);
inspection.repository.root = "<ROOT>";
await writeFile(
  path.join(path.resolve(fixture), "expected-inspection.json"),
  `${JSON.stringify(inspection, null, 2)}\n`,
);
await rm(temporary, { recursive: true, force: true });

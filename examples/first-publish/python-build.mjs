// Demonstrates a selected Python with no build frontend; all writes are temporary.
import assert from "node:assert/strict";
import { mkdtemp, mkdir, writeFile, readdir, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { exec } from "../../js/node_modules/command-stream/src/$.mjs";
import { buildPythonDistribution } from "../../js/src/python.mjs";
const directory = await mkdtemp(path.join(os.tmpdir(), "prm-build-example-"));
try {
  const selected = path.join(directory, "selected");
  assert.equal(
    (await exec("python3", ["-m", "venv", selected], { capture: true })).code,
    0,
  );
  const python = path.join(
    selected,
    process.platform === "win32" ? "Scripts/python.exe" : "bin/python",
  );
  assert.notEqual(
    (await exec(python, ["-m", "build", "--version"], { capture: true })).code,
    0,
  );
  const project = path.join(directory, "project");
  await mkdir(project);
  await writeFile(
    path.join(project, "pyproject.toml"),
    '[build-system]\nrequires = ["setuptools>=68"]\nbuild-backend = "setuptools.build_meta"\n[project]\nname = "prm-build-example"\nversion = "0.1.0"\n',
  );
  await writeFile(path.join(project, "example.py"), "VALUE = 1\n");
  await buildPythonDistribution(python, project);
  const files = await readdir(path.join(project, "dist"));
  assert.ok(
    files.some((file) => file.endsWith(".whl")) &&
      files.some((file) => file.endsWith(".tar.gz")),
  );
  assert.notEqual(
    (await exec(python, ["-m", "build", "--version"], { capture: true })).code,
    0,
  );
  console.log(
    "Built wheel and sdist without installing build into the selected Python.",
  );
} finally {
  await rm(directory, { recursive: true, force: true });
}

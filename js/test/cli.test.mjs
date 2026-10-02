import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { createServer } from "node:http";
import path from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { promisify } from "node:util";

import { isDirectExecution } from "../src/cli.mjs";

const execute = promisify(execFile);
const here = path.dirname(fileURLToPath(import.meta.url));
const cli = path.resolve(here, "../src/cli.mjs");
const fixture = path.resolve(here, "../../tests/fixtures/polyglot");
const requests = [];
let server;
let registryEnv;

before(async () => {
  // A registry where nothing is published yet, so tests never touch the network.
  server = createServer((request, response) => {
    requests.push(request.url);
    response.writeHead(404, { "content-type": "application/json" });
    response.end("{}");
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const base = `http://127.0.0.1:${server.address().port}`;
  registryEnv = {
    ...process.env,
    PACKAGE_REGISTRY_MANAGER_NPM_REGISTRY: `${base}/npm`,
    PACKAGE_REGISTRY_MANAGER_CRATES_IO_API: `${base}/crates`,
    PACKAGE_REGISTRY_MANAGER_PYPI_API: `${base}/pypi`,
    PACKAGE_REGISTRY_MANAGER_DOCKER_HUB_API: `${base}/docker`,
  };
});

after(() => new Promise((resolve) => server.close(resolve)));

test("recognizes the CLI entry point as a portable file URL", () => {
  assert.equal(isDirectExecution(pathToFileURL(cli).href, cli), true);
  assert.equal(
    isDirectExecution(pathToFileURL(cli).href, `${cli}.different`),
    false,
  );
});

test("inspect emits machine-readable output", async () => {
  const { stdout } = await execute(process.execPath, [
    cli,
    "--repository",
    fixture,
    "--format",
    "json",
    "inspect",
    "--offline",
  ]);
  const inspection = JSON.parse(stdout);
  assert.equal(inspection.schema_version, 1);
  assert.equal(inspection.packages.length, 9);
  assert.equal(
    inspection.packages.some((item) => "exists_on_registry" in item),
    false,
  );
  assert.equal(inspection.repository.github_owner, "acme");
  assert.equal(inspection.repository.github_repository, "polyglot");
});

test("setup is safe by default", async () => {
  const { stdout } = await execute(
    process.execPath,
    [cli, "--repository", fixture, "setup", "--registry", "npm"],
    { env: registryEnv },
  );
  assert.match(stdout, /Dry run only/);
  assert.match(stdout, /\(bootstrap\)/);
  assert.match(stdout, /npm login --auth-type=web --browser=false/);
  assert.match(stdout, /npm publish \{tarball\} --access public/);
  assert.match(
    stdout,
    /npx -y npm@(?:\^12|\^11\.10) trust github @acme\/widgets/,
  );
  assert.match(stdout, /prerequisites:\n {4}- Node\.js: /);
  assert.match(stdout, /npm two-factor authentication: /);
  assert.match(stdout, /npm logout/);
  assert.ok(requests.includes("/npm/@acme%2Fwidgets/latest"));
});

test("plan reports registry state in JSON", async () => {
  const { stdout } = await execute(
    process.execPath,
    [cli, "--repository", fixture, "--format", "json", "plan"],
    { env: registryEnv },
  );
  const plans = JSON.parse(stdout);
  const npm = plans.find((plan) => plan.registry === "npm");
  assert.equal(npm.mode, "bootstrap");
  assert.equal(npm.package.exists_on_registry, false);
  assert.equal(npm.package.trusted_publishing, false);
  const dockerHub = plans.find((plan) => plan.registry === "docker-hub");
  assert.equal(dockerHub.mode, "bootstrap");
  assert.ok(dockerHub.steps.some((step) => step.id === "create-repository"));
});

test("rejects an unknown --browser mode", async () => {
  await assert.rejects(
    execute(
      process.execPath,
      [cli, "--repository", fixture, "--browser", "chromium", "plan"],
      { env: registryEnv },
    ),
    (error) => {
      assert.match(error.stderr, /--browser must be 'default' or 'automated'/);
      return true;
    },
  );
});

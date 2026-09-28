import assert from "node:assert/strict";
import {
  chmod,
  cp,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

import { authUrlScanner, nodeOptionsWithShim } from "../src/auth-urls.mjs";
import { inspectRepository } from "../src/discovery.mjs";
import { buildPlans } from "../src/plan.mjs";
import { probePackage, registryEndpoint } from "../src/registry-state.mjs";
import { executePlan } from "../src/setup.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
let temporary;
let repository;

before(async () => {
  temporary = await mkdtemp(path.join(os.tmpdir(), "prm-npm-bootstrap-"));
  repository = path.join(temporary, "pipeline-template");
  await cp(
    path.resolve(here, "../../tests/fixtures/pipeline-template"),
    repository,
    { recursive: true },
  );
});

after(async () => {
  await rm(temporary, { recursive: true, force: true });
});

async function npmPlan(state, options) {
  const inspection = await inspectRepository(repository);
  Object.assign(
    inspection.packages.find((item) => item.name === "pipeline-app"),
    state,
  );
  return buildPlans(inspection, ["npm"], options).find(
    (plan) => plan.package.name === "pipeline-app",
  );
}

const argv = (plan, id) => {
  const step = plan.steps.find((item) => item.id === id);
  return step && [step.command.program, ...step.command.args];
};

test("plans a bootstrap for a package missing from npm", async () => {
  const plan = await npmPlan({
    exists_on_registry: false,
    trusted_publishing: false,
  });
  assert.equal(plan.mode, "bootstrap");
  assert.deepEqual(
    plan.steps.map((step) => step.id),
    [
      "validate-package",
      "check-registry",
      "check-sign-in",
      "sign-in",
      "fetch-default-branch",
      "prepare-worktree",
      "pack",
      "first-publish",
      "wait-for-registry",
      "check-trust",
      "attach-trusted-publisher",
      "configure-trusted-publisher",
      "verify-trusted-publisher",
      "audit-token-secrets",
      "delete-token-secret",
      "sign-out",
      "remove-worktree",
    ],
  );
  assert.deepEqual(argv(plan, "sign-in"), [
    "npm",
    "login",
    "--auth-type=web",
    "--browser=false",
  ]);
  assert.deepEqual(argv(plan, "pack"), [
    "npm",
    "pack",
    "--ignore-scripts",
    "--json",
    "--pack-destination",
    "{pack_destination}",
  ]);
  assert.deepEqual(argv(plan, "first-publish"), [
    "npm",
    "publish",
    "{tarball}",
    "--access",
    "public",
    "--auth-type=web",
    "--browser=false",
    "--provenance=false",
  ]);
  assert.deepEqual(argv(plan, "attach-trusted-publisher"), [
    "npx",
    "-y",
    "npm@latest",
    "trust",
    "github",
    "pipeline-app",
    "--repo",
    "acme/pipeline-app",
    "--file",
    "release.yml",
    "--allow-publish",
    "--yes",
    "--browser=false",
  ]);
  assert.deepEqual(argv(plan, "check-trust"), [
    "npx",
    "-y",
    "npm@latest",
    "trust",
    "list",
    "pipeline-app",
    "--json",
  ]);
  assert.deepEqual(argv(plan, "sign-out"), ["npm", "logout"]);
});

test("plans only trusted-publisher attachment for an existing package", async () => {
  const plan = await npmPlan({
    exists_on_registry: true,
    trusted_publishing: false,
  });
  assert.equal(plan.mode, "attach");
  const ids = plan.steps.map((step) => step.id);
  for (const id of ["first-publish", "pack", "prepare-worktree"]) {
    assert.equal(ids.includes(id), false, `${id} must not be planned`);
  }
  assert.ok(ids.includes("attach-trusted-publisher"));
});

test("plans nothing once trusted publishing is in use", async () => {
  const plan = await npmPlan({
    exists_on_registry: true,
    trusted_publishing: true,
  });
  assert.equal(plan.mode, "complete");
  assert.deepEqual(plan.steps, []);
});

test("keeps every conditional step when the registry state is unknown", async () => {
  const plan = await npmPlan({}, { verifyRelease: true });
  assert.equal(plan.mode, undefined);
  assert.deepEqual(
    plan.steps.slice(-6, -2).map((step) => step.id),
    [
      "trigger-release",
      "find-release-run",
      "watch-release",
      "confirm-provenance",
    ],
  );
  assert.deepEqual(argv(plan, "watch-release"), [
    "gh",
    "run",
    "watch",
    "{run_id}",
    "--repo",
    "acme/pipeline-app",
    "--exit-status",
  ]);
});

test("probes npm existence and trusted publishing", async () => {
  const responses = new Map([
    ["https://registry.npmjs.org/missing/latest", [404, {}]],
    [
      "https://registry.npmjs.org/@acme%2Fwidgets/latest",
      [
        200,
        { version: "1.0.0", _npmUser: { trustedPublisher: { id: "github" } } },
      ],
    ],
    ["https://registry.npmjs.org/manual/latest", [200, { version: "2.0.0" }]],
    ["https://registry.npmjs.org/down/latest", [503, {}]],
  ]);
  const fetch = async (url) => {
    const [status, body] = responses.get(url);
    return { status, ok: status === 200, json: async () => body };
  };
  const probe = (name) =>
    probePackage({ registry: "npm", name }, { fetch, env: {} });
  assert.deepEqual(await probe("missing"), { exists: false, trusted: false });
  assert.deepEqual(await probe("@acme/widgets"), {
    exists: true,
    trusted: true,
    version: "1.0.0",
  });
  assert.deepEqual(await probe("manual"), {
    exists: true,
    trusted: false,
    version: "2.0.0",
  });
  assert.deepEqual(await probe("down"), {});
});

test("finds npm web-authentication URLs in streamed output", () => {
  const urls = [];
  const scan = authUrlScanner((url) => urls.push(url));
  scan("npm notice Log in on https://registry.npmjs.org/\nLogin at:\n");
  scan(
    "\u001b[1mhttps://www.npmjs.com/login?next=/login/cli/1\u001b[22m\nPress ENTER",
  );
  scan(
    " to open in the browser...\nAuthenticate your account at:\r\nhttps://www.npmjs.com/auth/cli/2\n",
  );
  scan("Authenticate your account at: https://www.npmjs.com/auth/cli/2\n");
  assert.deepEqual(urls, [
    "https://www.npmjs.com/login?next=/login/cli/1",
    "https://www.npmjs.com/auth/cli/2",
  ]);
  assert.equal(
    nodeOptionsWithShim("--max-old-space-size=64", 'C:\\a "b"\\shim.cjs'),
    '--max-old-space-size=64 --require "C:\\\\a \\"b\\"\\\\shim.cjs"',
  );
});

test("never reads, writes, or requests an npm token", async () => {
  const forbidden = [
    /_authToken/,
    /NODE_AUTH_TOKEN/,
    /\bnpm token\b/,
    /"token",\s*"create"/,
    /process\.env\.NPM_TOKEN/,
    /env::var\("NPM_TOKEN"\)/,
    /\.npmrc/,
  ];
  for (const directory of ["../src", "../../rust/src"]) {
    const root = path.resolve(here, directory);
    for (const name of await readdir(root)) {
      const contents = await readFile(path.join(root, name), "utf8");
      for (const pattern of forbidden) {
        assert.doesNotMatch(contents, pattern, `${directory}/${name}`);
      }
    }
  }
  const plan = await npmPlan({}, { verifyRelease: true });
  for (const step of plan.steps.filter((item) => item.command)) {
    const rendered = [step.command.program, ...step.command.args].join(" ");
    assert.doesNotMatch(rendered, /\btoken\b|NPM_TOKEN.*set/i, step.id);
  }
});

const FAKE_TOOL = path.resolve(
  here,
  "../../tests/fixtures/fake-tools/fake-tool.cjs",
);

async function installFakeTools(directory) {
  const bin = path.join(directory, "bin");
  await mkdir(bin, { recursive: true });
  for (const tool of ["npm", "npx", "git", "gh"]) {
    const file = path.join(bin, tool);
    await writeFile(
      file,
      `#!${process.execPath}\n${await readFile(FAKE_TOOL, "utf8")}`,
    );
    await chmod(file, 0o755);
  }
  return bin;
}

async function runWithFakeTools(plan, registry) {
  const lines = [];
  const originalLog = console.log;
  const originalPath = process.env.PATH;
  const state = process.env.FAKE_STATE;
  process.env.PATH = `${path.join(state, "bin")}${path.delimiter}${originalPath}`;
  console.log = (...values) => lines.push(values.join(" "));
  try {
    await executePlan(plan, {
      repository,
      execute: true,
      yes: true,
      noBrowser: true,
      verbose: false,
      pollIntervalMs: 1,
      fetch: async (url) => {
        const found = registry(url);
        return {
          status: found ? 200 : 404,
          ok: Boolean(found),
          json: async () => found,
        };
      },
    });
  } finally {
    console.log = originalLog;
    process.env.PATH = originalPath;
  }
  const log = (await readFile(path.join(state, "log.jsonl"), "utf8"))
    .trim()
    .split("\n")
    .map((line) => JSON.parse(line));
  await rm(path.join(state, "log.jsonl"));
  return { lines, log };
}

test(
  "runs the whole npm bootstrap with web sign-in and resumes safely",
  { skip: process.platform === "win32" && "fake tools are POSIX scripts" },
  async () => {
    const state = await mkdtemp(path.join(temporary, "state-"));
    process.env.FAKE_STATE = state;
    await installFakeTools(state);
    let published = false;
    const registry = (url) => {
      if (url.endsWith("/pipeline-app/0.1.0")) {
        published = true;
        return { version: "0.1.0" };
      }
      return published ? { version: "0.1.0" } : null;
    };
    const plan = await npmPlan({
      exists_on_registry: false,
      trusted_publishing: false,
    });
    const { lines, log } = await runWithFakeTools(plan, registry);
    const commands = log.map((entry) => entry.argv.join(" "));
    const worktree = commands
      .find((item) => item.startsWith("git worktree add"))
      .split(" ")[4];
    const destination = path.dirname(worktree);
    assert.deepEqual(commands, [
      "npm pkg get name version repository",
      "npm whoami",
      "npm login --auth-type=web --browser=false",
      "git fetch origin HEAD",
      `git worktree add --detach ${worktree} FETCH_HEAD`,
      `npm pack --ignore-scripts --json --pack-destination ${destination}`,
      `npm publish ${destination}/pipeline-app-0.1.0.tgz --access public --auth-type=web --browser=false --provenance=false`,
      "npx -y npm@latest trust list pipeline-app --json",
      "npx -y npm@latest trust github pipeline-app --repo acme/pipeline-app --file release.yml --allow-publish --yes --browser=false",
      "npx -y npm@latest trust list pipeline-app --json",
      "gh secret list --repo acme/pipeline-app --json name",
      "gh secret delete NPM_TOKEN --repo acme/pipeline-app",
      "npm logout",
      `git worktree remove --force ${worktree}`,
    ]);
    const login = log.find((entry) => entry.argv[1] === "login");
    assert.equal(login.shim && login.tty, true, "npm sees a TTY on stdout");
    assert.ok(
      log
        .find((entry) => entry.argv[1] === "pack")
        .cwd.endsWith(path.join(path.basename(destination), "worktree")),
      "npm pack runs inside the temporary worktree",
    );
    assert.ok(
      lines.includes("Open https://www.npmjs.com/login?next=/login/cli/fake"),
    );
    assert.ok(lines.includes("Open https://www.npmjs.com/auth/cli/fake"));
    assert.ok(
      lines.some((line) => line.includes("pipeline-app-0.1.0.tgz: 120 bytes")),
    );
    await assert.rejects(stat(destination), "temporary files are removed");

    const resumed = await runWithFakeTools(
      await npmPlan({ exists_on_registry: true, trusted_publishing: false }),
      () => ({ version: "0.1.0" }),
    );
    const again = resumed.log.map((entry) => entry.argv.join(" "));
    assert.equal(
      again.some((item) => item.startsWith("npm publish")),
      false,
    );
    assert.equal(
      again.some((item) => item.includes("trust github")),
      false,
    );
    assert.ok(
      again.includes("npx -y npm@latest trust list pipeline-app --json"),
    );
  },
);

test("trims trailing slashes from endpoint overrides in linear time", () => {
  const variable = "PACKAGE_REGISTRY_MANAGER_NPM_REGISTRY";
  assert.equal(
    registryEndpoint("npm", { [variable]: "http://mirror.test/npm///" }),
    "http://mirror.test/npm",
  );
  const hostile = `${"/".repeat(100_000)}x`;
  const started = performance.now();
  assert.equal(registryEndpoint("npm", { [variable]: hostile }), hostile);
  assert.ok(performance.now() - started < 1000);
});

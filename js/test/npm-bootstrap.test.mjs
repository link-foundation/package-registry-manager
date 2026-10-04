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
import { TWO_FACTOR_HINT } from "../src/approvals.mjs";

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
      "fetch-default-branch",
      "prepare-worktree",
      "pack",
      "test-install",
      "verify-bins",
      "sign-in",
      "check-2fa",
      "enable-2fa",
      "verify-2fa",
      "first-publish",
      "wait-for-registry",
      "check-trust",
      "inspect-release-run",
      "read-release-failure",
      "attach-trusted-publisher",
      "configure-trusted-publisher",
      "verify-trusted-publisher",
      "audit-token-secrets",
      "delete-token-secret",
      "rerun-release",
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
    "npm@^11.10",
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
    "npm@^11.10",
    "trust",
    "list",
    "pipeline-app",
    "--browser=false",
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
  for (const tool of [
    "npm",
    "npx",
    "git",
    "gh",
    "open",
    "xdg-open",
    "defaults",
    "xdg-settings",
  ]) {
    const file = path.join(bin, tool);
    await writeFile(
      file,
      `#!${process.execPath}\n${await readFile(FAKE_TOOL, "utf8")}`,
    );
    await chmod(file, 0o755);
  }
  return bin;
}

async function runWithFakeTools(plan, registry, overrides = {}) {
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
      // Browser steps wait for Enter; answer at once.
      prompt: async () => "",
      fetch: async (url) => {
        const found = registry(url);
        return {
          status: found ? 200 : 404,
          ok: Boolean(found),
          json: async () => found,
        };
      },
      ...overrides,
    });
  } catch (error) {
    error.lines = lines;
    throw error;
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
      "git fetch origin HEAD",
      `git worktree add --detach ${worktree} FETCH_HEAD`,
      `npm pack --ignore-scripts --json --pack-destination ${destination}`,
      `npm install --no-save --no-package-lock --no-audit --no-fund --ignore-scripts --prefix ${destination}/install ${destination}/pipeline-app-0.1.0.tgz`,
      "npm login --auth-type=web --browser=false",
      "npm profile get --json",
      `npm publish ${destination}/pipeline-app-0.1.0.tgz --access public --auth-type=web --browser=false --provenance=false`,
      "npx -y npm@^11.10 trust list pipeline-app --browser=false",
      "gh run list --repo acme/pipeline-app --workflow release.yml --limit 1 --json databaseId,conclusion,status,url",
      "npx -y npm@^11.10 trust github pipeline-app --repo acme/pipeline-app --file release.yml --allow-publish --yes --browser=false",
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
      lines.includes("Open https://www.npmjs.com/auth/cli/fake-trust"),
      "npm trust list's approval link is relayed (#24)",
    );
    assert.ok(lines.includes("  npm stored the trusted publisher."));
    assert.ok(
      lines.some((line) =>
        /^Sign in within about 5 minutes \(until \d\d:\d\d\)\.$/.test(line),
      ),
    );
    assert.ok(
      lines.some((line) => /^Approve within about 5 minutes/.test(line)),
    );
    assert.ok(lines.includes(TWO_FACTOR_HINT));
    assert.ok(
      lines.some((line) =>
        line.includes(
          "Future releases publish from release.yml through trusted publishing; no login is needed.",
        ),
      ),
    );
    assert.ok(
      lines.some((line) => line.includes("pipeline-app-0.1.0.tgz: 120 bytes")),
    );
    assert.ok(
      lines.includes("  Two-factor authentication is on (auth-and-writes)."),
    );
    assert.ok(
      lines.includes("  pipeline-app --version: 0.1.0"),
      lines.join("\n"),
    );
    assert.ok(
      lines.includes(
        "  The latest release run success: https://github.com/acme/pipeline-app/actions/runs/41",
      ),
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
      again.includes(
        "npx -y npm@^11.10 trust list pipeline-app --browser=false",
      ),
    );
  },
);

async function readLog(state) {
  const file = path.join(state, "log.jsonl");
  const text = await readFile(file, "utf8").catch(() => "");
  return text
    .trim()
    .split("\n")
    .filter(Boolean)
    .map((line) => JSON.parse(line));
}

test(
  "opens npm web-authentication URLs in the default browser",
  { skip: process.platform === "win32" && "fake tools are POSIX scripts" },
  async () => {
    const state = await mkdtemp(path.join(temporary, "state-"));
    process.env.FAKE_STATE = state;
    await installFakeTools(state);
    const plan = await npmPlan({
      exists_on_registry: false,
      trusted_publishing: false,
    });
    plan.steps = plan.steps.filter((step) =>
      ["check-sign-in", "sign-in"].includes(step.id),
    );
    const profile = path.join(state, "unused-profile");
    const { lines, log } = await runWithFakeTools(plan, () => ({}), {
      noBrowser: false,
      browser: "default",
      browserProfile: profile,
    });
    assert.ok(
      lines.includes(
        "Opening https://www.npmjs.com/login?next=/login/cli/fake in Firefox, your default browser",
      ),
    );
    // The opener is detached, so wait for it to record its arguments.
    const opener = (entry) => ["open", "xdg-open"].includes(entry.argv[0]);
    let opened = log.find(opener);
    for (let attempt = 0; attempt < 200 && !opened; attempt += 1) {
      opened = (await readLog(state)).find(opener);
      if (!opened) {
        await new Promise((resolve) => setTimeout(resolve, 25));
      }
    }
    assert.deepEqual(opened.argv.slice(1), [
      "https://www.npmjs.com/login?next=/login/cli/fake",
    ]);
    await assert.rejects(stat(profile), "the automation profile is not used");
  },
);

const POSIX_ONLY = {
  skip: process.platform === "win32" && "fake tools are POSIX scripts",
};

const signInSteps = (plan) => {
  plan.steps = plan.steps.filter((step) =>
    ["check-sign-in", "sign-in", "sign-out"].includes(step.id),
  );
  return plan;
};
const missing = { exists_on_registry: false, trusted_publishing: false };

test(
  "requests a fresh login link when npm falls back to its username prompt (#17)",
  POSIX_ONLY,
  async () => {
    await freshState();
    const plan = signInSteps(await npmPlan(missing));
    const started = Date.now();
    const { lines, log } = await withEnv({ FAKE_LEGACY_LOGIN: "2" }, () =>
      runWithFakeTools(plan, () => ({})),
    );
    assert.ok(Date.now() - started < 20_000, "the prompt is not awaited");
    const logins = log.filter((entry) => entry.argv[1] === "login");
    assert.equal(logins.length, 3);
    for (const attempt of [2, 3]) {
      assert.ok(
        lines.some((line) =>
          line.includes(
            `npm fell back to its legacy username prompt); requesting a fresh one (attempt ${attempt} of 3)`,
          ),
        ),
        lines.join("\n"),
      );
    }
    assert.ok(log.some((entry) => entry.argv[1] === "logout"));
  },
);

test(
  "gives up after three expired login links with a clear message (#17)",
  POSIX_ONLY,
  async () => {
    await freshState();
    const plan = signInSteps(await npmPlan(missing));
    await withEnv({ FAKE_LEGACY_LOGIN: "3" }, () =>
      assert.rejects(
        runWithFakeTools(plan, () => ({})),
        /the browser link expired 3 times \(npm fell back to its legacy username prompt\); re-run the command when you are ready to approve within 5 minutes/,
      ),
    );
  },
);

test(
  "re-runs npm publish with a fresh approval link once one expired (#17)",
  POSIX_ONLY,
  async () => {
    await freshState();
    const plan = await npmPlan(missing);
    // Publish from the repository itself, without packing first.
    plan.steps = plan.steps
      .filter((step) => ["sign-in", "first-publish"].includes(step.id))
      .map((step) => ({ ...step, cwd: "." }));
    const { lines, log } = await withEnv({ FAKE_EXPIRED_PUBLISH: "1" }, () =>
      runWithFakeTools(plan, () => null),
    );
    const publishes = log.filter((entry) => entry.argv[1] === "publish");
    assert.equal(publishes.length, 2);
    assert.ok(
      lines.some((line) =>
        line.includes(
          "npm's approval session ended); requesting a fresh one (attempt 2 of 3)",
        ),
      ),
      lines.join("\n"),
    );
  },
);

test(
  "keeps the npm session with --keep-session (#17)",
  POSIX_ONLY,
  async () => {
    const state = await freshState();
    const plan = signInSteps(await npmPlan(missing));
    const { lines, log } = await runWithFakeTools(plan, () => ({}), {
      keepSession: true,
    });
    assert.equal(
      log.some((entry) => entry.argv[1] === "logout"),
      false,
    );
    assert.ok(lines.some((line) => line.startsWith("Keeping the npm session")));
    assert.equal(await readFile(path.join(state, "session"), "utf8"), "");
  },
);

test(
  "opens approval links in the application chosen with --open-with (#17)",
  POSIX_ONLY,
  async () => {
    const state = await freshState();
    const plan = signInSteps(await npmPlan(missing));
    const url = "https://www.npmjs.com/login?next=/login/cli/fake";
    const { lines, log } = await runWithFakeTools(plan, () => ({}), {
      noBrowser: false,
      browser: "default",
      openWith: "xdg-open",
    });
    assert.ok(lines.includes(`Opening ${url} in xdg-open`), lines.join("\n"));
    const opener = (entry) => ["open", "xdg-open"].includes(entry.argv[0]);
    let opened = log.find(opener);
    for (let attempt = 0; attempt < 200 && !opened; attempt += 1) {
      opened = (await readLog(state)).find(opener);
      if (!opened) {
        await new Promise((resolve) => setTimeout(resolve, 25));
      }
    }
    assert.ok(opened.argv.includes(url));
  },
);

async function withEnv(values, run) {
  const saved = Object.fromEntries(
    Object.keys(values).map((key) => [key, process.env[key]]),
  );
  Object.assign(process.env, values);
  try {
    return await run();
  } finally {
    for (const [key, value] of Object.entries(saved)) {
      if (value === undefined) {
        delete process.env[key];
      } else {
        process.env[key] = value;
      }
    }
  }
}

async function freshState() {
  const state = await mkdtemp(path.join(temporary, "state-"));
  process.env.FAKE_STATE = state;
  await installFakeTools(state);
  return state;
}

test(
  "opens the npm 2FA settings and stops before publishing while 2FA is off",
  POSIX_ONLY,
  async () => {
    const state = await freshState();
    const plan = await npmPlan({
      exists_on_registry: false,
      trusted_publishing: false,
    });
    const error = await withEnv({ FAKE_TFA: "off" }, () =>
      runWithFakeTools(plan, () => null).catch((failure) => failure),
    );
    assert.match(error.message, /two-factor authentication is still off/);
    const { lines } = error;
    assert.ok(
      lines.includes(
        "Open https://docs.npmjs.com/configuring-two-factor-authentication/",
      ),
    );
    const commands = (await readLog(state)).map((entry) =>
      entry.argv.join(" "),
    );
    assert.equal(
      commands.filter((item) => item === "npm profile get --json").length,
      2,
    );
    for (const prefix of ["npm publish", "npx"]) {
      assert.equal(
        commands.some((item) => item.startsWith(prefix)),
        false,
        prefix,
      );
    }
  },
);

test(
  "reports npm's pack corrections and stops when a bin was removed",
  POSIX_ONLY,
  async () => {
    const state = await freshState();
    const plan = await npmPlan({
      exists_on_registry: false,
      trusted_publishing: false,
    });
    const error = await withEnv({ FAKE_PACK_WARNINGS: "1" }, () =>
      runWithFakeTools(plan, () => null).catch((failure) => failure),
    );
    assert.match(error.message, /packed package\.json has no bin pipeline-app/);
    const { lines } = error;
    assert.ok(
      lines.includes(
        '    npm warn pack "bin[pipeline-app]" script name bin/cli.js was invalid and removed',
      ),
      lines.join("\n"),
    );
    assert.ok(
      lines.includes(
        "    npm warn pack npm auto-corrected some errors in your package.json when publishing.",
      ),
    );
    assert.equal(
      lines.some((line) => /pkg fix/.test(line)),
      false,
      "never suggests npm pkg fix",
    );
    const commands = (await readLog(state)).map((entry) =>
      entry.argv.join(" "),
    );
    assert.ok(commands.some((item) => item.startsWith("npm install")));
    assert.equal(
      commands.some((item) => /^npm (publish|pkg fix)/.test(item)),
      false,
    );
  },
);

test("keeps npm's warnings without its npm pkg fix advice", async () => {
  const { packWarnings } = await import("../src/setup.mjs");
  assert.deepEqual(
    packWarnings(
      '\u001b[33mnpm warn\u001b[39m pack auto-corrected.  Please run "npm pkg fix" to address these errors.\nnpm notice 1 file\nnpm warn run npm pkg fix\n',
    ),
    ["npm warn pack auto-corrected."],
  );
});

test(
  "stops when an installed bin fails to run with --version",
  POSIX_ONLY,
  async () => {
    await freshState();
    const plan = await npmPlan({
      exists_on_registry: false,
      trusted_publishing: false,
    });
    await withEnv({ FAKE_BIN_FAILS: "1" }, () =>
      assert.rejects(
        runWithFakeTools(plan, () => null),
        /bin pipeline-app \(bin\/cli\.js\) exited with status 1/,
      ),
    );
  },
);

test(
  "stops when an installed bin runs without printing its version (#22)",
  POSIX_ONLY,
  async () => {
    await freshState();
    const plan = await npmPlan({
      exists_on_registry: false,
      trusted_publishing: false,
    });
    await withEnv({ FAKE_BIN_SILENT: "1" }, () =>
      assert.rejects(
        runWithFakeTools(plan, () => null),
        /bin pipeline-app \(bin\/cli\.js\) printed nothing when run with --version/,
      ),
    );
  },
);

test("reads npm pack output before and since npm 12 (#22)", async () => {
  const { packedEntry } = await import("../src/setup.mjs");
  const packed = { filename: "a-1.0.0.tgz", version: "1.0.0" };
  assert.deepEqual(packedEntry(JSON.stringify([packed])), packed);
  assert.deepEqual(packedEntry(JSON.stringify({ a: packed })), packed);
  assert.throws(() => packedEntry("{}"), /npm pack printed no package/);
});

test(
  "packs and installs the tarball with npm 12's pack output (#22)",
  POSIX_ONLY,
  async () => {
    await freshState();
    const plan = await npmPlan({
      exists_on_registry: false,
      trusted_publishing: false,
    });
    // The failing bin stops the run right after the tarball was installed.
    const error = await withEnv(
      { FAKE_PACK_JSON: "object", FAKE_BIN_FAILS: "1" },
      () => runWithFakeTools(plan, () => null).catch((failure) => failure),
    );
    assert.match(error.message, /bin pipeline-app \(bin\/cli\.js\) exited/);
    assert.ok(
      error.lines.includes(
        "  pipeline-app-0.1.0.tgz: 120 bytes packed, 300 bytes unpacked, 1 files",
      ),
      error.lines.join("\n"),
    );
  },
);

test(
  "re-runs a release that npm rejected before trust and watches it",
  POSIX_ONLY,
  async () => {
    await freshState();
    const plan = await npmPlan(
      { exists_on_registry: true, trusted_publishing: false },
      { verifyRelease: true },
    );
    let calls = 0;
    const registry = () =>
      (calls += 1) === 1
        ? { version: "0.1.0" }
        : {
            version: "0.1.1",
            _npmUser: { trustedPublisher: { id: "github" } },
            dist: { attestations: {} },
          };
    const { lines, log } = await withEnv(
      { FAKE_RELEASE_RUN: "failed-publish" },
      () => runWithFakeTools(plan, registry),
    );
    const commands = log.map((entry) => entry.argv.join(" "));
    assert.ok(
      commands.includes("gh run view 42 --repo acme/pipeline-app --log-failed"),
    );
    assert.ok(
      commands.includes("gh run rerun 42 --repo acme/pipeline-app --failed"),
    );
    assert.ok(
      commands.includes(
        "gh run watch 42 --repo acme/pipeline-app --exit-status",
      ),
    );
    assert.equal(
      commands.some((item) => item.startsWith("gh workflow run")),
      false,
      "the re-run is watched instead of a new dispatch",
    );
    assert.ok(
      lines.some((line) => line.includes("E404/invalid-publisher")),
      lines.join("\n"),
    );
  },
);

test("reads trusted publishers from npm trust list output (#24)", async () => {
  const { listsTrustedPublisher } = await import("../src/setup.mjs");
  const human =
    "Authenticate your account at:\nhttps://www.npmjs.com/auth/cli/x\n\ntype: \u001b[32mgithub\u001b[39m\nid: \u001b[32m5c6fb388\u001b[39m\nfile: \u001b[32mrelease.yml\u001b[39m\n";
  assert.equal(listsTrustedPublisher(human), true);
  assert.equal(listsTrustedPublisher('{"type":"github","file":"a.yml"}'), true);
  assert.equal(
    listsTrustedPublisher(
      "Authenticate your account at:\nhttps://www.npmjs.com/auth/cli/x\nNo trust configurations found for package (a)\n",
    ),
    false,
  );
  assert.equal(listsTrustedPublisher(""), false);
});

test(
  "signs out and removes the worktree when a middle step fails (#24)",
  POSIX_ONLY,
  async () => {
    await freshState();
    const plan = await npmPlan(missing);
    const registry = (url) =>
      url.endsWith("/pipeline-app/0.1.0") ? { version: "0.1.0" } : null;
    // npm trust github fails, so verify-trusted-publisher throws.
    const error = await withEnv({ FAKE_TRUST_GITHUB: "fail" }, () =>
      runWithFakeTools(plan, registry).catch((failure) => failure),
    );
    assert.match(
      error.message,
      /npm does not list a trusted publisher for the package/,
    );
    const commands = (await readLog(process.env.FAKE_STATE)).map((entry) =>
      entry.argv.join(" "),
    );
    const failed = commands.lastIndexOf(
      "npx -y npm@^11.10 trust list pipeline-app --browser=false",
    );
    assert.deepEqual(commands.slice(failed + 1, failed + 2), ["npm logout"]);
    assert.match(commands[failed + 2], /^git worktree remove --force /);
    const worktree = commands[failed + 2].split(" ")[4];
    await assert.rejects(
      stat(path.dirname(worktree)),
      "temporary files are removed",
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

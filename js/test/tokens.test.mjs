import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  chmod,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  writeFile,
} from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { after, test } from "node:test";

import { cratesFlow, pypiFlow } from "../src/flows.mjs";
import { buildPlans } from "../src/plan.mjs";
import { executePlan } from "../src/setup.mjs";
import {
  auditRegistryTokens,
  cargoHome,
  registryToken,
  tokenState,
  tokenSecretSteps,
  verifyTokenRevoked,
} from "../src/tokens.mjs";

const FAKE_TOOL = fileURLToPath(
  new URL("../../tests/fixtures/fake-tools/fake-tool.cjs", import.meta.url),
);
const temporary = await mkdtemp(path.join(os.tmpdir(), "prm-tokens-"));

after(() => rm(temporary, { recursive: true, force: true }));

const crate = {
  registry: "crates-io",
  name: "demo",
  manifest: "Cargo.toml",
  publishable: true,
  exists_on_registry: false,
  trusted_publishing: false,
  workflow: "release.yml",
};
const ids = (steps) => steps.map((step) => step.id);
const rejected = { errors: [{ detail: "authentication failed" }] };
const websiteOnly = {
  errors: [
    { detail: "this action can only be performed on the crates.io website" },
  ],
};

test("requires a verified OIDC release before unused-secret deletion", async () => {
  for (const registry of [
    "npm",
    "pypi",
    "crates-io",
    "rubygems",
    "nuget",
    "jsr",
  ]) {
    const steps = tokenSecretSteps({ registry }, "acme/demo");
    assert.deepEqual(ids(steps), [
      "audit-token-secrets",
      "confirm-oidc-cleanup",
      "delete-token-secret",
    ]);
    assert.equal(steps[1].when, "token-secret-present");
    await assert.rejects(
      executePlan(
        {
          registry,
          package: { ...crate, registry },
          steps: [{ ...steps[1], when: undefined }],
        },
        {
          repository: temporary,
          execute: true,
          yes: true,
          noBrowser: true,
          prompt: async () => "n",
        },
      ),
      /verified OIDC release is required/,
    );
  }
});

test("splits token secrets into ones still read and leftovers (#16)", () => {
  const output = JSON.stringify([
    { name: "CARGO_TOKEN" },
    { name: "CARGO_REGISTRY_TOKEN" },
    { name: "NPM_TOKEN" },
    { name: "DOCKERHUB_TOKEN" },
  ]);
  assert.deepEqual(
    auditRegistryTokens(output, { ...crate, token_secrets: ["CARGO_TOKEN"] }),
    { inUse: ["CARGO_TOKEN"], leftover: ["CARGO_REGISTRY_TOKEN"] },
  );
  assert.deepEqual(auditRegistryTokens(output, { registry: "npm" }), {
    inUse: [],
    leftover: ["NPM_TOKEN"],
  });
  assert.deepEqual(auditRegistryTokens("", { registry: "pypi" }), {
    inUse: [],
    leftover: [],
  });
});

test("reads the token from cargo's [registry] table only", () => {
  assert.equal(
    registryToken(
      '[registries.other]\ntoken = "other"\n\n[registry] # crates.io\ntoken = "cio-1"\n',
    ),
    "cio-1",
  );
  assert.equal(registryToken("[registry]\ntoken = 'cio-2'\r\n"), "cio-2");
  assert.equal(registryToken('[registries.x]\ntoken = "x"\n'), undefined);
  assert.equal(cargoHome({ CARGO_HOME: "/c" }, "/h"), "/c");
  assert.equal(cargoHome({}, "/h"), path.join("/h", ".cargo"));
});

test("classifies crates.io answers to a token", () => {
  assert.equal(tokenState(403, rejected), "revoked");
  assert.equal(
    tokenState(401, {
      errors: [{ detail: "The given API token does not match the format" }],
    }),
    "revoked",
  );
  assert.equal(tokenState(403, websiteOnly), "active");
  assert.equal(tokenState(200, {}), "active");
  assert.equal(tokenState(500, undefined), undefined);
  assert.equal(
    tokenState(403, {
      errors: [{ detail: "this action requires authentication" }],
    }),
    undefined,
  );
});

test("plans the first-publish token as a confirmed, verified exception", () => {
  const context = {
    directory: ".",
    slug: "acme/demo",
    workflow: "release.yml",
    manual: true,
  };
  const steps = cratesFlow(crate, context);
  assert.deepEqual(ids(steps), [
    "validate-package",
    "check-registry",
    "fetch-default-branch",
    "prepare-worktree",
    "create-publish-token",
    "sign-in",
    "first-publish",
    "wait-for-registry",
    "revoke-publish-token",
    "verify-token-revoked",
    "configure-trusted-publisher",
    "audit-token-secrets",
    "confirm-oidc-cleanup",
    "delete-token-secret",
    "sign-out",
    "remove-worktree",
  ]);
  const create = steps.find((step) => step.id === "create-publish-token");
  assert.equal(create.confirm, true);
  assert.match(create.description, /one-time exception/);
  assert.deepEqual(
    ids(cratesFlow(crate, { ...context, slug: null })).filter((id) =>
      id.includes("secret"),
    ),
    [],
  );
  assert.deepEqual(ids(pypiFlow({ ...crate, registry: "pypi" }, context)), [
    "build-package",
    "check-registry",
    "create-pending-publisher",
    "trigger-release",
    "wait-for-registry",
    "configure-trusted-publisher",
    "audit-token-secrets",
    "confirm-oidc-cleanup",
    "delete-token-secret",
  ]);
});

function commandsSoFar(state) {
  return readFileSync(path.join(state, "log.jsonl"), "utf8")
    .trim()
    .split("\n")
    .map((line) => JSON.parse(line).argv.join(" "));
}

async function cargoCredentials(contents) {
  const directory = await mkdtemp(path.join(temporary, "cargo-"));
  if (contents !== undefined) {
    await writeFile(path.join(directory, "credentials.toml"), contents);
  }
  return directory;
}

function fakeCratesIo(answers, seen = []) {
  return async (url, init) => {
    seen.push({ url, authorization: init?.headers?.authorization });
    const [status, body] = answers.length > 1 ? answers.shift() : answers[0];
    return { status, ok: status < 300, json: async () => body };
  };
}

test("asks to revoke a still active token, then confirms it", async () => {
  const seen = [];
  const prompts = [];
  await verifyTokenRevoked(async (message) => prompts.push(message), {
    env: {
      CARGO_HOME: await cargoCredentials('[registry]\ntoken = "cio-1"\n'),
      PACKAGE_REGISTRY_MANAGER_CRATES_IO_API: "https://crates.test/api/v1",
    },
    fetch: fakeCratesIo(
      [
        [403, websiteOnly],
        [403, rejected],
      ],
      seen,
    ),
  });
  assert.deepEqual(seen, [
    { url: "https://crates.test/api/v1/me/tokens", authorization: "cio-1" },
    { url: "https://crates.test/api/v1/me/tokens", authorization: "cio-1" },
  ]);
  assert.equal(prompts.length, 1);
  assert.match(prompts[0], /still accepts the first-publish token/);
});

test("fails when the token stays active, and warns without a token", async () => {
  const env = {
    CARGO_HOME: await cargoCredentials('[registry]\ntoken = "cio-1"\n'),
  };
  await assert.rejects(
    verifyTokenRevoked(async () => "", {
      env,
      fetch: fakeCratesIo([[403, websiteOnly]]),
    }),
    /still authenticates on crates\.io; revoke it at https:\/\/crates\.io\/settings\/tokens/,
  );
  const fetched = [];
  const warnings = [];
  const originalError = console.error;
  console.error = (line) => warnings.push(line);
  try {
    await verifyTokenRevoked(async () => "", {
      env: { CARGO_HOME: await cargoCredentials() },
      fetch: fakeCratesIo([[403, rejected]], fetched),
    });
  } finally {
    console.error = originalError;
  }
  assert.equal(fetched.length, 0);
  assert.match(warnings[0], /no readable crates\.io token/);
});

test(
  "bootstraps a crate, verifies the token revocation, and deletes leftover secrets",
  { skip: process.platform === "win32" && "fake tools are POSIX scripts" },
  async () => {
    const state = await mkdtemp(path.join(temporary, "state-"));
    const bin = path.join(state, "bin");
    await mkdir(bin);
    for (const tool of ["cargo", "git", "gh"]) {
      const file = path.join(bin, tool);
      await writeFile(
        file,
        `#!${process.execPath}\n${await readFile(FAKE_TOOL, "utf8")}`,
      );
      await chmod(file, 0o755);
    }
    const repository = await mkdtemp(path.join(temporary, "repository-"));
    await writeFile(
      path.join(repository, "Cargo.toml"),
      '[package]\nname = "demo"\n',
    );
    const [plan] = buildPlans(
      {
        repository: { github_owner: "acme", github_repository: "demo" },
        packages: [{ ...crate, token_secrets: ["CARGO_TOKEN"] }],
      },
      ["crates-io"],
      { manual: true },
    );
    const saved = { ...process.env };
    const cargo = await cargoCredentials();
    Object.assign(process.env, {
      PATH: `${bin}${path.delimiter}${process.env.PATH}`,
      FAKE_STATE: state,
      FAKE_REPO_TOKEN_NAMES: "CARGO_TOKEN,CARGO_REGISTRY_TOKEN,GITHUB_TOKEN",
      CARGO_HOME: cargo,
    });
    const lines = [];
    const originalLog = console.log;
    console.log = (...values) => lines.push(values.join(" "));
    const prompts = [];
    let revoked = false;
    try {
      await executePlan(plan, {
        repository,
        execute: true,
        noBrowser: true,
        pollIntervalMs: 1,
        prompt: async (message) => {
          prompts.push(message);
          // The maintainer revokes the token while the tool waits.
          revoked ||= message.includes("still accepts");
          return message.endsWith("[y/N] ") ? "y" : "";
        },
        fetch: async (url) => {
          if (url.endsWith("/me/tokens")) {
            const body = revoked ? rejected : websiteOnly;
            return { status: 403, ok: false, json: async () => body };
          }
          // Missing at the check, visible once cargo published it.
          const published = commandsSoFar(state).includes("cargo publish");
          return published
            ? { status: 200, ok: true, json: async () => ({}) }
            : { status: 404, ok: false, json: async () => ({}) };
        },
      });
    } finally {
      console.log = originalLog;
      process.env = saved;
    }
    const commands = commandsSoFar(state);
    assert.deepEqual(
      commands.filter((item) => !item.startsWith("git")),
      [
        "cargo publish --dry-run",
        "cargo login",
        "cargo publish",
        "gh secret list --repo acme/demo --json name",
        "gh secret delete CARGO_REGISTRY_TOKEN --repo acme/demo",
        "cargo logout",
      ],
    );
    assert.equal(prompts[0], "Create a one-time first-publish token? [y/N] ");
    assert.ok(
      prompts.some((item) => item.includes("still accepts")),
      prompts.join("\n"),
    );
    assert.ok(
      lines.includes(
        "  crates.io rejects the first-publish token: it is revoked.",
      ),
    );
    assert.ok(
      lines.includes(
        "  CARGO_TOKEN is still read by a workflow; switch that workflow to trusted publishing before deleting it.",
      ),
    );
    await assert.rejects(readFile(path.join(cargo, "credentials.toml")));
  },
);

test("stops before any upload when the one-time token is declined", async () => {
  const [plan] = buildPlans(
    { repository: {}, packages: [crate] },
    ["crates-io"],
    { manual: true },
  );
  const create = plan.steps.findIndex(
    (step) => step.id === "create-publish-token",
  );
  await assert.rejects(
    executePlan(
      { ...plan, steps: plan.steps.slice(create) },
      {
        repository: temporary,
        execute: true,
        noBrowser: true,
        prompt: async () => "n",
      },
    ),
    /one-time first-publish token was declined; nothing was published/,
  );
});

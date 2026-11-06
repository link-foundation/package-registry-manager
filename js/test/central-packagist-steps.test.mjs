import assert from "node:assert/strict";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { inspectRepository } from "../src/discovery.mjs";
import { buildPlans } from "../src/plan.mjs";
import { SetupSession } from "../src/setup.mjs";
import { CENTRAL_PACKAGIST_STEPS } from "../src/central-packagist-steps.mjs";

const root = fileURLToPath(
  new URL("../../tests/fixtures/links-notation", import.meta.url),
);
const inspection = await inspectRepository(root);
const repository = {
  github_owner: "link-foundation",
  github_repository: "links-notation",
};

function plan(registry) {
  const found = buildPlans(inspection, [registry]).find(
    (entry) =>
      entry.registry === registry &&
      entry.package.publishable &&
      (registry !== "maven-central" || entry.package.gradle),
  );
  return { ...found, repository: { ...found.repository, ...repository } };
}

const response = (status, body) => ({
  status,
  ok: status >= 200 && status < 300,
  json: async () => body,
});

/** A gh-manager double recording every secret write; values never logged. */
function fakeGithub({
  secrets = {},
  runs = [],
  jobs = [],
  logs = {},
  files = {},
} = {}) {
  const writes = [];
  const verifies = [];
  const variables = new Map();
  return {
    variables: () => ({
      get: async (name) => variables.get(name) ?? null,
      ensure: async (name, value) => {
        variables.set(name, { value });
        return { verified: true };
      },
    }),
    writes,
    verifies,
    secrets: (scope) => ({
      getMetadata: async (name) => {
        if (scope.org) {
          const error = new Error("forbidden");
          error.status = 403;
          throw error;
        }
        return secrets[name] ? { name } : null;
      },
      set: async (name, value, options = {}) => {
        writes.push({
          name,
          length: value.length,
          expiresAt: options.expiresAt,
        });
        if (options.expiresAt)
          variables.set(`${name}_EXPIRES_AT`, { value: options.expiresAt });
        secrets[name] = true;
        return { name, changed: true };
      },
    }),
    rest: {
      request: async (path, { allowNotFound } = {}) => {
        if (path in files) {
          return files[path];
        }
        if (/^\/repos\/[^/]+\/[^/]+$/.test(path)) {
          return { default_branch: "main" };
        }
        if (allowNotFound) {
          return null;
        }
        throw Object.assign(new Error(`GitHub API 404 for ${path}`), {
          status: 404,
        });
      },
    },
    runs: {
      list: async () => runs,
      jobs: async () => jobs,
      jobLog: async (_slug, id) => logs[id] ?? null,
    },
    repair: {
      verify: async (request) => {
        verifies.push(request);
        return {
          status: "verified",
          runUrl: "https://github.com/x/y/actions/runs/2",
        };
      },
    },
  };
}

function session(registry, options = {}) {
  const fetched = [];
  const routes = options.routes ?? {};
  const instance = new SetupSession(plan(registry), {
    yes: false,
    fetch: async (url, init = {}) => {
      fetched.push(`${init.method ?? "GET"} ${url}`);
      const hit = Object.entries(routes).find(([pattern]) =>
        url.includes(pattern),
      );
      return hit ? response(...hit[1]) : response(404, null);
    },
    prompt: async () => "",
    ...options,
  });
  const logs = [];
  instance.log = (message) => logs.push(message);
  instance.automatedPage = async () => {
    throw new Error("no browser may open in this test");
  };
  return { instance, fetched, logs };
}

test("every generated Central and Packagist step reaches its own handler", () => {
  for (const registry of ["maven-central", "packagist", "swiftpm"]) {
    const ids = plan(registry).steps.map((step) => step.id);
    const routed = ids.filter((id) => CENTRAL_PACKAGIST_STEPS.has(id));
    assert.ok(routed.length >= 2, `${registry}: ${ids.join(", ")}`);
    for (const id of ids.filter((id) => !CENTRAL_PACKAGIST_STEPS.has(id))) {
      assert.ok(
        plan(registry).steps.find((step) => step.id === id).kind !== "api",
        `${registry}:${id} would fall through to the crates.io session`,
      );
    }
  }
});

test("Central pair: existing secrets are kept without a browser or any crates.io call", async () => {
  const github = fakeGithub({
    secrets: { CENTRAL_USERNAME: 1, CENTRAL_TOKEN: 1 },
  });
  const { instance, fetched } = session("maven-central", { github });
  const step = instance.plan.steps.find(
    (s) => s.id === "ensure-central-credential-pair",
  );
  const result = await instance.runStep(step);
  assert.equal(result.status, "present");
  assert.equal(github.writes.length, 0);
  assert.ok(!fetched.some((url) => /crates\.io/.test(url)));
  assert.equal(instance.outcome, undefined);
});

test("Central pair: missing fields with --no-browser return a prerequisite before any browser", async () => {
  const github = fakeGithub({ secrets: { CENTRAL_USERNAME: 1 } });
  const { instance } = session("maven-central", { github, noBrowser: true });
  // Only the routed steps: the Gradle build itself is not under test here.
  instance.plan.steps = instance.plan.steps.filter((s) =>
    CENTRAL_PACKAGIST_STEPS.has(s.id),
  );
  await instance.run();
  assert.equal(instance.outcome.status, "manual-prerequisite");
  assert.equal(github.writes.length, 0);
  const pair = instance.outcome.incomplete.find(
    (e) => e.step === "ensure-central-credential-pair",
  );
  assert.deepEqual(pair.secrets, ["CENTRAL_TOKEN"]);
  assert.ok(
    !instance.outcome.incomplete.some(
      (e) => e.step === "verify-central-publication",
    ),
  );
});

test("Central pair: declining the confirmation is an interruption, not success", async () => {
  const github = fakeGithub();
  const { instance } = session("maven-central", {
    github,
    prompt: async () => "n",
  });
  const step = instance.plan.steps.find(
    (s) => s.id === "ensure-central-credential-pair",
  );
  const result = await instance.runStep(step);
  assert.equal(result.status, "interrupted");
  assert.equal(instance.outcome.status, "interrupted");
  assert.equal(github.writes.length, 0);
});

test("Central pair: a confirmed new pair is stored, read back, and retries only causal jobs", async () => {
  const github = fakeGithub({
    runs: [
      {
        id: 7,
        conclusion: "failure",
        html_url: "https://github.com/x/y/actions/runs/7",
      },
    ],
    jobs: [
      { id: 1, conclusion: "failure" },
      { id: 2, conclusion: "failure" },
    ],
    logs: {
      1: "Error: CENTRAL_USERNAME secret is missing",
      2: "test failure unrelated to credentials",
    },
  });
  const logs = [];
  const { instance } = session("maven-central", {
    github,
    prompt: async () => "y",
  });
  instance.log = (message) => logs.push(message);
  instance.automatedPage = async (options) => {
    assert.equal(options.importScope, "domains");
    return {
      goto: async (url) =>
        assert.match(url, /central\.sonatype\.com\/usertoken/),
      evaluate: async () => ({ username: "user-abc", password: "secret-xyz" }),
    };
  };
  const step = instance.plan.steps.find(
    (s) => s.id === "ensure-central-credential-pair",
  );
  const result = await instance.runStep(step);
  assert.equal(result.status, "configured");
  assert.deepEqual(
    github.writes.map((w) => w.name),
    ["CENTRAL_USERNAME", "CENTRAL_TOKEN"],
  );
  assert.deepEqual(github.verifies[0].jobIds, [1]);
  assert.equal(result.retry.status, "verified");
  assert.ok(!JSON.stringify(logs).includes("secret-xyz"));
  assert.ok(!JSON.stringify(result).includes("secret-xyz"));
});

test("Central namespace and signing stay incomplete without read-back evidence", async () => {
  const github = fakeGithub({
    secrets: { GPG_PRIVATE_KEY: 1, GPG_PASSPHRASE: 1 },
  });
  const { instance } = session("maven-central", { github, noBrowser: true });
  for (const id of ["verify-namespace", "ensure-signing-key"]) {
    const result = await instance.runStep(
      instance.plan.steps.find((s) => s.id === id),
    );
    assert.equal(result.status, "manual-prerequisite", id);
  }
  assert.equal(instance.incomplete.length, 2);
});

test("Central publication: a root POM alone never certifies the Kotlin variants", async () => {
  const base =
    "io/github/link-foundation/links-notation-kotlin/0.30.0/links-notation-kotlin-0.30.0";
  const { instance } = session("maven-central", {
    github: fakeGithub(),
    routes: { [`${base}.pom`]: [200, null] },
  });
  const step = instance.plan.steps.find(
    (s) => s.id === "verify-central-publication",
  );
  assert.equal((await instance.runStep(step)).status, "root-metadata-missing");
});

const packagist = "repo.packagist.org/p2/link-foundation/links-notation";
for (const [name, routes, status] of [
  ["unregistered", {}, "unregistered"],
  ["auth-rejected", { [`${packagist}.json`]: [403, null] }, "auth-rejected"],
  [
    "transport-unknown",
    { [`${packagist}.json`]: [503, null] },
    "transport-unknown",
  ],
  [
    "dev-only",
    {
      [`${packagist}.json`]: [
        200,
        { packages: { "link-foundation/links-notation": [] } },
      ],
      [`${packagist}~dev.json`]: [
        200,
        {
          packages: {
            "link-foundation/links-notation": [{ version: "dev-main" }],
          },
        },
      ],
    },
    "dev-only",
  ],
  [
    "stable-version-missing",
    {
      [`${packagist}.json`]: [
        200,
        {
          packages: {
            "link-foundation/links-notation": [{ version: "0.29.0" }],
          },
        },
      ],
    },
    "stable-version-missing",
  ],
  [
    "verified",
    {
      [`${packagist}.json`]: [
        200,
        {
          packages: {
            "link-foundation/links-notation": [{ version: "0.30.0" }],
          },
        },
      ],
    },
    "verified",
  ],
]) {
  test(`Packagist exact stable tag: ${name}`, async () => {
    const { instance } = session("packagist", { github: fakeGithub(), routes });
    const step = instance.plan.steps.find(
      (s) => s.id === "verify-packagist-package",
    );
    assert.equal(instance.plan.package.version, "0.30.0");
    assert.equal((await instance.runStep(step)).status, status);
  });
}

test("Packagist run ends incomplete because the hook has no read-back evidence", async () => {
  const github = fakeGithub({
    secrets: { PACKAGIST_USERNAME: 1, PACKAGIST_TOKEN: 1 },
  });
  const { instance } = session("packagist", {
    github,
    routes: {
      [`${packagist}.json`]: [
        200,
        {
          packages: {
            "link-foundation/links-notation": [{ version: "0.30.0" }],
          },
        },
      ],
    },
  });
  for (const step of instance.plan.steps.filter((s) =>
    CENTRAL_PACKAGIST_STEPS.has(s.id),
  )) {
    await instance.runStep(step);
  }
  assert.deepEqual(
    instance.incomplete.map((entry) => entry.step),
    ["packagist-github-hook"],
  );
});

const encode = (text) => ({
  content: Buffer.from(text).toString("base64"),
  encoding: "base64",
});
const SLUG = "/repos/link-foundation/links-notation";
const MANIFEST =
  'let package = Package(name: "LinksNotation", products: [.library(name: "LinksNotation", targets: ["LinksNotation"])])';

for (const [name, files, status] of [
  ["missing tag", {}, "missing-tag"],
  [
    "wrong commit",
    {
      [`${SLUG}/git/ref/tags/0.30.0`]: {
        object: { type: "commit", sha: "abc" },
      },
      [`${SLUG}/contents/swift/VERSION?ref=abc`]: encode("0.29.0\n"),
    },
    "wrong-commit",
  ],
  [
    "manifest missing",
    {
      [`${SLUG}/git/ref/tags/0.30.0`]: {
        object: { type: "commit", sha: "abc" },
      },
      [`${SLUG}/contents/swift/VERSION?ref=abc`]: encode("0.30.0\n"),
    },
    "manifest-missing",
  ],
  [
    "manifest incorrect",
    {
      [`${SLUG}/git/ref/tags/0.30.0`]: {
        object: { type: "commit", sha: "abc" },
      },
      [`${SLUG}/contents/swift/VERSION?ref=abc`]: encode("0.30.0\n"),
      [`${SLUG}/contents/Package.swift?ref=abc`]: encode(
        'let package = Package(name: "Other")',
      ),
    },
    "manifest-incorrect",
  ],
  [
    "release missing",
    {
      [`${SLUG}/git/ref/tags/0.30.0`]: {
        object: { type: "commit", sha: "abc" },
      },
      [`${SLUG}/contents/swift/VERSION?ref=abc`]: encode("0.30.0\n"),
      [`${SLUG}/contents/Package.swift?ref=abc`]: encode(MANIFEST),
    },
    "release-missing",
  ],
  [
    "verified",
    {
      [`${SLUG}/git/ref/tags/0.30.0`]: {
        object: { type: "commit", sha: "abc" },
      },
      [`${SLUG}/contents/swift/VERSION?ref=abc`]: encode("0.30.0\n"),
      [`${SLUG}/contents/Package.swift?ref=abc`]: encode(MANIFEST),
      [`${SLUG}/releases/tags/0.30.0`]: {
        draft: false,
        html_url: "https://github.com/r",
      },
    },
    "verified",
  ],
]) {
  test(`SwiftPM release states stay distinct: ${name}`, async () => {
    const { instance } = session("swiftpm", { github: fakeGithub({ files }) });
    const step = instance.plan.steps.find(
      (s) => s.id === "verify-swift-release",
    );
    assert.equal((await instance.runStep(step)).status, status);
  });
}

test("SwiftPM consumer: a failed or skipped consumer step keeps the release unverified", async () => {
  for (const [conclusion, status] of [
    ["failure", "consumer-failed"],
    ["skipped", "consumer-skipped"],
    ["success", "verified"],
  ]) {
    const github = fakeGithub({
      runs: [
        {
          id: 3,
          status: "completed",
          conclusion,
          html_url: "https://github.com/r/3",
        },
      ],
      jobs: [
        {
          id: 9,
          name: "swift / publishRelease",
          steps: [
            {
              name: "Verify SwiftPM resolves and builds the release",
              conclusion,
            },
          ],
        },
      ],
    });
    const { instance } = session("swiftpm", { github });
    instance.conditions.add("swift-release-verified");
    const step = instance.plan.steps.find(
      (s) => s.id === "verify-swift-consumer",
    );
    assert.equal((await instance.runStep(step)).status, status, conclusion);
  }
});

test("Central token lifetime survives encrypted pair storage and an idempotent second setup", async () => {
  const github = fakeGithub();
  const expiry = new Date(Date.now() + 30 * 86400000).toISOString();
  const { instance, logs } = session("maven-central", {
    github,
    yes: true,
    noRetry: true,
  });
  let browserOpens = 0;
  instance.automatedPage = async () => {
    browserOpens++;
    return {
      goto: async () => {},
      evaluate: async () => ({
        username: "private-user",
        password: "private-token",
        expires_at: expiry,
        token_id: "id-1",
        token_name: "library-publishing",
      }),
    };
  };
  const step = instance.plan.steps.find(
    (entry) => entry.id === "ensure-central-credential-pair",
  );
  const first = await instance.runStep(step);
  assert.equal(first.expiry.expires_at, expiry);
  assert.equal(first.expiry.token_name, "library-publishing");
  assert.ok(github.writes.every((entry) => entry.expiresAt === expiry));
  const identity = JSON.parse(
    (await github.variables().get("CENTRAL_TOKEN_IDENTITY")).value,
  );
  assert.equal(identity.name, "library-publishing");
  const second = await instance.runStep(step);
  assert.equal(second.expiry.status, "recorded");
  assert.equal(second.expiry.expires_at, expiry);
  assert.equal(browserOpens, 1);
  assert.equal(github.writes.length, 2);
  assert.doesNotMatch(
    JSON.stringify([first, second, logs]),
    /private-user|private-token/,
  );
});

test("Central refuses a revoked or expired pair without storing values", async () => {
  for (const token of [
    { revoked: true },
    { expires_at: "2020-01-01T00:00:00Z" },
  ]) {
    const github = fakeGithub();
    const { instance } = session("maven-central", { github, yes: true });
    instance.automatedPage = async () => ({
      goto: async () => {},
      evaluate: async () => ({ username: "user", password: "value", ...token }),
    });
    const step = instance.plan.steps.find(
      (entry) => entry.id === "ensure-central-credential-pair",
    );
    assert.equal((await instance.runStep(step)).status, "unverified");
    assert.equal(github.writes.length, 0);
  }
});

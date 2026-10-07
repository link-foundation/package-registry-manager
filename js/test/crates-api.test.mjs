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

import {
  firstPublish,
  firstPublishQuestion,
  crateVersion,
  githubConfigRequest,
  signIn,
  tokenRequest,
} from "../src/crates-api.mjs";
import { cratesFlow } from "../src/flows.mjs";
import { buildPlans } from "../src/plan.mjs";
import { executePlan } from "../src/setup.mjs";
import {
  chooseSignInSource,
  findSignInSources,
  migrationFor,
  signInDomains,
} from "../src/sign-in-import.mjs";

const FAKE_TOOL = fileURLToPath(
  new URL("../../tests/fixtures/fake-tools/fake-tool.cjs", import.meta.url),
);
const temporary = await mkdtemp(path.join(os.tmpdir(), "prm-crates-api-"));

after(() => rm(temporary, { recursive: true, force: true }));

/** The secret the stubbed crates.io hands out; it must never be printed. */
const TOKEN = "cio_secret_first_publish_token_0123456789";
const API = "https://crates.test/api/v1";
const ENV = { PACKAGE_REGISTRY_MANAGER_CRATES_IO_API: API };
const PUBLISHER = {
  organization: "acme",
  repository: "demo",
  workflow: "release.yml",
  environment: null,
};
const rejected = { errors: [{ detail: "authentication failed" }] };
const websiteOnly = {
  errors: [
    { detail: "this action can only be performed on the crates.io website" },
  ],
};

/** Reads the method, path, and body a page-side fetch script sends. */
function pageCall(script) {
  const match = /fetch\(("[^"]*"), (\{.*\})\);/.exec(script);
  if (!match) {
    return { click: true };
  }
  const init = JSON.parse(match[2]);
  return {
    method: init.method,
    path: JSON.parse(match[1]),
    body: init.body === undefined ? undefined : JSON.parse(init.body),
    credentials: init.credentials,
  };
}

/**
 * A stubbed crates.io page: `answers` maps "METHOD /path" to `[status, body]`
 * or a function returning it; every call is recorded in `calls`.
 */
function stubPage(answers, calls = []) {
  return {
    calls,
    visited: [],
    cleared: [],
    async goto(url) {
      this.visited.push(url);
    },
    async evaluate(script) {
      const call = pageCall(script);
      calls.push(call);
      if (call.click) {
        return true;
      }
      const answer = answers[`${call.method} ${call.path}`];
      const [status, body] =
        typeof answer === "function" ? answer() : (answer ?? [404, null]);
      return { status, body };
    },
    async clearCookies(domains) {
      this.cleared.push(...domains);
    },
    async close() {},
  };
}

const signedIn = (verified = true) => [
  200,
  { user: { login: "maintainer", email_verified: verified } },
];
const created = [200, { api_token: { id: 42, token: TOKEN } }];

/**
 * A stubbed crates.io API for token-authenticated calls: the token works until
 * `DELETE /tokens/current`, unless `stuck` keeps it active.
 */
function stubApi({ stuck = false, attach = 201 } = {}, seen = []) {
  let revoked = false;
  return async (url, init = {}) => {
    const method = init.method ?? "GET";
    seen.push({ method, url, authorization: init.headers?.authorization });
    let status = 404;
    let body = {};
    if (url === `${API}/tokens/current` && method === "DELETE") {
      revoked = !stuck;
      status = 204;
    } else if (url === `${API}/me/tokens`) {
      [status, body] = revoked ? [401, rejected] : [403, websiteOnly];
    } else if (url === `${API}/trusted_publishing/github_configs`) {
      status = attach;
    }
    return { status, ok: status < 300, json: async () => body };
  };
}

/** Runs `action` with console output captured, and returns the output. */
async function captured(action) {
  const lines = [];
  const original = { log: console.log, error: console.error };
  console.log = (...values) => lines.push(values.join(" "));
  console.error = (...values) => lines.push(values.join(" "));
  let error;
  try {
    await action();
  } catch (caught) {
    error = caught;
  } finally {
    Object.assign(console, original);
  }
  return { lines, error, text: lines.join("\n") };
}

test("requests a 1-hour token limited to the crate and two endpoints", () => {
  const now = new Date("2026-10-04T12:00:00.000Z");
  assert.deepEqual(tokenRequest("demo", now), {
    api_token: {
      name: "prm-first-publish-demo",
      crate_scopes: ["demo"],
      endpoint_scopes: ["publish-new", "trusted-publishing"],
      expired_at: "2026-10-04T13:00:00.000Z",
    },
  });
  assert.deepEqual(githubConfigRequest("demo", PUBLISHER), {
    github_config: {
      crate: "demo",
      repository_owner: "acme",
      repository_name: "demo",
      workflow_filename: "release.yml",
      environment: null,
    },
  });
  assert.equal(
    firstPublishQuestion("demo", "0.1.0", PUBLISHER),
    "Create a 1-hour token limited to crate demo with publish-new + trusted-publishing, publish demo v0.1.0, attach release.yml as trusted publisher, revoke the token? [y/N] ",
  );
});

test("reads the crate version from the [package] table only", async () => {
  const directory = await mkdtemp(path.join(temporary, "version-"));
  await writeFile(
    path.join(directory, "Cargo.toml"),
    '[package]\nname = "demo"\nversion = "1.2.3"\n\n[dependencies]\nserde = { version = "1" }\n',
  );
  assert.equal(await crateVersion(directory), "1.2.3");
  assert.equal(await crateVersion(temporary), undefined);
});

test("creates the token in the page, hands it only to publish, attaches, revokes, and verifies", async () => {
  const page = stubPage({ "PUT /api/v1/me/tokens": created });
  const seen = [];
  const envs = [];
  const now = new Date("2026-10-04T12:00:00.000Z");
  const { text, error } = await captured(async () => {
    const result = await firstPublish({
      page,
      crate: "demo",
      publisher: PUBLISHER,
      now,
      env: ENV,
      fetch: stubApi({}, seen),
      verbose: true,
      publish: async (env) => {
        envs.push(env);
        return { code: 0 };
      },
      waitForRegistry: async () => {},
    });
    assert.deepEqual(result, { attached: true });
  });
  assert.equal(error, undefined);
  assert.deepEqual(page.calls, [
    {
      method: "PUT",
      path: "/api/v1/me/tokens",
      body: tokenRequest("demo", now),
      credentials: "same-origin",
    },
  ]);
  assert.deepEqual(envs, [{ CARGO_REGISTRY_TOKEN: TOKEN }]);
  assert.equal(process.env.CARGO_REGISTRY_TOKEN, undefined);
  assert.deepEqual(
    seen.map((call) => `${call.method} ${call.url} ${call.authorization}`),
    [
      `POST ${API}/trusted_publishing/github_configs ${TOKEN}`,
      `DELETE ${API}/tokens/current ${TOKEN}`,
      `GET ${API}/me/tokens ${TOKEN}`,
    ],
  );
  assert.match(text, /Revoked the first-publish token; crates.io rejects it/);
  assert.ok(!text.includes(TOKEN), "the token was printed");
});

test("revokes the token when cargo publish fails", async () => {
  const page = stubPage({ "PUT /api/v1/me/tokens": created });
  const seen = [];
  let waited = false;
  const { text, error } = await captured(() =>
    firstPublish({
      page,
      crate: "demo",
      publisher: PUBLISHER,
      env: ENV,
      fetch: stubApi({}, seen),
      publish: async () => ({ code: 101 }),
      waitForRegistry: async () => {
        waited = true;
      },
    }),
  );
  assert.match(error?.message ?? "", /cargo exited with status 101/);
  assert.equal(waited, false);
  assert.deepEqual(
    seen.map((call) => call.method),
    ["DELETE", "GET"],
  );
  assert.ok(!text.includes(TOKEN) && !error.message.includes(TOKEN));
});

test("fails the run while crates.io still accepts the token", async () => {
  const page = stubPage({
    "PUT /api/v1/me/tokens": created,
    "DELETE /api/v1/me/tokens/42": [204, null],
  });
  const { text, error } = await captured(() =>
    firstPublish({
      page,
      crate: "demo",
      publisher: PUBLISHER,
      env: ENV,
      fetch: stubApi({ stuck: true }),
      publish: async () => ({ code: 0 }),
      waitForRegistry: async () => {},
    }),
  );
  assert.match(
    error?.message ?? "",
    /first-publish token still authenticates on crates\.io; revoke it at https:\/\/crates\.io\/settings\/tokens/,
  );
  // The browser session revokes it by id as a fallback.
  assert.deepEqual(page.calls.at(-1), {
    method: "DELETE",
    path: "/api/v1/me/tokens/42",
    body: undefined,
    credentials: "same-origin",
  });
  assert.ok(!text.includes(TOKEN) && !error.message.includes(TOKEN));
});

test("stops before publishing when crates.io creates no token", async () => {
  let published = false;
  const { error } = await captured(() =>
    firstPublish({
      page: stubPage({
        "PUT /api/v1/me/tokens": [
          400,
          { errors: [{ detail: "crate scope is invalid" }] },
        ],
      }),
      crate: "demo",
      env: ENV,
      fetch: stubApi(),
      publish: async () => {
        published = true;
        return { code: 0 };
      },
      waitForRegistry: async () => {},
    }),
  );
  assert.match(
    error.message,
    /crate scope is invalid\); nothing was published/,
  );
  assert.equal(published, false);
});

test("signs in with GitHub, then requires a verified email", async () => {
  let answers = 0;
  const page = stubPage({
    "GET /api/v1/me": () =>
      ++answers < 3
        ? [403, { errors: [{ detail: "must be logged in" }] }]
        : signedIn(),
  });
  const { text, error } = await captured(async () => {
    const session = await signIn(page, { pollIntervalMs: 1 });
    assert.equal(session.signedInBefore, false);
  });
  assert.equal(error, undefined);
  assert.ok(
    page.calls.some((call) => call.click),
    "clicked Log in with GitHub",
  );
  assert.match(text, /Signed in to crates.io as maintainer/);
  const unverified = await captured(() =>
    signIn(stubPage({ "GET /api/v1/me": signedIn(false) })),
  );
  assert.match(
    unverified.error.message,
    /verified email address.*https:\/\/crates\.io\/settings\/profile/,
  );
});

test("plans the API flow by default and the checklist with --manual", () => {
  const crate = { registry: "crates-io", name: "demo", manifest: "Cargo.toml" };
  const ids = (steps) => steps.map((step) => step.id);
  assert.deepEqual(
    ids(cratesFlow(crate, { directory: ".", slug: "acme/demo" })),
    [
      "validate-package",
      "check-registry",
      "fetch-default-branch",
      "prepare-worktree",
      "crates-sign-in",
      "first-publish",
      "attach-trusted-publisher",
      "configure-trusted-publisher",
      "audit-token-secrets",
      "confirm-oidc-cleanup",
      "delete-token-secret",
      "crates-sign-out",
      "remove-worktree",
    ],
  );
  const manual = ids(cratesFlow(crate, { directory: ".", manual: true }));
  assert.ok(manual.includes("create-publish-token"));
  assert.ok(!manual.includes("crates-sign-in"));
});

test("offers only the registry's sign-in cookies, the default browser first", async () => {
  assert.deepEqual(signInDomains("crates-io"), ["crates.io", "github.com"]);
  assert.deepEqual(signInDomains("npm"), ["npmjs.com"]);
  assert.deepEqual(signInDomains("pypi"), ["pypi.org", "github.com"]);
  assert.deepEqual(
    migrationFor({ browser: "edge", profile: "/p" }, "domains", ["crates.io"]),
    {
      browser: "edge",
      profile: "/p",
      include: ["cookies"],
      domains: ["crates.io"],
    },
  );
  assert.deepEqual(migrationFor({ browser: "edge" }, "full", ["crates.io"]), {
    browser: "edge",
  });
  const reads = [];
  const sources = await findSignInSources(["crates.io", "github.com"], {
    sources: ["chrome", "firefox", "edge"],
    defaultSource: "firefox",
    listProfiles: async ({ browser }) =>
      browser === "edge"
        ? []
        : [{ name: "Default", path: `/${browser}/Default` }],
    readCookies: async (options) => {
      reads.push(options);
      return options.browser === "chrome" &&
        options.domainFilter === "github.com"
        ? []
        : [{ name: "session", value: "secret-cookie" }];
    },
  });
  assert.deepEqual(sources, [
    {
      browser: "firefox",
      profile: "/firefox/Default",
      label: "Firefox",
      domains: ["crates.io", "github.com"],
    },
    {
      browser: "chrome",
      profile: "/chrome/Default",
      label: "Google Chrome",
      domains: ["crates.io"],
    },
  ]);
  assert.ok(reads.every((options) => options.cache === false));
  const asked = [];
  const ask = (answer) => async (message) => {
    asked.push(message);
    return answer;
  };
  const { lines } = await captured(async () => {
    assert.equal(
      await chooseSignInSource(sources, ["crates.io"], ask(""), false),
      sources[0],
    );
    assert.equal(
      await chooseSignInSource(sources, ["crates.io"], ask("2"), false),
      sources[1],
    );
    assert.equal(
      await chooseSignInSource(sources, ["crates.io"], ask("n"), false),
      undefined,
    );
    assert.equal(
      await chooseSignInSource(sources, ["crates.io"], ask("y"), true),
      sources[0],
    );
  });
  assert.equal(asked.length, 3, "auto does not ask");
  assert.equal(
    asked[0],
    "Import your crates.io sign-in from Firefox into the automated profile? [Y/n/1-2] ",
  );
  assert.ok(lines.includes("  2. Google Chrome (crates.io)"));
  assert.ok(!lines.join("\n").includes("secret-cookie"));
});

function commands(state) {
  return readFileSync(path.join(state, "log.jsonl"), "utf8")
    .trim()
    .split("\n")
    .map((line) => JSON.parse(line));
}

async function fakeTools(state) {
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
  return bin;
}

/**
 * Runs the crates.io setup end to end with fake cargo, git, and gh, a stubbed
 * crates.io page, and a stubbed crates.io API.
 */
async function bootstrap({ publishFails = false, keepSession = false } = {}) {
  const state = await mkdtemp(path.join(temporary, "state-"));
  const bin = await fakeTools(state);
  const repository = await mkdtemp(path.join(temporary, "repository-"));
  await writeFile(
    path.join(repository, "Cargo.toml"),
    '[package]\nname = "demo"\nversion = "0.1.0"\n',
  );
  const [plan] = buildPlans(
    {
      repository: { github_owner: "acme", github_repository: "demo" },
      packages: [
        {
          registry: "crates-io",
          name: "demo",
          manifest: "Cargo.toml",
          publishable: true,
          exists_on_registry: false,
          trusted_publishing: false,
          workflow: "release.yml",
        },
      ],
    },
    ["crates-io"],
  );
  let session = false;
  const page = stubPage({
    "GET /api/v1/me": () =>
      session
        ? signedIn()
        : [403, { errors: [{ detail: "must be logged in" }] }],
    "PUT /api/v1/me/tokens": created,
  });
  const evaluate = page.evaluate.bind(page);
  page.evaluate = async (script) => {
    // The maintainer finishes the GitHub sign-in once the button is clicked.
    session ||= pageCall(script).click === true;
    return evaluate(script);
  };
  const saved = { ...process.env };
  const cargo = await mkdtemp(path.join(temporary, "cargo-"));
  Object.assign(process.env, {
    PATH: `${bin}${path.delimiter}${process.env.PATH}`,
    FAKE_STATE: state,
    FAKE_REPO_TOKEN_NAMES: "GITHUB_TOKEN",
    CARGO_HOME: cargo,
    ...(publishFails ? { FAKE_CARGO_PUBLISH: "fail" } : {}),
  });
  const seen = [];
  const api = stubApi({}, seen);
  const prompts = [];
  let output;
  try {
    output = await captured(() =>
      executePlan(plan, {
        repository,
        execute: true,
        keepSession,
        pollIntervalMs: 1,
        env: ENV,
        automation: page,
        browserOptions: { channel: "chrome", import: null },
        importDiscovery: { defaultBrowserName: "Firefox", sources: [] },
        prompt: async (message) => {
          prompts.push(message);
          return "y";
        },
        fetch: async (url, init) => {
          if (url.startsWith(API) && !url.includes("/crates/")) {
            return api(url, init);
          }
          // Missing at the check, visible once cargo published it.
          const published = commands(state).some(
            (entry) => entry.argv.join(" ") === "cargo publish",
          );
          return published
            ? { status: 200, ok: true, json: async () => ({}) }
            : { status: 404, ok: false, json: async () => ({}) };
        },
      }),
    );
  } finally {
    process.env = saved;
  }
  return { state, cargo, page, seen, prompts, output };
}

test(
  "bootstraps a crate through the crates.io API with one confirmation",
  { skip: process.platform === "win32" && "fake tools are POSIX scripts" },
  async () => {
    const { state, cargo, page, seen, prompts, output } = await bootstrap();
    assert.equal(output.error, undefined, output.text);
    const cargoRuns = commands(state).filter(
      (entry) => entry.argv[0] === "cargo",
    );
    assert.deepEqual(
      cargoRuns.map((entry) => [entry.argv.join(" "), entry.registryToken]),
      [
        ["cargo publish --dry-run", null],
        ["cargo publish", TOKEN],
      ],
    );
    assert.deepEqual(prompts, [
      "Create a 1-hour token limited to crate demo with publish-new + trusted-publishing, publish demo v0.1.0, attach release.yml as trusted publisher, revoke the token? [y/N] ",
    ]);
    assert.deepEqual(
      seen.map((call) => `${call.method} ${call.url.slice(API.length)}`),
      [
        "POST /trusted_publishing/github_configs",
        "DELETE /tokens/current",
        "GET /me/tokens",
      ],
    );
    // The token-authenticated attach replaces the cookie one and the form.
    assert.ok(
      !page.calls.some(
        (call) => call.path === "/api/v1/trusted_publishing/github_configs",
      ),
    );
    assert.deepEqual(page.cleared, ["crates.io", "github.com"]);
    await assert.rejects(readFile(path.join(cargo, "credentials.toml")));
    assert.ok(!output.text.includes(TOKEN), "the token was printed");
  },
);

test(
  "revokes the token and keeps the session on request when cargo publish fails",
  { skip: process.platform === "win32" && "fake tools are POSIX scripts" },
  async () => {
    const { seen, page, output } = await bootstrap({
      publishFails: true,
      keepSession: true,
    });
    assert.match(output.error?.message ?? "", /cargo exited with status 101/);
    assert.deepEqual(
      seen.map((call) => call.method),
      ["DELETE", "GET"],
    );
    assert.deepEqual(page.cleared, []);
    assert.match(output.text, /Keeping the crates.io \/ github.com sign-in/);
    assert.ok(!output.text.includes(TOKEN), "the token was printed");
  },
);

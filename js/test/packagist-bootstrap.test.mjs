import assert from "node:assert/strict";
import { test } from "node:test";
import { readFile, mkdtemp, writeFile, rm } from "node:fs/promises";
import path from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { runInNewContext } from "node:vm";
import sodium from "libsodium-wrappers";
import {
  createRestClient,
  createSecretManager,
} from "@link-foundation/gh-manager";
import { githubServices } from "../src/github.mjs";
import * as native from "../src/packagist-setup.mjs";
import * as publication from "../src/packagist-publication.mjs";
import { loadPackagistEngine } from "../src/packagist-driver.mjs";
import packagistPage from "../src/packagist-page.mjs";
import { runCentralPackagistStep } from "../src/central-packagist-steps.mjs";

await sodium.ready;
const keys = sodium.crypto_box_keypair();
const SAFE = "fixture-safe-private-token";
const MAIN = "fixture-main-private-token";
const SHA = "a".repeat(40);
const repo = "https://github.com/team/library";
const response = (status, body) => ({
  status,
  ok: status >= 200 && status < 300,
  json: async () => body,
});
const readSources = async (prefix) =>
  Object.fromEntries(
    await Promise.all(
      ["page", "setup", "publication"].map(async (part) => [
        part,
        await readFile(
          new URL(`${prefix}/packagist-${part}.mjs`, import.meta.url),
          "utf8",
        ),
      ]),
    ),
  );
const embedded = await loadPackagistEngine(await readSources("../../rust/src"));

/** Installed gh-manager 1.5.0 over a REST fixture, including real sealed boxes. */
async function fixture(options = {}) {
  const directory = await mkdtemp(path.join(tmpdir(), "prm-packagist-test-"));
  await writeFile(
    path.join(directory, "composer.json"),
    JSON.stringify({ name: "team/library" }),
  );
  const secrets = new Map();
  let hooks = options.hooks ?? [
    { id: 8, config: { url: "https://example.org/unrelated" } },
  ];
  const calls = [];
  const encrypted = [];
  const decrypted = [];
  const deliveries = new Map();
  let next = 20;
  const rest = createRestClient({
    token: "fixture-github",
    fetch: async (url, init) => {
      const pathname = new URL(url).pathname;
      const method = init.method;
      const body = init.body ? JSON.parse(init.body) : null;
      calls.push({
        url,
        method,
        body: body
          ? {
              ...body,
              config: body.config
                ? { ...body.config, secret: "withheld" }
                : undefined,
            }
          : null,
      });
      if (pathname.endsWith("/public-key")) {
        return response(200, {
          key_id: "test-key",
          key: sodium.to_base64(
            keys.publicKey,
            sodium.base64_variants.ORIGINAL,
          ),
        });
      }
      if (pathname.includes("/secrets")) {
        const name = pathname.split("/").at(-1);
        if (method === "PUT") {
          if (options.storageFailure) {
            return response(403, { message: SAFE });
          }
          assert.ok(!init.body.includes(SAFE));
          encrypted.push(body.encrypted_value);
          decrypted.push(
            sodium.to_string(
              sodium.crypto_box_seal_open(
                sodium.from_base64(
                  body.encrypted_value,
                  sodium.base64_variants.ORIGINAL,
                ),
                keys.publicKey,
                keys.privateKey,
              ),
            ),
          );
          if (!options.readbackFailure) {
            secrets.set(name, { name });
          }
          return response(204);
        }
        if (name === "secrets") {
          return response(200, { secrets: [...secrets.values()] });
        }
        return response(secrets.has(name) ? 200 : 404, secrets.get(name) ?? {});
      }
      if (pathname.endsWith("/hooks") && method === "GET") {
        return response(200, hooks);
      }
      if (pathname.endsWith("/hooks") && method === "POST") {
        const hook = { ...body, id: 9 };
        hooks.push(hook);
        return response(201, hook);
      }
      const hookId = Number(/\/hooks\/(\d+)/.exec(pathname)?.[1]);
      if (pathname.endsWith("/tests")) {
        const list = deliveries.get(hookId) ?? [];
        list.push({
          id: next++,
          event: "push",
          status_code: options.deliveryRejected ? 403 : 202,
        });
        deliveries.set(hookId, list);
        return response(204);
      }
      if (pathname.endsWith("/deliveries")) {
        return response(200, deliveries.get(hookId) ?? []);
      }
      if (/\/deliveries\/\d+$/.test(pathname)) {
        return response(200, deliveries.get(hookId)?.at(-1));
      }
      if (/\/hooks\/\d+$/.test(pathname)) {
        if (method === "PATCH") {
          hooks = hooks.map((hook) =>
            hook.id === hookId ? { ...hook, ...body } : hook,
          );
        }
        return response(
          200,
          hooks.find((hook) => hook.id === hookId),
        );
      }
      if (pathname.includes("/git/ref/tags/")) {
        return response(200, { object: { type: "commit", sha: SHA } });
      }
      if (pathname.includes("/releases/tags/")) {
        return response(options.releaseMissing ? 404 : 200, {
          tag_name: "v1.2.3",
          draft: false,
        });
      }
      return response(404, {});
    },
  });
  const github = githubServices({ rest });
  // Keep the actual installed encryption/readback implementation, but fail
  // absent fixture metadata immediately instead of waiting 15 seconds twice.
  github.secrets = ({ repo }) => {
    const [owner, name] = repo.split("/");
    return createSecretManager({
      rest,
      scope: { repo: { owner, name } },
      verificationTimeout: 0,
      log: { debug() {} },
    });
  };
  const navigation = [];
  const readKinds = [];
  let profileReads = 0;
  let packageReads = 0;
  let submitted = false;
  const logs = [];
  const session = {
    plan: {
      registry: "packagist",
      repository: { github_owner: "team", github_repository: "library" },
      package: {
        name: "team/library",
        manifest: "composer.json",
        version: "1.2.3",
      },
    },
    stepResults: [],
    log: (message) => logs.push(message),
    options: {
      repository: directory,
      yes: true,
      github,
      sleep: async () => {},
      execFile: async (program, args, { cwd }) => {
        assert.equal(program, "composer");
        if (args[0] === "validate") {
          if (options.invalidComposer) {
            throw new Error(SAFE);
          }
          return;
        }
        assert.ok(
          args.includes("--no-scripts") && args.includes("--no-plugins"),
        );
        await writeFile(
          path.join(cwd, "composer.lock"),
          JSON.stringify({
            packages: [
              {
                name: "team/library",
                version: "1.2.3",
                source: { reference: SHA },
              },
            ],
          }),
        );
      },
      fetch: async (url, init) => {
        assert.ok(!url.includes(SAFE) && !url.includes(MAIN));
        if (url.includes("/api/")) {
          assert.equal(init.credentials, "omit");
          assert.equal(init.redirect, "error");
          assert.equal(
            init.headers.authorization,
            `Bearer user:${url.endsWith("create-package") ? MAIN : SAFE}`,
          );
          if (options.transportFailure) {
            throw new Error(SAFE);
          }
          if (url.endsWith("create-package")) {
            submitted = true;
          }
          return response(options.apiRejected ? 403 : 202, {
            status: options.apiRejected ? "error" : "success",
            jobs: ["job-1"],
          });
        }
        packageReads++;
        if (options.absent && !submitted) {
          return response(404, null);
        }
        return response(200, {
          package: {
            name: "team/library",
            repository: options.wrongRepository
              ? "https://github.com/other/library"
              : repo,
            versions:
              options.delayed && packageReads < 3
                ? {}
                : {
                    "v1.2.3": {
                      version: "v1.2.3",
                      source: {
                        url: `${repo}.git`,
                        reference: options.shaMismatch ? "b".repeat(40) : SHA,
                      },
                    },
                  },
          },
        });
      },
    },
    prompt: async () => {
      logs.push("user-presence");
      return "";
    },
    automatedPage: async () => ({
      goto: async (url) => navigation.push(url),
      evaluate: async (script) => {
        const main = script.endsWith("(true)");
        readKinds.push(main ? "main" : "safe");
        profileReads++;
        if (options.missingAccount) {
          return { status: "authentication-required", oauth: null };
        }
        if (options.stale && profileReads <= 3) {
          return profileReads === 1 || profileReads === 3
            ? { status: "stale-oauth" }
            : {
                status: "authentication-required",
                oauth: "https://packagist.org/login/github",
              };
        }
        return {
          status: "authenticated",
          username: "user",
          token: main ? MAIN : SAFE,
        };
      },
    }),
  };
  return {
    session,
    secrets,
    encrypted,
    decrypted,
    calls,
    navigation,
    readKinds,
    logs,
    cleanup: () => rm(directory, { recursive: true, force: true }),
  };
}

for (const [runtime, engine, verification] of [
  ["JS", native, publication],
  ["Rust embedded", embedded.engine, embedded.verification],
]) {
  test(`${runtime}: installed manager encrypts Safe credentials, creates/tests hook and verifies consumer`, async () => {
    const f = await fixture();
    try {
      const step = {
        id: "packagist-update-api",
        secrets: [{ secret: "CUSTOM_USER" }, { secret: "CUSTOM_TOKEN" }],
      };
      const stored = await engine.ensurePackagistUpdate(f.session, step);
      assert.equal(stored.status, "configured");
      assert.equal(stored.update, "queued");
      assert.deepEqual(f.decrypted, ["user", SAFE]);
      assert.equal(f.encrypted.length, 2);
      assert.equal(f.secrets.size, 2);
      assert.equal(
        (await engine.ensurePackagistHook(f.session)).status,
        "verified",
      );
      assert.equal(
        (await verification.verifyPackagistPublication(f.session)).status,
        "verified",
      );
      assert.ok(
        f.calls.some(
          (call) => call.method === "POST" && call.url.endsWith("/hooks"),
        ),
      );
      assert.ok(!f.calls.some((call) => call.url.includes("/hooks/8/")));
      assert.ok(!JSON.stringify([stored, f.logs]).includes(SAFE));
      assert.ok(!f.readKinds.includes("main"));
      assert.equal(f.session.packagistCredential, undefined);
    } finally {
      await f.cleanup();
    }
  });
  for (const [name, options, status] of [
    ["encrypted storage failure", { storageFailure: true }, "storage-failed"],
    ["secret presence failure", { readbackFailure: true }, "storage-failed"],
    ["API rejection", { apiRejected: true }, "auth-rejected"],
    ["API transport unknown", { transportFailure: true }, "transport-unknown"],
  ]) {
    test(`${runtime}: ${name} cannot pass`, async () => {
      const f = await fixture(options);
      try {
        const result = await engine.ensurePackagistUpdate(f.session, {
          secrets: [
            { secret: "PACKAGIST_USERNAME" },
            { secret: "PACKAGIST_TOKEN" },
          ],
        });
        assert.equal(result.status, status);
        assert.ok(!JSON.stringify(result).includes(SAFE));
        assert.equal(f.session.packagistCredential, undefined);
      } finally {
        await f.cleanup();
      }
    });
  }
  for (const absent of [false, true]) {
    test(`${runtime}: ${absent ? "missing" : "existing"} package verified without duplicate submission`, async () => {
      const f = await fixture({ absent });
      try {
        assert.equal(
          (await engine.ensurePackagistPackage(f.session)).status,
          "verified",
        );
        assert.equal(
          (await engine.ensurePackagistPackage(f.session)).status,
          "verified",
        );
        assert.equal(
          f.readKinds.filter((kind) => kind === "main").length,
          absent ? 1 : 0,
        );
      } finally {
        await f.cleanup();
      }
    });
  }
  test(`${runtime}: stale OAuth recovers in one profile using a fresh login`, async () => {
    const f = await fixture({ stale: true });
    try {
      assert.equal(
        (await engine.acquirePackagistCredential(f.session)).status,
        "authenticated",
      );
      assert.ok(
        f.navigation.filter((url) => url.endsWith("/login/")).length >= 1,
      );
      assert.ok(!f.navigation.some((url) => /callback/.test(url)));
      assert.equal(f.logs.length, 0);
    } finally {
      await f.cleanup();
    }
  });
  test(`${runtime}: missing account stops only for registration/authentication`, async () => {
    const f = await fixture({ missingAccount: true });
    try {
      assert.equal(
        (await engine.acquirePackagistCredential(f.session)).status,
        "authentication-required",
      );
      assert.deepEqual(f.logs, ["user-presence"]);
    } finally {
      await f.cleanup();
    }
  });
  for (const [name, options, status] of [
    ["wrong repository", { wrongRepository: true }, "repository-mismatch"],
    ["invalid Composer", { invalidComposer: true }, "validation-failed"],
  ]) {
    test(`${runtime}: ${name} blocks registration`, async () => {
      const f = await fixture(options);
      try {
        assert.equal(
          (await engine.ensurePackagistPackage(f.session)).status,
          status,
        );
        assert.equal(f.readKinds.length, 0);
      } finally {
        await f.cleanup();
      }
    });
  }
  for (const [name, options, status] of [
    ["delayed indexing", { delayed: true }, "verified"],
    ["source SHA mismatch", { shaMismatch: true }, "source-mismatch"],
    ["missing release", { releaseMissing: true }, "release-missing"],
  ]) {
    test(`${runtime}: ${name} remains distinct`, async () => {
      const f = await fixture(options);
      try {
        assert.equal(
          (await verification.verifyPackagistPublication(f.session)).status,
          status,
        );
      } finally {
        await f.cleanup();
      }
    });
  }
  for (const correct of [false, true]) {
    test(`${runtime}: ${correct ? "correct" : "incorrect"} existing hook is idempotent`, async () => {
      const f = await fixture({
        hooks: [
          {
            id: 9,
            active: correct,
            config: {
              url: "https://packagist.org/api/github?username=user",
              content_type: correct ? "json" : "form",
            },
            events: correct ? ["push"] : ["*"],
          },
        ],
      });
      try {
        assert.equal(
          (await engine.ensurePackagistHook(f.session)).status,
          "verified",
        );
        assert.equal(
          (await engine.ensurePackagistHook(f.session)).status,
          "verified",
        );
        assert.equal(
          f.calls.filter((call) => call.method === "PATCH").length,
          correct ? 0 : 1,
        );
        assert.equal(
          f.calls.filter(
            (call) => call.method === "POST" && call.url.endsWith("/hooks"),
          ).length,
          0,
        );
      } finally {
        await f.cleanup();
      }
    });
  }
  test(`${runtime}: rejected push delivery stays incomplete`, async () => {
    const f = await fixture({ deliveryRejected: true });
    try {
      assert.equal(
        (await engine.ensurePackagistHook(f.session)).status,
        "hook-unverified",
      );
    } finally {
      await f.cleanup();
    }
  });
}

test("Packagist hook inspects GitHub instead of unconditionally returning a manual prerequisite", async () => {
  let inspected = false;
  const session = {
    plan: {
      repository: { github_owner: "team", github_repository: "library" },
    },
    options: {
      noBrowser: true,
      github: {
        rest: {
          request: async () => {
            inspected = true;
            return [];
          },
        },
      },
    },
    log() {},
  };
  await runCentralPackagistStep(session, { id: "packagist-github-hook" });
  assert.equal(inspected, true);
});

test("profile selects Safe/main by official button labels and never prints fields", () => {
  const group = (label, value) => ({
    querySelector: (selector) =>
      selector === "button"
        ? { textContent: label }
        : { getAttribute: () => value },
  });
  const context = {
    URL,
    location: {
      origin: "https://packagist.org",
      href: "https://packagist.org/profile/",
    },
    document: {
      body: { textContent: "Profile" },
      querySelector: () => ({ textContent: "user" }),
      querySelectorAll: () => [
        group("Show Main API Token", MAIN),
        group("Show Safe API Token", SAFE),
      ],
    },
  };
  assert.equal(
    runInNewContext(`(${packagistPage})(false)`, context).token,
    SAFE,
  );
  assert.equal(
    runInNewContext(`(${packagistPage})(true)`, context).token,
    MAIN,
  );
});

test("shared Packagist driver and modules ship byte-identically in npm and crate", async () => {
  for (const part of ["page", "setup", "publication", "driver"]) {
    assert.equal(
      await readFile(
        new URL(`../src/packagist-${part}.mjs`, import.meta.url),
        "utf8",
      ),
      await readFile(
        new URL(`../../rust/src/packagist-${part}.mjs`, import.meta.url),
        "utf8",
      ),
    );
  }
});

test("causal retry requires every configuration gate and freshly selects affected jobs", async () => {
  const f = await fixture();
  try {
    f.session.plan.package.workflow = "publish.yml";
    let jobsRead = 0;
    let request;
    f.session.options.github.rest = {
      request: async () => ({ default_branch: "main" }),
    };
    f.session.options.github.runs = {
      list: async () => [{ id: 77, conclusion: "failure" }],
      jobs: async () => {
        jobsRead++;
        return [1, 2, 3, 4].map((id) => ({ id, conclusion: "failure" }));
      },
      jobLog: async (_slug, id) =>
        ({
          1: "PACKAGIST_TOKEN is missing",
          2: "Composer unit test failed: missing local file",
          3: "Audit: team/library could not find exact version",
          4: "unrelated test failed",
        })[id],
    };
    f.session.options.github.repair = {
      verify: async (value) => {
        request = value;
        return { status: "verified" };
      },
    };
    assert.equal(
      (await publication.retryPackagistJobs(f.session)).status,
      "not-run",
    );
    assert.equal(jobsRead, 0);
    f.session.stepResults = [
      "ensure-packagist-package",
      "packagist-update-api",
      "packagist-github-hook",
    ].map((step) => ({ step, status: "verified" }));
    assert.equal(
      (await publication.retryPackagistJobs(f.session)).status,
      "verified",
    );
    assert.equal(jobsRead, 1);
    assert.deepEqual(request.jobIds, [1, 3]);
    f.session.options.github.runs.list = async () => [
      { id: 78, conclusion: "success" },
      { id: 77, conclusion: "failure" },
    ];
    assert.equal(
      (await publication.retryPackagistJobs(f.session)).status,
      "not-needed",
    );
    assert.equal(jobsRead, 1, "a superseded failure must not be retried");
  } finally {
    await f.cleanup();
  }
});

for (const [http, expected] of [
  [404, "unregistered"],
  [403, "auth-rejected"],
  [503, "transport-unknown"],
]) {
  test(`Packagist publication HTTP ${http} is ${expected}`, async () => {
    const f = await fixture();
    try {
      f.session.options.fetch = async () => response(http, null);
      assert.equal(
        (await publication.verifyPackagistPublication(f.session)).status,
        expected,
      );
    } finally {
      await f.cleanup();
    }
  });
}

test("installed driver uses the native browser protocol and emits only redacted results", async () => {
  const { execFile } = await import("node:child_process");
  const sources = await readSources("../../rust/src");
  const driver = new URL("../src/packagist-driver.mjs", import.meta.url).href;
  const script = `import {runPackagistDriver} from ${JSON.stringify(driver)}; await runPackagistDriver(${JSON.stringify(sources)});`;
  const directory = await mkdtemp(path.join(tmpdir(), "prm-driver-test-"));
  const program = path.join(directory, "driver.mjs");
  await writeFile(program, script);
  // Use a real process and the installed gh-manager resolution path. No network
  // call or test account is needed to prove the authentication boundary.
  let result;
  try {
    result = await new Promise((resolve, reject) => {
      const child = execFile(
        process.execPath,
        [program],
        {
          env: {
            ...process.env,
            PATH: `${fileURLToPath(new URL("../node_modules/.bin", import.meta.url))}${path.delimiter}${process.env.PATH}`,
          },
        },
        (error, stdout) => (error ? reject(error) : resolve(stdout)),
      );
      child.stdin.end(
        `${JSON.stringify({ plan: { steps: [{ id: "packagist-update-api" }] }, options: { noBrowser: true } })}\n`,
      );
    });
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
  assert.equal(JSON.parse(result).results[0].status, "authentication-required");
  assert.ok(!result.includes(SAFE));
});

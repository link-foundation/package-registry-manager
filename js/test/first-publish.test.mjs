import assert from "node:assert/strict";
import { test } from "node:test";
import { SetupSession } from "../src/setup.mjs";
import * as policy from "../src/npm-policy.mjs";
import * as python from "../src/python.mjs";
import * as imports from "../src/sign-in-import.mjs";

const publisher = {
  project: "demo",
  organization: "acme",
  repository: "demo",
  workflow: "release.yml",
};
const plan = {
  registry: "pypi",
  package: { name: "demo" },
  trusted_publisher: publisher,
  steps: [],
};

test("PyPI waits for login and form readiness without stdin, then verifies submission (#50)", async () => {
  let polls = 0;
  const session = new SetupSession(plan, {
    yes: true,
    pollIntervalMs: 1,
    waitTimeoutMs: 1000,
    prompt: () => {
      throw Error("must not read stdin");
    },
    automation: {
      goto: async () => {},
      evaluate: async (script) => {
        if (script.includes("shouldSubmit = true")) {
          return { submitted: true };
        }
        if (script.includes("shouldSubmit = false")) {
          return {
            filled:
              ++polls > 2
                ? ["project", "organization", "repository", "workflow"]
                : [],
          };
        }
        return { confirmed: true };
      },
    },
  });
  await session.browser({
    id: "create-pending-publisher",
    url: "https://pypi.org/manage/account/publishing/",
  });
  assert.ok(polls > 2);
  assert.ok(!session.conditions.has("trust-missing"));
});

test("npm login and approval use the same automated browser (#50)", async () => {
  const session = new SetupSession({ ...plan, registry: "npm" }, {});
  const calls = [];
  session.open = async (...args) => calls.push(args);
  await session.openLink(
    "https://www.npmjs.com/login?next=/login/cli/a",
    "login",
  );
  await session.openLink("https://www.npmjs.com/auth/cli/a", "approve");
  assert.ok(calls.every(([, automate]) => automate === true));
});

test("npm catches punctuation moved between letters and popular edit-distance names (#51)", async () => {
  for (const [name, existing] of [
    ["jsonstream", "js-on-stream"],
    ["gh-upload", "rc-upload"],
    ["lodahs", "lodash"],
  ]) {
    await assert.rejects(
      policy.checkNamePolicy(name, {
        npmScope: "acme",
        fetch: async (url) => ({
          status: url.includes("/-/v1/search") ? 200 : 404,
          ok: url.includes("/-/v1/search"),
          json: async () => ({ objects: [{ package: { name: existing } }] }),
        }),
      }),
      new RegExp(`may be refused as too similar to.*${existing}`),
    );
  }
});

test("authorization failures are not npm naming failures (#51)", () => {
  assert.equal(
    policy.policyRefusal("E403 403 Forbidden - permission denied"),
    false,
  );
});

test("npm similarity preflight never searches an empty trailing segment (#51)", async () => {
  for (const suffix of ["-", "_", "."]) {
    const name = `unique-publish-example${suffix}`;
    const queries = [];
    await policy.checkNamePolicy(name, {
      fetch: async (url) => {
        const query = new URL(url).searchParams.get("text");
        if (query !== null) {
          queries.push(query);
        }
        return {
          status: query === "" ? 400 : query === null ? 404 : 200,
          ok: query !== null && query !== "",
          json: async () => ({ objects: [] }),
        };
      },
    });
    assert.deepEqual(queries, [name]);
  }
});

test("Python build installs its frontend only into an isolated environment (#50)", async () => {
  assert.equal(typeof python.buildPythonDistribution, "function");
  const calls = [];
  await python.buildPythonDistribution("python3.13", "/project", {
    run: async (program, args) => {
      calls.push([program, args]);
      return { code: 0 };
    },
  });
  assert.deepEqual(calls[0][1].slice(0, 2), ["-m", "venv"]);
  assert.notEqual(calls[1][0], "python3.13");
  assert.ok(calls[1][1].includes("build"));
  assert.deepEqual(calls[2][1], ["-m", "build"]);
});

test("auto import falls back to saved logins and restricts migration domains (#52)", async () => {
  const source = await imports.resolveImportChoice("auto", ["pypi.org"], {
    sources: ["chrome"],
    defaultBrowserName: "Google Chrome",
    listProfiles: async () => [{ name: "Default", path: "/chrome/Default" }],
    readCookies: async () => [],
    migrateProfile: async () => ({ migrated: { passwords: 1 }, skipped: [] }),
  });
  assert.equal(source?.browser, "chrome");
  assert.deepEqual(
    imports.migrationFor(source, "domains", ["pypi.org"]).include,
    ["cookies", "passwords", "passkeys"],
  );
});

test("Safari access denial is visible without verbose output (#50)", async () => {
  const warnings = [];
  let opened = false;
  await imports.findSignInSources(["pypi.org"], {
    sources: ["safari"],
    platform: "darwin",
    listProfiles: async () => {
      throw Object.assign(Error("Operation not permitted"), { code: "EPERM" });
    },
    warn: (message) => warnings.push(message),
    openSettings: async () => {
      opened = true;
    },
  });
  assert.ok(
    warnings.some((message) => /Full Disk Access.*restart/i.test(message)),
  );
  assert.equal(opened, true);
});

test("naming refusal lists verified free alternatives and keeps the session during deferred cleanup (#51)", async () => {
  const signOut = { id: "sign-out", when: "npm-session", kind: "command" };
  const session = new SetupSession(
    {
      ...plan,
      registry: "npm",
      package: { name: "gh-upload" },
      repository: { github_owner: "acme" },
      steps: [signOut],
    },
    {
      fetch: async (url) => ({
        status: url.includes("gh-upload-toolkit") ? 503 : 404,
      }),
    },
  );
  session.conditions.add("npm-session");
  session.runProcess = async () => ({
    code: 1,
    stdout: "",
    stderr:
      "E403 403 Forbidden - Package name too similar to existing package rc-upload; try '@konard/gh-upload'",
  });
  await assert.rejects(session.command({ id: "first-publish" }), (error) => {
    assert.match(
      error.message,
      /currently free alternatives: @konard\/gh-upload, @acme\/gh-upload/,
    );
    assert.doesNotMatch(error.message, /gh-upload-toolkit/);
    return true;
  });
  session.runProcess = async () => {
    throw Error("must not sign out");
  };
  assert.deepEqual(await session.cleanup({ deferAuth: true }), []);
});

test("PyPI server errors prevent release and never ask for Enter (#50)", async () => {
  const session = new SetupSession(plan, {
    prompt: () => {
      throw Error("must not read stdin");
    },
    waitTimeoutMs: 100,
    pollIntervalMs: 1,
    automation: {
      evaluate: async (script) =>
        script.includes("shouldSubmit")
          ? {
              filled: ["project", "organization", "repository", "workflow"],
              submitted: true,
            }
          : { error: "project name is already registered" },
    },
  });
  await assert.rejects(session.prefill(), /PyPI refused.*already registered/);
  assert.ok(session.conditions.has("trust-missing"));
});

test("PyPI timeout does not mistake a login form for a publisher form (#50)", async () => {
  const session = new SetupSession(plan, {
    waitTimeoutMs: 5,
    pollIntervalMs: 1,
    automation: { evaluate: async () => ({ filled: [] }) },
  });
  await assert.rejects(session.prefill(), /timed out waiting for sign-in/);
});

test("existing PyPI projects do not require a pending-project field (#50)", async () => {
  const session = new SetupSession(
    { ...plan, mode: "attach" },
    {
      yes: true,
      waitTimeoutMs: 5,
      pollIntervalMs: 1,
      automation: {
        evaluate: async (script) =>
          script.includes("shouldSubmit")
            ? {
                filled: ["organization", "repository", "workflow"],
                submitted: true,
              }
            : { confirmed: true },
      },
    },
  );
  await session.prefill();
  assert.ok(!session.conditions.has("trust-missing"));
});

test("no saved-login diagnostic names browsers before the form opens (#52)", async () => {
  const warnings = [];
  assert.deepEqual(
    await imports.findSavedLoginSources(["pypi.org"], {
      sources: ["chrome", "safari"],
      listProfiles: async () => [{ name: "Default" }],
      migrateProfile: async () => ({ migrated: { passwords: 0, passkeys: 0 } }),
      warn: (message) => warnings.push(message),
    }),
    [],
  );
  assert.match(
    warnings.join("\n"),
    /Saved-login export did not produce a usable credential.*pypi.org.*Chrome.*Safari/,
  );
});

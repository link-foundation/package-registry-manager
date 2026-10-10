import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import path from "node:path";
import os from "node:os";
import { setupCredential } from "../src/credential-setup.mjs";
import { READ_TOKEN } from "../src/credential-browser.mjs";
import { executePlans } from "../src/setup-batch.mjs";

async function fixture(level) {
  const root = await mkdtemp(path.join(os.tmpdir(), "prm-credential-"));
  await writeFile(
    path.join(root, ".package-registry-manager.json"),
    JSON.stringify({ tokens: { "docker-hub": { level, expiry_days: 30 } } }),
  );
  return root;
}

function session(root, github) {
  return {
    plan: {
      registry: "docker-hub",
      package: {},
      repository: { github_owner: "acme", github_repository: "tool" },
    },
    options: { repository: root, github, yes: true, browserOptions: {} },
    prompt: async () => "",
    automatedPage: async (options, quiet) => {
      assert.equal(quiet, true);
      assert.equal(options.importScope, "domains");
      return {
        goto: async () => {},
        evaluate: async (script) =>
          script === READ_TOKEN
            ? {
                id: "new",
                value: "sensitive-value",
                expires_at: new Date(Date.now() + 30 * 86400000).toISOString(),
              }
            : true,
      };
    },
  };
}

test("library ensure receives token only in acquisition callback and reports org fallback (#43)", async () => {
  for (const level of [undefined, "repo"]) {
    const root = await fixture(level);
    const calls = [];
    const github = {
      health: {
        health: async () => ({ status: "auth-failing" }),
        test: async () => {
          calls.push("test");
          return { status: "ok" };
        },
      },
      secrets: (scope) => ({
        getMetadata: async () =>
          scope.repo && level !== "repo"
            ? null
            : { name: "DOCKERHUB_TOKEN", token_id: "old" },
        ensure: async (name, settings) => {
          assert.equal(name, "DOCKERHUB_TOKEN");
          assert.equal((await settings.acquire()).value, "sensitive-value");
          assert.ok(!JSON.stringify(settings).includes("sensitive-value"));
          assert.ok(settings.failurePatterns.length);
          if (level === "repo") {
            assert.equal(scope.repo, "acme/tool");
          } else {
            assert.equal(scope.org, "acme");
            assert.equal(settings.visibility, "selected");
            assert.deepEqual(settings.repos, ["acme/tool"]);
          }
          calls.push("ensure");
          return {
            path: "repository",
            fallbackReason: "organization refused",
            changed: true,
          };
        },
      }),
    };
    try {
      const run = session(root, github);
      run.prompt = async (message) => {
        if (message.startsWith("Revoke registry token old")) {
          calls.push("revoke");
        }
        return "";
      };
      await setupCredential(run);
      assert.deepEqual(calls, ["ensure", "test", "revoke"]);
      assert.equal(run.outcome.secret_scope, "repository");
      assert.equal(run.outcome.fallback_reason, "organization refused");
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  }
});

test("healthy CI requires no browser or configuration workflow (#43)", async () => {
  const root = await fixture();
  try {
    const run = session(root, {
      health: { health: async () => ({ status: "ok" }) },
    });
    run.options.noBrowser = true;
    run.automatedPage = async () =>
      assert.fail("healthy secrets need no browser");
    await setupCredential(run);
    assert.equal(run.outcome.status, "complete");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("shared organization credentials replace repository overrides before testing and revoking (#43)", async () => {
  const root = await fixture();
  const calls = [];
  try {
    const run = session(root, {
      health: {
        health: async () => ({ status: "auth-failing" }),
        test: async (name, options) => {
          calls.push(`test:${options.scope.repo}`);
          return { status: "ok" };
        },
      },
      secrets: (scope) => ({
        getMetadata: async () => (scope.repo ? { token_id: "old" } : null),
        ensure: async (name, settings) => {
          assert.equal((await settings.acquire()).value, "sensitive-value");
          calls.push(`store:${scope.repo ?? scope.org}`);
          return {
            path: scope.org ? "organization" : "repository",
            valueChanged: true,
          };
        },
      }),
    });
    run.options.secretRepositories = {
      "docker-hub:DOCKERHUB_TOKEN": ["acme/tool", "acme/peer"],
    };
    run.prompt = async (message) => {
      if (message.startsWith("Revoke registry token old")) {
        calls.push("revoke");
      }
      return "";
    };
    await setupCredential(run);
    assert.deepEqual(calls, [
      "store:acme",
      "store:acme/tool",
      "store:acme/peer",
      "test:acme/tool",
      "test:acme/peer",
      "revoke",
    ]);
    assert.equal(run.outcome.secret_scope, "repository");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("credential approval omits secret identifiers and declining creates no token (#38)", async () => {
  const root = await fixture();
  const identifier = "REGISTRY_SECRET_IDENTIFIER";
  let message;
  try {
    const run = session(root, {
      health: { health: async () => ({ status: "unknown" }) },
      secrets: () => ({ getMetadata: async () => null }),
    });
    run.plan.package.token_secrets = [identifier];
    run.options.yes = false;
    run.prompt = async (text) => {
      message = text;
      return "n";
    };
    run.automatedPage = async () => assert.fail("creation was declined");
    await assert.rejects(setupCredential(run), /credential creation declined/);
    assert.ok(message);
    assert.ok(!message.includes(identifier));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("GitHub health failure happens before browser credential creation (#43)", async () => {
  const root = await fixture();
  try {
    const run = session(root, {
      health: {
        health: async () => {
          throw new Error("permission denied");
        },
      },
    });
    run.automatedPage = async () => assert.fail("GitHub access failed");
    await assert.rejects(setupCredential(run), /permission denied/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("repository fallback and its reason appear in the printed setup summary (#43)", async (context) => {
  const root = await fixture();
  const output = [];
  context.mock.method(console, "log", (message) => output.push(message));
  try {
    const outcomes = await executePlans(
      [
        {
          registry: "docker-hub",
          repository: { root, github_owner: "acme", github_repository: "tool" },
          package: {
            name: "acme/tool",
            publishable: true,
            manifest: "Dockerfile",
          },
          steps: [
            {
              id: "manage-registry-token",
              kind: "api",
              title: "Set up CI credential",
            },
          ],
        },
      ],
      {
        execute: true,
        yes: true,
        repository: root,
        github: {
          health: {
            health: async () => ({ status: "auth-failing" }),
            test: async () => ({ status: "ok" }),
          },
          secrets: () => ({
            getMetadata: async () => null,
            ensure: async () => ({
              path: "repository",
              fallbackReason: "organization refused",
              changed: true,
            }),
          }),
        },
        automation: {
          goto: async () => {},
          evaluate: async (script) =>
            script === READ_TOKEN
              ? { id: "new", value: "private-value" }
              : true,
          close: async () => {},
        },
        prompt: async () => "",
      },
    );
    assert.equal(outcomes[0].secret_scope, "repository");
    assert.ok(
      output.some((line) =>
        line.includes("repository secret; organization refused"),
      ),
    );
    assert.ok(!output.join("\n").includes("private-value"));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

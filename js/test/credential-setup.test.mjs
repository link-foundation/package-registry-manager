import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, mkdir, writeFile, rm } from "node:fs/promises";
import path from "node:path";
import os from "node:os";
import { setupCredential, secretCommand } from "../src/credential-setup.mjs";
import { READ_TOKEN } from "../src/credential-browser.mjs";

async function fixture(level) {
  const root = await mkdtemp(path.join(os.tmpdir(), "prm-credential-"));
  await mkdir(path.join(root, ".github/workflows"), { recursive: true });
  await writeFile(
    path.join(root, ".github/workflows/verify.yml"),
    "run-name: verify ${{ inputs.prm_nonce }}\non:\n  workflow_dispatch:\n    inputs:\n      prm_nonce: {required: true}\njobs:\n  login:\n    steps:\n      - run: docker login\n",
  );
  await writeFile(
    path.join(root, ".package-registry-manager.json"),
    JSON.stringify({
      tokens: {
        "docker-hub": {
          verification_workflow: "verify.yml",
          level,
          expiry_days: 30,
        },
      },
    }),
  );
  return root;
}

test(
  "token values travel only through stdin, never mutation output (#38)",
  {
    skip: process.platform === "win32" && "fake tools are POSIX scripts",
  },
  async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), "prm-secret-command-"));
    const previous = process.env.PATH;
    try {
      await writeFile(
        path.join(root, "gh-manager"),
        "#!/usr/bin/env node\nlet input='';process.stdin.on('data',c=>input+=c);process.stdin.on('end',()=>{if(process.argv.includes('sensitive-value')) process.exit(9);process.stdout.write(input);process.stderr.write(input);});\n",
        { mode: 0o755 },
      );
      process.env.PATH = `${root}${path.delimiter}${previous}`;
      assert.equal(
        await secretCommand(
          ["secret", "ensure", "DOCKERHUB_TOKEN"],
          "sensitive-value",
          root,
        ),
        "",
      );
    } finally {
      process.env.PATH = previous;
      await rm(root, { recursive: true, force: true });
    }
  },
);

test("token setup defaults to selected org repositories and verifies before revoke (#38)", async () => {
  for (const level of [undefined, "repo"]) {
    const root = await fixture(level);
    const calls = [];
    const session = {
      plan: {
        registry: "docker-hub",
        package: {},
        repository: { github_owner: "acme", github_repository: "tool" },
      },
      options: {
        repository: root,
        yes: true,
        verificationNonce: "correlated",
        browserOptions: {},
        secretCommand: async (args, input) => {
          calls.push({ args, input });
          return input === undefined
            ? JSON.stringify({
                present: true,
                token_id: "old",
                expires_at: "2000-01-01T00:00:00Z",
              })
            : "";
        },
      },
      prompt: async (message) => {
        calls.push({ prompt: message });
        return "";
      },
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
                  expires_at: new Date(
                    Date.now() + 30 * 86400000,
                  ).toISOString(),
                }
              : script.includes("tokens-list")
                ? true
                : { filled: ["name", "scope", "expires"] },
        };
      },
      capture: async (step) => {
        const args = step.command.args;
        calls.push({ gh: args });
        return {
          code: 0,
          stdout: JSON.stringify(
            args[0] === "repo"
              ? { defaultBranchRef: { name: "main" } }
              : args[1] === "list"
                ? [{ databaseId: 1, displayTitle: "correlated" }]
                : { status: "completed", conclusion: "success" },
          ),
        };
      },
    };
    try {
      await setupCredential(session);
      const store = calls.find((call) => call.input !== undefined);
      assert.equal(store.input, "sensitive-value");
      assert.ok(!store.args.includes(store.input));
      assert.ok(store.args.includes("--expires-at"));
      if (level === "repo") {
        assert.ok(store.args.includes("--repo"));
        assert.ok(!store.args.includes("--visibility"));
      } else {
        assert.ok(store.args.includes("--org"));
        assert.ok(store.args.includes("selected"));
        assert.ok(store.args.includes("acme/tool"));
      }
      assert.ok(
        calls.findIndex(
          (call) => call.gh?.[0] === "run" && call.gh[1] === "view",
        ) <
          calls.findIndex((call) =>
            call.prompt?.startsWith("Revoke registry token old"),
          ),
      );
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  }
});

test("unsupported gh-manager fails before any browser credential creation (#38)", async () => {
  const root = await fixture();
  let opened = false;
  try {
    await assert.rejects(
      setupCredential({
        plan: {
          registry: "docker-hub",
          package: {},
          repository: { github_owner: "acme", github_repository: "tool" },
        },
        options: {
          repository: root,
          yes: true,
          secretCommand: async () => {
            throw Error("gh-manager#6 unavailable");
          },
        },
        automatedPage: async () => {
          opened = true;
        },
      }),
      /gh-manager#6/,
    );
    assert.equal(opened, false);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

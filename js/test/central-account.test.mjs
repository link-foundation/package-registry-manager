import assert from "node:assert/strict";
import { test } from "node:test";
import vm from "node:vm";
import { mkdtemp, readFile, rm, mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import centralPage from "../src/central-page.mjs";
import readCentralPair from "../src/central-token-page.mjs";
import { verifyNamespace } from "../src/central-namespace.mjs";

const namespace = "io.github.acme";
function portal({
  entries = [],
  account = "github|123",
  namespaceStatus = 200,
} = {}) {
  const calls = [];
  let current = entries;
  return {
    calls,
    context: {
      AbortSignal,
      document: { querySelectorAll: () => [] },
      fetch: async (url, init = {}) => {
        calls.push([url, init.method ?? "GET"]);
        if (url === "/api/auth/me") {
          return {
            status: 200,
            ok: true,
            json: async () => ({ sub: account }),
          };
        }
        if (init.method === "POST") {
          current = [
            {
              name: namespace,
              state: url.includes("/verify?") ? "VERIFIED" : "UNVERIFIED",
              verificationKey: "proof-123",
              owningOrgName: "Acme",
            },
          ];
        }
        return {
          status: namespaceStatus,
          ok: namespaceStatus === 200,
          json: async () => current,
        };
      },
    },
  };
}

test("authenticated ownership works before public artifacts; wrong accounts and transport remain distinct", async () => {
  const source = portal({
    entries: [{ name: namespace, state: "VERIFIED", owningOrgName: "Acme" }],
  });
  const run = (expected) =>
    vm.runInNewContext(
      `(${centralPage})(${JSON.stringify(expected)})`,
      source.context,
    );
  const state = await run({ namespace, account: "github|123" });
  assert.equal(state.status, "verified");
  assert.equal(state.organization, "Acme");
  assert.ok(source.calls.every(([url]) => !url.includes("maven")));
  assert.equal(
    (await run({ namespace, account: "github|other" })).status,
    "wrong-account",
  );
  const broken = portal({ namespaceStatus: 503 });
  assert.equal(
    (
      await vm.runInNewContext(
        `(${centralPage})(${JSON.stringify({ namespace })})`,
        broken.context,
      )
    ).status,
    "transport-unknown",
  );
});

test("authorized proof registration retains cleanup across denied deletion and reuses verified namespace", async () => {
  const repository = await mkdtemp(path.join(tmpdir(), "prm-central-proof-"));
  const source = portal();
  const mutations = [];
  let proof;
  let denied = true;
  const github = {
    rest: {
      request: async (endpoint, options = {}) => {
        if (endpoint === "/repos/acme/library") {
          return {
            id: 10,
            owner: { id: 12, login: "acme", type: "Organization" },
          };
        }
        if (options.method === "POST") {
          mutations.push(options.method);
          proof = {
            id: 34,
            owner: { id: 12 },
            full_name: "acme/proof-123",
            private: false,
            size: 0,
            default_branch: "main",
          };
          return proof;
        }
        if (options.method === "DELETE") {
          mutations.push(options.method);
          if (denied) {
            throw new Error("forbidden");
          }
          proof = null;
          return null;
        }
        if (endpoint.endsWith("/branches")) {
          return [];
        }
        return proof ?? null;
      },
    },
  };
  const session = {
    options: { repository, github, yes: true },
    plan: {
      repository: { github_owner: "acme", github_repository: "library" },
    },
    automatedPage: async () => ({
      goto: async () => {},
      evaluate: (script) => vm.runInNewContext(script, source.context),
    }),
  };
  try {
    const first = await verifyNamespace(session, { namespace });
    assert.equal(first.status, "verified");
    assert.equal(first.cleanup.status, "pending");
    const obligation = path.join(
      repository,
      ".package-registry-manager/central-namespace-proof.json",
    );
    assert.equal(JSON.parse(await readFile(obligation)).id, 34);
    const posts = source.calls.filter(([, method]) => method === "POST").length;
    denied = false;
    const second = await verifyNamespace(session, { namespace });
    assert.equal(second.status, "verified");
    assert.equal(second.cleanup.status, "complete");
    assert.equal(JSON.parse(await readFile(obligation)), null);
    assert.equal(
      source.calls.filter(([, method]) => method === "POST").length,
      posts,
    );
    assert.deepEqual(mutations, ["POST", "DELETE", "DELETE"]);
  } finally {
    await rm(repository, { recursive: true, force: true });
  }
});

test("token dialog records actual server expiry and name, never a guessed month", async () => {
  const expiry = "2026-12-06T11:22:33Z";
  const field = (name, value) => ({
    children: [],
    textContent: name,
    nextElementSibling: { value },
  });
  const context = {
    AbortSignal,
    document: {
      querySelector: () => ({
        querySelectorAll: () => [
          field("Username", "user-id"),
          field("Password", "private-value"),
        ],
      }),
    },
    fetch: async () => ({
      ok: true,
      json: async () => ({
        tokens: [
          {
            id: "token-1",
            name_code: "user-id",
            display_name: "library-publishing",
            expiration_timestamp: expiry,
          },
        ],
      }),
    }),
  };
  const pair = await vm.runInNewContext(`(${readCentralPair})()`, context);
  assert.equal(pair.expires_at, "2026-12-06T11:22:33.000Z");
  assert.equal(pair.token_name, "library-publishing");
  context.fetch = async () => ({ ok: false });
  const unknown = await vm.runInNewContext(`(${readCentralPair})()`, context);
  assert.equal(unknown.expiry_status, "unknown");
  assert.equal(unknown.expires_at, undefined);
});

test("namespace ownership rejects organization mismatch and suspended accounts", async () => {
  for (const [entry, expected, status] of [
    [
      { name: namespace, state: "VERIFIED", owningOrgName: "Other" },
      { organization: "Acme" },
      "wrong-organization",
    ],
    [
      { name: namespace, state: "VERIFIED", userSuspended: true },
      {},
      "unverified",
    ],
  ]) {
    const source = portal({ entries: [entry] });
    const result = await vm.runInNewContext(
      `(${centralPage})(${JSON.stringify({ namespace, ...expected })})`,
      source.context,
    );
    assert.equal(result.status, status);
  }
});

test("cleanup preserves a proof repository that has changed identity or gained content", async () => {
  for (const changed of [{ id: 999 }, { size: 1 }]) {
    const repository = await mkdtemp(path.join(tmpdir(), "prm-proof-guard-"));
    const proof = {
      namespace,
      account: "github|123",
      slug: "acme/proof",
      id: 1,
      owner_id: 12,
      default_branch: "main",
    };
    const directory = path.join(repository, ".package-registry-manager");
    await mkdir(directory);
    await writeFile(
      path.join(directory, "central-namespace-proof.json"),
      JSON.stringify(proof),
    );
    const source = portal({
      entries: [{ name: namespace, state: "VERIFIED" }],
    });
    let deleted = false;
    const session = {
      options: {
        repository,
        github: {
          rest: {
            request: async (_endpoint, options = {}) => {
              if (options.method === "DELETE") deleted = true;
              return {
                id: 1,
                owner: { id: 12 },
                size: 0,
                default_branch: "main",
                ...changed,
              };
            },
          },
        },
      },
      plan: {},
      automatedPage: async () => ({
        goto: async () => {},
        evaluate: (script) => vm.runInNewContext(script, source.context),
      }),
    };
    try {
      assert.equal(
        (await verifyNamespace(session, { namespace })).cleanup.status,
        "pending",
      );
      assert.equal(deleted, false);
      assert.equal(
        JSON.parse(
          await readFile(path.join(directory, "central-namespace-proof.json")),
        ).id,
        1,
      );
    } finally {
      await rm(repository, { recursive: true, force: true });
    }
  }
});

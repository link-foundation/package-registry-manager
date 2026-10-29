import assert from "node:assert/strict";
import { test } from "node:test";
import {
  cleanupMigratedSecrets,
  publishedThroughOidc,
  readsSecret,
} from "../src/migration-cleanup.mjs";

function fixture({
  otherReads = false,
  successful = true,
  changed = false,
} = {}) {
  const deleted = [];
  const job = {
    id: 20,
    conclusion: "success",
    steps: [
      {
        name: "Authenticate with crates.io (trusted publishing)",
        conclusion: "success",
      },
      { name: "Publish crate", conclusion: "success" },
    ],
  };
  let headReads = 0;
  const github = {
    rest: {
      request: async (path) =>
        path.endsWith("/commits/main")
          ? { sha: changed && ++headReads > 2 ? "changed" : "head" }
          : { default_branch: "main" },
    },
    repos: {
      list: async () => [{ full_name: "acme/one" }, { full_name: "acme/two" }],
      files: async (slug) => [
        {
          content:
            otherReads && slug === "acme/two"
              ? "env:\n  CARGO_TOKEN: ${{ secrets.CARGO_TOKEN }}\n"
              : "jobs:\n  publish:\n    permissions:\n      id-token: write\n",
        },
      ],
    },
    runs: {
      list: async () => [
        { id: 10, head_sha: "head", conclusion: "success", event: "push" },
      ],
      jobs: async () => [job],
      jobLog: async () =>
        successful
          ? "Published crate v1.0.1 at registry crates-io"
          : "crate v1.0.1 already exists; skipping publish",
    },
    secrets: (scope) => ({
      getMetadata: async (name) =>
        name.startsWith("CARGO_TOKEN") &&
        !deleted.includes(`${scope.repo ?? scope.org}:${name}`)
          ? { name }
          : null,
      delete: async (name) =>
        deleted.push(`${scope.repo ?? scope.org}:${name}`),
    }),
  };
  return {
    deleted,
    session: {
      plan: {
        registry: "crates-io",
        package: { workflow: "js.yml" },
        repository: { github_owner: "acme", github_repository: "one" },
      },
      options: {
        github,
        accountScan: true,
        yes: true,
        deleteTokenSecrets: true,
      },
    },
  };
}

test("cleanup deletes repository and organization token plus expiry only after a real OIDC upload", async () => {
  const { session, deleted } = fixture();
  await cleanupMigratedSecrets(session);
  assert.deepEqual(deleted, [
    "acme/one:CARGO_TOKEN",
    "acme/one:CARGO_TOKEN_EXPIRES_AT",
    "acme:CARGO_TOKEN",
    "acme:CARGO_TOKEN_EXPIRES_AT",
  ]);
});

test("incomplete scans and dynamic secret forwarding retain secrets", async () => {
  const incomplete = fixture();
  incomplete.session.options.scanErrors = true;
  await assert.rejects(
    cleanupMigratedSecrets(incomplete.session),
    /complete account scan/,
  );
  assert.deepEqual(incomplete.deleted, []);
  const inaccessible = fixture();
  inaccessible.session.options.github.repos.files = async () => {
    throw new Error("private repository inaccessible");
  };
  await assert.rejects(
    cleanupMigratedSecrets(inaccessible.session),
    /inaccessible/,
  );
  assert.deepEqual(inaccessible.deleted, []);
  assert.equal(
    readsSecret([{ content: "secrets: inherit" }], "CARGO_TOKEN"),
    true,
  );
  assert.equal(
    readsSecret(
      [{ content: "env: ${{ secrets[inputs.name] }}" }],
      "CARGO_TOKEN",
    ),
    true,
  );
  assert.throws(() => readsSecret([{}], "CARGO_TOKEN"), /incomplete/);
});
test("remaining organization consumers keep the shared secret while verified repository cleanup proceeds", async () => {
  const { session, deleted } = fixture({ otherReads: true });
  await cleanupMigratedSecrets(session);
  assert.deepEqual(deleted, [
    "acme/one:CARGO_TOKEN",
    "acme/one:CARGO_TOKEN_EXPIRES_AT",
  ]);
});
test("organization heads are rechecked after the deletion prompt", async () => {
  const { session, deleted } = fixture();
  let confirmed = false;
  session.options.yes = false;
  session.prompt = async () => {
    confirmed = true;
    return "yes";
  };
  session.options.github.rest.request = async (path) =>
    path.endsWith("/commits/main")
      ? {
          sha: confirmed && path.includes("acme/two/") ? "changed" : "head",
        }
      : { default_branch: "main" };
  await assert.rejects(cleanupMigratedSecrets(session), /changed/);
  assert.deepEqual(deleted, []);
});
test("skipped uploads, stale commits and changed default branches never authorize deletion", async () => {
  const skipped = fixture({ successful: false });
  await cleanupMigratedSecrets(skipped.session);
  assert.deepEqual(skipped.deleted, []);
  const stale = fixture();
  stale.session.options.github.runs.list = async () => [
    { id: 10, head_sha: "old", conclusion: "success", event: "push" },
  ];
  await cleanupMigratedSecrets(stale.session);
  assert.deepEqual(stale.deleted, []);
  const changed = fixture({ changed: true });
  await assert.rejects(cleanupMigratedSecrets(changed.session), /changed/);
  assert.deepEqual(changed.deleted, []);
  assert.equal(
    publishedThroughOidc(
      { conclusion: "success", steps: [] },
      "Published crate v1.0.1",
      "crates-io",
    ),
    false,
  );
});

test("default migration retains fallback tokens and expiry after verified OIDC (#62)", async () => {
  const { session, deleted } = fixture();
  session.options.deleteTokenSecrets = false;
  session.options.github.repos.files = async () => [
    { content: "env:\n  CARGO_TOKEN: ${{ secrets.CARGO_TOKEN }}" },
  ];
  await cleanupMigratedSecrets(session);
  assert.deepEqual(deleted, []);
  assert.equal(session.outcome.status, "fallback-retained");
  assert.equal(session.outcome.oidc_verified, true);
});

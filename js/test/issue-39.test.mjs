import assert from "node:assert/strict";
import { test } from "node:test";
import { publishingWorkflow, grantsPackagesWrite } from "../src/workflows.mjs";
import {
  checkNamePolicy,
  nameVariants,
  policyRefusal,
} from "../src/npm-policy.mjs";

test("container comments and credential preflight cannot select a publisher (#37)", () => {
  const workflows = [
    {
      name: "preflight.yml",
      contents: `on: push
# docker.io/acme/tool and ghcr.io/acme/tool
jobs:
  preflight:
    steps:
      - uses: docker/login-action@v3
        with:
          password: \${{ secrets.DOCKERHUB_TOKEN }}
      - run: echo 'docker push ghcr.io/acme/tool'
`,
    },
  ];
  assert.equal(publishingWorkflow(workflows, "docker-hub"), null);
  assert.equal(publishingWorkflow(workflows, "ghcr"), null);
  assert.equal(
    grantsPackagesWrite("permissions: read-all # packages: write"),
    false,
  );
});

test("npm punctuation policy stops before approval and reports server refusals (#36)", async () => {
  const visited = [];
  await assert.rejects(
    checkNamePolicy("gh-upload", {
      fetch: async (url) => {
        visited.push(url);
        return {
          status: url.includes("/ghupload?") ? 200 : 404,
          ok: url.includes("/ghupload?"),
        };
      },
    }),
    /similar to existing 'ghupload'/,
  );
  assert.equal(visited.length, 2);
  await assert.rejects(
    checkNamePolicy("BAD", {
      fetch: () => {
        throw Error("must not fetch");
      },
    }),
    /invalid/,
  );
  await assert.rejects(
    checkNamePolicy("gh-upload", {
      fetch: async () => ({ status: 403, ok: false }),
    }),
    /no approval requested/,
  );
  assert.ok(nameVariants("@acme/gh-upload").includes("@acme/ghupload"));
  assert.ok(nameVariants(`${"a-".repeat(30)}b`).length <= 64);
  assert.equal(
    policyRefusal("E403 package name is too similar to existing package"),
    true,
  );
  assert.equal(policyRefusal("EOTP invalid code"), false);
});

test("multiple npm manifests retain the root reusable publishing workflow", async () => {
  const { mkdtemp, mkdir, writeFile, rm } = await import("node:fs/promises");
  const path = await import("node:path");
  const os = await import("node:os");
  const { inspectRepository } = await import("../src/discovery.mjs");
  const root = await mkdtemp(path.join(os.tmpdir(), "prm-reusable-"));
  try {
    await mkdir(path.join(root, ".github/workflows"), { recursive: true });
    await mkdir(path.join(root, "packages/orphan"), { recursive: true });
    await writeFile(
      path.join(root, "package.json"),
      '{"name":"tool","version":"1.0.0"}',
    );
    await writeFile(
      path.join(root, "packages/orphan/package.json"),
      '{"name":"orphan","version":"1.0.0"}',
    );
    await writeFile(
      path.join(root, ".github/workflows/release.yml"),
      "on: push\njobs:\n  publish:\n    permissions: {id-token: write}\n    uses: ./.github/workflows/publish.yml\n",
    );
    await writeFile(
      path.join(root, ".github/workflows/publish.yml"),
      "on: workflow_call\njobs:\n  npm:\n    permissions: {id-token: write}\n    steps:\n      - run: npm publish\n",
    );
    const inspection = await inspectRepository(root);
    assert.equal(
      inspection.packages.find((item) => item.name === "tool").workflow,
      "release.yml",
    );
    assert.equal(
      inspection.packages.find((item) => item.name === "orphan").workflow,
      undefined,
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("inert build-push action names and another action's push input are not publishers", () => {
  for (const contents of [
    "on: push\njobs:\n  preflight:\n    steps:\n      - name: 'uses: docker/build-push-action@v6'\n        run: echo ready\n      - uses: acme/check@v1\n        with:\n          push: true\n          registry: ghcr.io\n",
    "on: push\njobs:\n  preflight:\n    steps:\n      - uses: docker/build-push-action@v6\n        with:\n          push: false\n          tags: ghcr.io/acme/tool\n      - uses: acme/check@v1\n        with:\n          push: true\n",
  ]) {
    const workflows = [{ name: "preflight.yml", contents }];
    assert.equal(publishingWorkflow(workflows, "docker-hub"), null);
    assert.equal(publishingWorkflow(workflows, "ghcr"), null);
  }
});

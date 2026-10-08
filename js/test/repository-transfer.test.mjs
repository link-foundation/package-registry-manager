import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { buildPlans } from "../src/plan.mjs";
import {
  inspectManifestRepositories,
  compareRepositories,
  provenanceRepositories,
  publisherIdentities,
  publisherMatches,
} from "../src/repository-identity.mjs";
import { probePackage, probeRegistryState } from "../src/registry-state.mjs";
import {
  manifestRepositoryProposal,
  offerManifestRepository,
} from "../src/manifest-proposal.mjs";
import {
  runRepositoryRepair,
  PYPI_PUBLISHERS_SCRIPT,
} from "../src/repository-repair.mjs";

const fixture = JSON.parse(
  await readFile(
    new URL(
      "../../tests/fixtures/repository-transfer/inspection.json",
      import.meta.url,
    ),
    "utf8",
  ),
);
const evidence = JSON.parse(
  await readFile(
    new URL(
      "../../tests/fixtures/repository-transfer/evidence.json",
      import.meta.url,
    ),
    "utf8",
  ),
);
const old = "konard/disk-space-saviour";
const current = "link-foundation/disk-space-saviour";
const response = (body) => ({
  status: body ? 200 : 404,
  ok: Boolean(body),
  json: async () => body,
});

test("offline manifest inspection warns even with historical trusted publishing", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "prm-transfer-"));
  try {
    await writeFile(
      path.join(root, "package.json"),
      JSON.stringify(evidence.manifest),
    );
    const inspection = structuredClone(fixture);
    inspection.repository.root = root;
    delete inspection.packages[0].repository_mismatches;
    await inspectManifestRepositories(inspection);
    assert.match(
      inspection.packages[0].warnings[0],
      /manifest still names konard/,
    );
    assert.equal(buildPlans(inspection)[0].mode, "repair");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("online inspection follows the canonical redirect and reads npm provenance/config separately", async () => {
  const inspection = structuredClone(fixture);
  inspection.repository.github_owner = "konard";
  delete inspection.packages[0].repository_mismatches;
  const attestation = {
    attestations: [
      {
        bundle: {
          dsseEnvelope: {
            payload: Buffer.from(
              JSON.stringify(evidence.npm_statement),
            ).toString("base64"),
          },
        },
      },
    ],
  };
  const commands = [];
  const result = await probeRegistryState(inspection, {
    runIdentity: async (program, args) => {
      commands.push([program, ...args]);
      return program === "gh"
        ? current
        : JSON.stringify([evidence.publishers[0]]);
    },
    fetch: async (url) =>
      response(
        url.endsWith("/latest")
          ? {
              _npmUser: { trustedPublisher: {} },
              dist: {
                attestations: { url: "https://registry.test/attestations" },
              },
            }
          : attestation,
      ),
  });
  assert.equal(result.repository.github_owner, "link-foundation");
  assert.deepEqual(
    result.packages[0].repository_mismatches.map((finding) => finding.source),
    ["provenance", "trusted publisher"],
  );
  assert.equal(result.packages[0].publisher_settings_verified, false);
  assert.ok(
    commands.some((args) => args.includes("repos/konard/disk-space-saviour")),
  );
  assert.equal(buildPlans(result)[0].mode, "repair");
});

test("PyPI provenance names its publisher; unavailable configuration never means complete", async () => {
  assert.deepEqual(provenanceRepositories(evidence.pypi_provenance), [old]);
  const inspection = structuredClone(fixture);
  inspection.packages[0].registry = "pypi";
  delete inspection.packages[0].repository_mismatches;
  const result = await probeRegistryState(inspection, {
    runIdentity: async () => current,
    fetch: async (url) =>
      response(
        url.includes("/integrity/")
          ? evidence.pypi_provenance
          : {
              info: { name: "disk-space-saviour", version: "1" },
              urls: [{ filename: "tool.whl" }],
            },
      ),
  });
  assert.equal(result.packages[0].publisher_settings_verified, false);
  assert.equal(buildPlans(result)[0].mode, "repair");
});

test("crates.io uses latest-version provenance and reads configured publishers independently", async () => {
  const pkg = { registry: "crates-io", name: "tool" };
  const state = await probePackage(pkg, {
    fetch: async (url) =>
      response(
        url.includes("github_configs?")
          ? {
              github_configs: [
                {
                  id: 1,
                  repository_owner: "konard",
                  repository_name: "disk-space-saviour",
                  workflow_filename: "release.yml",
                },
              ],
            }
          : {
              crate: { max_version: "2" },
              versions: [
                { num: "2", trustpub_data: null },
                { num: "1", trustpub_data: { repository: old } },
              ],
            },
      ),
  });
  assert.equal(state.trusted, false);
  assert.deepEqual(state.provenance_repositories, []);
  assert.equal(state.configured_publishers[0].repository, old);
});

test("only exact repository, workflow and environment matches count as the replacement", () => {
  const [plan] = buildPlans(fixture);
  const human = evidence.publishers
    .map(
      (publisher) =>
        `type: github\nid: ${publisher.id}\nrepository: ${publisher.repository}\nfile: ${publisher.file}\n`,
    )
    .join("\n");
  const publishers = publisherIdentities(human);
  assert.equal(publishers.length, 3);
  assert.equal(publisherMatches(publishers[0], plan.trusted_publisher), false);
  assert.equal(publisherMatches(publishers[1], plan.trusted_publisher), true);
  assert.equal(publisherMatches(publishers[2], plan.trusted_publisher), false);
  assert.equal(
    publisherMatches(
      { ...publishers[1], environment: "prod" },
      plan.trusted_publisher,
    ),
    false,
  );
  const pkg = {};
  compareRepositories(pkg, fixture.repository, {
    configured_publishers: [publishers[0]],
  });
  assert.match(pkg.warnings[0], /trusted publisher still names/);
});

function sessionWith(publishers) {
  let stored = structuredClone(publishers);
  const commands = [];
  const plan = buildPlans(fixture)[0];
  return {
    plan,
    options: { yes: true },
    conditions: new Set(),
    commands,
    runProcess: async (step) => {
      commands.push(step.command.args);
      if (step.command.args.includes("revoke")) {
        const id = step.command.args.at(-2);
        stored = stored.filter((publisher) => publisher.id !== id);
      }
      return { code: 0, stdout: JSON.stringify(stored) };
    },
  };
}

test("replacement verification failure preserves old trust and stops repair", async () => {
  const session = sessionWith([evidence.publishers[0]]);
  await assert.rejects(
    runRepositoryRepair(session, { id: "verify-repository-publisher" }),
    /old publishers were kept/,
  );
  await assert.rejects(
    runRepositoryRepair(session, { id: "remove-old-publisher" }),
    /verify the replacement/,
  );
  assert.ok(session.commands.every((args) => !args.includes("revoke")));
});

test("verified replacement precedes removal and other workflows of the current repository survive", async () => {
  const session = sessionWith(evidence.publishers);
  await runRepositoryRepair(session, { id: "verify-repository-publisher" });
  await runRepositoryRepair(session, { id: "remove-old-publisher" });
  const revoked = session.commands.filter((args) => args.includes("revoke"));
  assert.equal(revoked.length, 1);
  assert.equal(revoked[0].at(-2), "old-id");
  assert.ok(session.conditions.has("repository-publisher-verified"));
});

test("manifest repair preserves npm directory and Cargo/Python unrelated metadata", () => {
  const proposal = JSON.parse(
    manifestRepositoryProposal(
      JSON.stringify(evidence.manifest),
      "package.json",
      current,
    ),
  );
  assert.equal(proposal.repository.directory, "js");
  assert.equal(
    proposal.repository.url,
    `git+https://github.com/${current}.git`,
  );
  for (const [manifest, contents] of [
    [
      "Cargo.toml",
      `[package]\nname='tool'\nrepository='https://github.com/${old}'\n`,
    ],
    [
      "pyproject.toml",
      `[project.urls]\nSource='https://github.com/${old}'\nDocs='https://docs.test'\n`,
    ],
  ]) {
    const result = manifestRepositoryProposal(contents, manifest, current);
    assert.ok(result.includes(current));
    assert.equal(result.replace(current, old), contents);
  }
});

test("missing packages repair metadata before retaining their bootstrap steps", () => {
  const inspection = structuredClone(fixture);
  inspection.packages[0].exists_on_registry = false;
  const [plan] = buildPlans(inspection);
  assert.equal(plan.steps[0].id, "fix-manifest-repository");
  assert.ok(plan.steps.some((step) => step.id === "first-publish"));
});

test("manifest PR uses an isolated worktree and pauses setup without editing the checkout", async () => {
  const calls = [];
  const contents = JSON.stringify(evidence.manifest);
  let checkout;
  const result = await offerManifestRepository(buildPlans(fixture)[0], {
    yes: true,
    run: async (program, args, cwd) => {
      calls.push([program, ...args]);
      if (args[0] === "remote") {
        return `https://github.com/${current}`;
      }
      if (args.includes(".full_name")) {
        return current;
      }
      if (args.includes(".default_branch")) {
        return "main";
      }
      if (args[0] === "show") {
        return contents;
      }
      if (args[0] === "worktree" && args[1] === "add") {
        checkout = args[4];
        await mkdir(checkout);
        await writeFile(path.join(checkout, "package.json"), contents);
      }
      if (args[0] === "commit") {
        assert.equal(
          JSON.parse(await readFile(path.join(cwd, "package.json"))).repository
            .directory,
          "js",
        );
      }
      if (args[0] === "pr") {
        return "https://github.com/link-foundation/disk-space-saviour/pull/99";
      }
      return "";
    },
  });
  assert.equal(result.status, "manifest-pr");
  assert.ok(calls.find((args) => args.includes("--draft")));
  assert.ok(calls.find((args) => args.includes("remove")));
  await assert.rejects(readFile(path.join(checkout, "package.json")), /ENOENT/);
});

test("PyPI settings reader has exact field extraction, shared with Rust", async () => {
  const rust = await readFile(
    new URL("../../rust/src/setup/repository_session.rs", import.meta.url),
    "utf8",
  );
  for (const checkout of [
    rust.replaceAll("\r\n", "\n"),
    rust.replaceAll("\r\n", "\n").replaceAll("\n", "\r\n"),
  ]) {
    assert.ok(
      checkout.replaceAll("\r\n", "\n").includes(PYPI_PUBLISHERS_SCRIPT),
    );
  }
  assert.doesNotThrow(() => new Function(`return ${PYPI_PUBLISHERS_SCRIPT}`));
});

test("inline Python URLs and issue URLs keep their paths in the proposal", () => {
  const text = `[project]\nurls = { Homepage = "https://github.com/${old}", Issues = "https://github.com/${old}/issues" }\n`;
  const result = manifestRepositoryProposal(text, "pyproject.toml", current);
  assert.equal(result, text.replaceAll(old, current));
});

test("release retry blocks stale metadata and dispatches only a corrected default branch", async () => {
  for (const [historicalFixed, defaultFixed, expected] of [
    [false, false, "blocked"],
    [false, true, "workflow"],
    [true, true, "rerun"],
  ]) {
    const session = sessionWith([evidence.publishers[1]]);
    await runRepositoryRepair(session, { id: "verify-repository-publisher" });
    const sent = [];
    session.capture = async () => ({
      code: 0,
      stdout: JSON.stringify([
        { databaseId: 99, conclusion: "failure", headSha: "deadbeef" },
      ]),
    });
    session.command = async (step) => sent.push(step.command.args);
    let reference;
    session.options.run = async (program, args) => {
      if (program === "gh") {
        return "main";
      }
      if (args[0] === "fetch") {
        reference = args.at(-1);
        return "";
      }
      if (args[0] === "show") {
        return JSON.stringify({
          repository: {
            url: `https://github.com/${(reference === "deadbeef" ? historicalFixed : defaultFixed) ? current : old}`,
          },
        });
      }
    };
    if (expected === "blocked") {
      await assert.rejects(
        runRepositoryRepair(session, { id: "rerun-release" }),
        /merge the manifest/,
      );
      assert.equal(sent.length, 0);
    } else {
      await runRepositoryRepair(session, { id: "rerun-release" });
      assert.equal(sent.length, 1);
      assert.equal(sent[0][expected === "rerun" ? 1 : 0], expected);
    }
  }
});

test("a transferred repository cannot be complete despite historical trusted publishing", () => {
  const [plan] = buildPlans(fixture);
  assert.equal(plan.mode, "repair");
  const ids = plan.steps.map((step) => step.id);
  for (const id of [
    "attach-trusted-publisher",
    "verify-repository-publisher",
    "remove-old-publisher",
    "fix-manifest-repository",
    "rerun-release",
  ]) {
    assert.ok(ids.includes(id), id);
  }
  assert.ok(
    ids.indexOf("attach-trusted-publisher") <
      ids.indexOf("verify-repository-publisher"),
  );
  assert.ok(
    ids.indexOf("verify-repository-publisher") <
      ids.indexOf("remove-old-publisher"),
  );
  assert.ok(
    ids.indexOf("fix-manifest-repository") < ids.indexOf("rerun-release"),
  );
  assert.ok(!ids.includes("first-publish"));
});

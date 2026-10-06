import assert from "node:assert/strict";
import { test } from "node:test";
import {
  mkdir,
  mkdtemp,
  readFile,
  rm,
  symlink,
  writeFile,
} from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { offerWorkflow, workflowProposal } from "../src/workflow-proposal.mjs";
import { buildPlans } from "../src/plan.mjs";
import { detectPublisher } from "../src/publishers.mjs";

const packages = [
  { registry: "npm", name: "tool", manifest: "js/package.json" },
  { registry: "crates-io", name: "tool", manifest: "rust/Cargo.toml" },
  {
    registry: "pypi",
    name: "tool",
    manifest: "python/pyproject.toml",
    requires_python: ">=3.13",
  },
];
const inspection = {
  repository: { release_workflow: "release.yml" },
  packages,
};

test("adds missing jobs in the workflow already used by the other registries (#33)", () => {
  const contents =
    "on: workflow_dispatch\njobs:\n  pypi:\n    steps:\n      - run: python -m twine upload dist/*\nenv:\n  OTHER: kept\n";
  const proposal = workflowProposal(inspection, packages.slice(0, 2), [
    { name: "release.yml", contents },
  ]);
  assert.equal(proposal.workflow, "release.yml");
  assert.match(proposal.contents, /npm publish --provenance --access public/);
  assert.match(proposal.contents, /npm install --global npm@\^11/);
  assert.match(proposal.contents, /rust-lang\/crates-io-auth-action@v1/);
  assert.match(proposal.contents, /cargo publish/);
  assert.match(proposal.contents, /id-token: write/);
  assert.doesNotMatch(proposal.contents, /NPM_TOKEN/);
  assert.equal(proposal.contents.split("env:\n").at(-1), "  OTHER: kept\n");
  assert.match(proposal.contents, /github.ref == format\(/);
});

test("creates a dispatchable release workflow when none exists (#33)", () => {
  const proposal = workflowProposal({ repository: {}, packages }, packages, []);
  assert.equal(proposal.workflow, "release.yml");
  assert.match(proposal.contents, /workflow_dispatch/);
  assert.match(proposal.contents, /pypa\/gh-action-pypi-publish@release\/v1/);
  assert.match(
    proposal.contents,
    /python-version-file: 'python\/pyproject.toml'/,
  );
  assert.match(proposal.contents, /packages-dir: 'python\/dist\/'/);
  const withoutRequirement = workflowProposal(
    { repository: {}, packages },
    [{ ...packages[2], requires_python: undefined }],
    [],
  );
  assert.match(withoutRequirement.contents, /python-version: '3.x'/);
  assert.doesNotMatch(withoutRequirement.contents, /python-version-file/);
});

test("proposed jobs are detected for all registries on the next setup (#33)", async () => {
  const proposal = workflowProposal({ repository: {}, packages }, packages, []);
  for (const registry of ["npm", "crates-io", "pypi"]) {
    const publisher = await detectPublisher(
      process.cwd(),
      [{ name: proposal.workflow, contents: proposal.contents }],
      registry,
    );
    assert.equal(publisher.workflow, "release.yml", registry);
    assert.equal(publisher.jobs.length, 1, registry);
    assert.deepEqual(publisher.warnings, [], registry);
  }
});

test("refuses ambiguous targets and unsafe manifest paths (#33)", () => {
  const split = {
    repository: {},
    packages: [
      { registry: "npm", workflow: "a.yml" },
      { registry: "pypi", workflow: "b.yml" },
    ],
  };
  assert.throws(() => workflowProposal(split, packages, []), /--workflow/);
  assert.throws(
    () =>
      workflowProposal(
        inspection,
        [{ ...packages[0], manifest: "../escape/package.json" }],
        [],
      ),
    /manifest path/,
  );
  const proposal = workflowProposal(split, packages, [], "chosen.yml");
  assert.equal(proposal.workflow, "chosen.yml");
});

test("an explicit proposal target does not attach a trusted publisher (#33)", () => {
  const plans = buildPlans(
    { ...inspection, packages: [{ ...packages[0], publishable: true }] },
    [],
    { workflow: "chosen.yml", addPublishJob: true },
  );
  assert.deepEqual(
    plans[0].steps.map((step) => step.id),
    ["add-publishing-workflow"],
  );
  assert.equal(plans[0].trusted_publisher, undefined);
});

test("the workflow offer previews changes, pushes only a new branch and cleans up (#33)", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "prm-workflow-test-"));
  const commands = [];
  let checkout;
  let written;
  const completeInspection = {
    ...inspection,
    repository: {
      github_owner: "acme",
      github_repository: "tool",
      release_workflow: "release.yml",
    },
  };
  const plans = [
    {
      registry: "npm",
      repository: completeInspection.repository,
      package: { ...packages[0], publishable: true },
    },
  ];
  const run = async (program, args) => {
    commands.push([program, ...args]);
    if (args[0] === "remote") {
      return "https://github.com/acme/tool.git";
    }
    if (args[0] === "repo") {
      return "main";
    }
    if (args[0] === "worktree" && args[1] === "add") {
      checkout = args[4];
      await mkdir(path.join(checkout, ".github/workflows"), {
        recursive: true,
      });
      await writeFile(
        path.join(checkout, ".github/workflows/release.yml"),
        "jobs:\n  existing:\n    steps:\n      - run: echo preserved\n",
      );
    }
    if (args[0] === "add") {
      written = await readFile(
        path.join(checkout, ".github/workflows/release.yml"),
        "utf8",
      );
    }
    if (args[0] === "pr") {
      return "https://github.com/acme/tool/pull/1";
    }
    return "";
  };
  try {
    const options = {
      repository: root,
      inspection: completeInspection,
      run,
      yes: false,
      prompt: async () => "n",
    };
    assert.equal((await offerWorkflow(plans, options)).status, "blocked");
    assert.deepEqual(commands, []);
    const result = await offerWorkflow(plans, { ...options, yes: true });
    assert.equal(result.status, "workflow-pr");
    const push = commands.find((args) => args[1] === "push");
    assert.match(push[3], /^HEAD:refs\/heads\/prm\/publish-jobs-\d+$/);
    const pr = commands.find((args) => args[1] === "pr");
    assert.ok(pr.includes("--draft"));
    assert.ok(pr.includes("main"));
    assert.match(written, /echo preserved/);
    assert.match(written, /npm publish --provenance/);
    await assert.rejects(
      readFile(path.join(checkout, ".github/workflows/release.yml")),
      { code: "ENOENT" },
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test(
  "a proposal refuses a workflow symlink before writing or pushing (#33)",
  { skip: process.platform === "win32" },
  async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), "prm-workflow-link-"));
    const outside = path.join(root, "outside.yml");
    await writeFile(outside, "jobs:\n");
    const commands = [];
    const repo = { github_owner: "acme", github_repository: "tool" };
    try {
      await assert.rejects(
        offerWorkflow(
          [{ registry: "npm", repository: repo, package: packages[0] }],
          {
            repository: root,
            inspection: { repository: repo, packages },
            yes: true,
            run: async (program, args) => {
              commands.push([program, ...args]);
              if (args[0] === "remote") {
                return "https://github.com/acme/tool.git";
              }
              if (args[0] === "repo") {
                return "main";
              }
              if (args[0] === "worktree" && args[1] === "add") {
                await mkdir(path.join(args[4], ".github/workflows"), {
                  recursive: true,
                });
                await symlink(
                  outside,
                  path.join(args[4], ".github/workflows/release.yml"),
                );
              }
              return "";
            },
          },
        ),
        /without symlinks/,
      );
      assert.equal(await readFile(outside, "utf8"), "jobs:\n");
      assert.ok(!commands.some((args) => args[1] === "push"));
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  },
);

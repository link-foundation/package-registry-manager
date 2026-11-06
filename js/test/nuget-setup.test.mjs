import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { configureNuget } from "../src/nuget-setup.mjs";

test("NuGet sign-in prompts and mismatch errors omit the configured profile", async () => {
  const repository = await mkdtemp(path.join(tmpdir(), "prm-nuget-privacy-"));
  const username = "private-profile-18";
  try {
    await writeFile(
      path.join(repository, ".package-registry-manager.json"),
      JSON.stringify({
        nuget: {
          username,
          package_owner: "Org",
          package_ids: ["Org.Library"],
          operations: ["package:pushversion"],
        },
      }),
    );
    for (const actual of [undefined, "different-profile"]) {
      const prompts = [];
      const session = {
        options: {
          repository,
          github: {
            rest: { request: async () => ({ id: 34, owner: { id: 12 } }) },
          },
        },
        plan: {
          package: { name: "Org.Library" },
          trusted_publisher: {
            organization: "acme",
            repository: "library",
            workflow: "ci.yml",
          },
        },
        visiblePage: async () => ({
          goto: async () => {},
          evaluate: async () => ({ ready: Boolean(actual), username: actual }),
        }),
        prompt: async (message) => {
          prompts.push(message);
          throw new Error("sign-in interrupted");
        },
      };
      await assert.rejects(configureNuget(session), (error) => {
        assert.doesNotMatch(error.message, new RegExp(username));
        assert.match(
          error.message,
          actual ? /differs from/ : /sign-in interrupted/,
        );
        return true;
      });
      assert.equal(prompts.length, actual ? 0 : 1);
      assert.ok(prompts.every((message) => !message.includes(username)));
    }
  } finally {
    await rm(repository, { recursive: true, force: true });
  }
});

test("create then reload verifies saved display scopes and second setup is idempotent", async () => {
  const { default: vm } = await import("node:vm");
  const repository = await mkdtemp(
    path.join(tmpdir(), "prm-nuget-idempotent-"),
  );
  let creations = 0;
  let reloads = 0;
  let saved = [];
  const current = {};
  const observable = (name) => (value) =>
    value === undefined ? current[name] : (current[name] = value);
  const draft = {
    PackageOwners: [{ Owner: "Org", CanPushExisting: true }],
    gitHub: {},
    Valid: () => true,
    CreateOrEdit: () => {
      creations++;
      saved = [
        {
          PublisherName: "GitHubActions",
          Key: 42,
          Owner: "Org",
          PolicySubjects: ["Org.Library"],
          PolicyScopes: ["Push only new package versions"],
          PendingPolicyScopes: ["package:push", "package:unlist"],
          PushScope: "package:push",
          IsOwnerValid: true,
          gitHub: {
            RepositoryOwner: "acme",
            RepositoryOwnerId: 12,
            Repository: "library",
            RepositoryId: 34,
            WorkflowFile: "publish-csharp.yml",
            Environment: "nuget",
            IsPermanentlyEnabled: true,
          },
        },
      ];
    },
  };
  for (const field of [
    "PackageOwner",
    "SelectedProvider",
    "PendingPolicyName",
    "PushScopeChecked",
    "PushScope",
    "UnlistScopeChecked",
    "ScopeSubjectsInput",
    "PendingCreateOrEdit",
  ]) {
    draft[field] = observable(field);
  }
  for (const field of [
    "PendingRepositoryOwner",
    "PendingRepository",
    "PendingWorkflowFile",
    "PendingEnvironment",
  ]) {
    draft.gitHub[field] = observable(field);
  }
  const context = {
    document: {
      body: {},
      querySelector: () => ({ textContent: "profile" }),
      querySelectorAll: () => [
        {
          value: "package:pushversion",
          closest: () => ({
            querySelector: () => ({
              textContent: "Push only new package versions",
            }),
          }),
          getAttribute: () => "",
        },
      ],
    },
    ko: {
      dataFor: () => ({
        Policies: () => saved,
        NewPolicy: () => draft,
        Error: () => null,
      }),
    },
  };
  const page = {
    goto: async () => {
      reloads++;
    },
    evaluate: (script) => vm.runInNewContext(script, context),
  };
  const session = {
    options: {
      repository,
      pollIntervalMs: 1,
      yes: true,
      github: {
        rest: { request: async () => ({ id: 34, owner: { id: 12 } }) },
        variables: () => ({
          ensure: async () => ({
            verified: true,
            effective: [{ value: "profile" }],
            changed: false,
          }),
        }),
        secrets: () => {
          throw new Error("fallback credentials must not be touched");
        },
      },
    },
    plan: {
      package: {
        name: "Org.Library",
        workflow: "js.yml",
        workflow_callees: ["publish-csharp.yml -> upload"],
      },
      trusted_publisher: {
        organization: "acme",
        repository: "library",
        workflow: "js.yml",
        environment: "nuget",
      },
    },
    visiblePage: async () => page,
    automation: page,
    conditions: new Set(["trust-missing"]),
    recordRecoveryChange: () => {},
  };
  try {
    await writeFile(
      path.join(repository, ".package-registry-manager.json"),
      JSON.stringify({
        nuget: {
          username: "profile",
          package_owner: "Org",
          package_ids: ["Org.Library"],
          operations: ["package:pushversion"],
        },
      }),
    );
    await configureNuget(session);
    assert.equal(session.nugetEvidence.policy.verified, true);
    assert.equal(current.PendingWorkflowFile, "publish-csharp.yml");
    assert.equal(current.UnlistScopeChecked, false);
    assert.ok(reloads >= 2);
    await configureNuget(session);
    assert.equal(creations, 1);
    assert.equal(session.nugetEvidence.policy_changed, false);
    assert.equal(session.plan.package.workflow, "js.yml");
  } finally {
    await rm(repository, { recursive: true, force: true });
  }
});

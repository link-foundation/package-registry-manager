import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { configureNuget } from "../src/nuget-setup.mjs";
import { publishNugetArtifact } from "../src/nuget-policy.mjs";

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

for (const renamed of [false, true]) {
  test(
    renamed
      ? "renamed first publication edits the oldest policy, reloads and keeps other repositories"
      : "create then reload verifies saved display scopes and second setup is idempotent",
    async () => {
      const packageName = renamed ? "links-notation" : "Org.Library";
      const operation = renamed ? "package:push" : "package:pushversion";
      const label = renamed
        ? "Push new packages and versions"
        : "Push only new package versions";
      let edits = 0;
      let pendingReads = 0;
      let recoveryChanges = 0;
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
        PackageOwners: [
          { Owner: "Org", CanPushExisting: true, CanPushNew: true },
        ],
        gitHub: {},
        Valid: () => true,
        CreateOrEdit: () => {
          creations++;
          saved = [
            {
              PublisherName: "GitHubActions",
              Key: 42,
              Owner: "Org",
              PolicySubjects: [packageName],
              PolicyScopes: [label],
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
      const savedPolicy = (key, subjects, scopes, repository = "library") => ({
        PublisherName: "GitHubActions",
        Key: key,
        Owner: "Org",
        Creator: "creator-profile",
        PolicySubjects: subjects,
        PolicyScopes: scopes,
        IsOwnerValid: true,
        gitHub: {
          RepositoryOwner: "acme",
          RepositoryOwnerId: 12,
          Repository: repository,
          RepositoryId: repository === "library" ? 34 : 35,
          WorkflowFile: "publish-csharp.yml",
          Environment: "nuget",
          IsPermanentlyEnabled: true,
        },
      });
      if (renamed) {
        const original = savedPolicy(
          24222,
          ["Historical.Library"],
          ["Push only new package versions"],
        );
        const newer = savedPolicy(24255, [packageName], [label]);
        const unrelated = savedPolicy(
          7,
          ["Other.Library"],
          ["package:pushversion"],
          "another-repository",
        );
        // The real Gallery returns creation order; IDs themselves are not used for sorting.
        saved = [original, newer, unrelated];
        for (const field of [
          "PackageOwners",
          "PackageOwner",
          "SelectedProvider",
          "PendingPolicyName",
          "PushScopeChecked",
          "PushScope",
          "UnlistScopeChecked",
          "ScopeSubjectsInput",
        ]) {
          original[field] = draft[field];
        }
        for (const field of [
          "PendingRepositoryOwner",
          "PendingRepository",
          "PendingWorkflowFile",
          "PendingEnvironment",
        ]) {
          original.gitHub[field] = draft.gitHub[field];
          original.gitHub[field]("unsaved-identity-edit");
        }
        original.Valid = () => true;
        original.PendingCreateOrEdit = () => edits > 0 && ++pendingReads < 3;
        original.CreateOrEdit = () => {
          edits++;
          original.PolicySubjects = current.ScopeSubjectsInput.split("\n");
          original.PolicyScopes =
            current.PushScope === "package:push"
              ? [label]
              : ["Push only new package versions"];
          original.PolicyName = current.PendingPolicyName;
        };
      }
      const unrelatedBefore = renamed ? JSON.stringify(saved[2]) : null;
      const push = () =>
        publishNugetArtifact("same.nupkg", {
          username: "profile",
          login: async () => ({ status: "success", key: "temporary" }),
          fingerprint: async () => "same-sha",
          exactVersionExists: async () => false,
          // An exchanged OIDC key inherits only the oldest identity match's scopes.
          push: async () => ({
            status:
              saved[0]?.PolicySubjects.includes(packageName) &&
              saved[0]?.PolicyScopes.includes(label)
                ? "accepted"
                : "authorization-failed",
          }),
        });
      const context = {
        document: {
          body: {},
          querySelector: () => ({ textContent: "profile" }),
          querySelectorAll: () => [
            {
              value: operation,
              closest: () => ({
                querySelector: () => ({
                  textContent: label,
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
            name: packageName,
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
        recordRecoveryChange: () => {
          recoveryChanges++;
        },
      };
      try {
        await writeFile(
          path.join(repository, ".package-registry-manager.json"),
          JSON.stringify({
            nuget: {
              username: "profile",
              package_owner: "Org",
              package_ids: [packageName],
              operations: [operation],
            },
          }),
        );
        if (renamed) {
          assert.equal((await push()).publication, "unverified");
        }
        await configureNuget(session);
        assert.equal(session.nugetEvidence.policy.verified, true);
        assert.equal(current.PendingWorkflowFile, "publish-csharp.yml");
        assert.equal(current.UnlistScopeChecked, false);
        assert.ok(reloads >= 2);
        await configureNuget(session);
        assert.equal(creations, renamed ? 0 : 1);
        assert.equal(edits, renamed ? 1 : 0);
        assert.equal(recoveryChanges, 1);
        if (renamed) {
          assert.equal((await push()).publication, "accepted");
          assert.equal(session.nugetEvidence.policy.policy_id, 24222);
          assert.deepEqual(saved[0].PolicySubjects, ["links-notation"]);
          assert.deepEqual(saved[0].PolicyScopes, [
            "Push new packages and versions",
          ]);
          assert.equal(saved[0].Owner, "Org");
          assert.equal(saved[0].Creator, "creator-profile");
          assert.equal(saved[0].gitHub.RepositoryOwnerId, 12);
          assert.equal(saved[0].gitHub.RepositoryId, 34);
          assert.equal(saved[0].gitHub.IsPermanentlyEnabled, true);
          assert.equal(current.PendingRepositoryOwner, "acme");
          assert.equal(current.PendingRepository, "library");
          assert.equal(current.PendingEnvironment, "nuget");
          assert.equal(pendingReads >= 3, true);
          assert.equal(JSON.stringify(saved[2]), unrelatedBefore);
        }
        assert.equal(session.nugetEvidence.policy_changed, false);
        assert.equal(session.plan.package.workflow, "js.yml");
      } finally {
        await rm(repository, { recursive: true, force: true });
      }
    },
  );
}

import { test } from "node:test";
import assert from "node:assert/strict";
import vm from "node:vm";
import { readFileSync } from "node:fs";
import nugetPage from "../src/nuget-page.mjs";

const expected = {
  username: "profile",
  package_owner: "Org",
  organization: "org",
  repository: "library",
  owner_id: "12",
  repository_id: "34",
  workflow: "ci.yml",
  package_ids: ["Org.Library"],
  operations: ["package:pushversion"],
};
function page(username, policies = []) {
  const values = {};
  const observable = (field, value) => {
    values[field] = value;
    return (next) => {
      if (next !== undefined) {
        values[field] = next;
      }
      return values[field];
    };
  };
  const draft = {
    PackageOwners: [{ Owner: "Org", CanPushExisting: true }],
    gitHub: {},
    Valid: () => true,
    CreateOrEdit: () => {
      values.created = true;
    },
  };
  for (const field of [
    "PackageOwner",
    "SelectedProvider",
    "PendingPolicyName",
    "PushScopeChecked",
    "PushScope",
    "UnlistScopeChecked",
    "StageScopeChecked",
    "ScopeSubjectsInput",
    "PendingCreateOrEdit",
  ]) {
    draft[field] = observable(field, false);
  }
  for (const field of [
    "PendingRepositoryOwner",
    "PendingRepository",
    "PendingWorkflowFile",
    "PendingEnvironment",
  ]) {
    draft.gitHub[field] = observable(field, "");
  }
  const model = {
    Policies: () => policies,
    NewPolicy: () => draft,
    Error: () => null,
  };
  const context = {
    document: {
      body: {},
      querySelector: () => (username ? { textContent: username } : null),
    },
    ko: { dataFor: () => model },
  };
  return {
    run: (create = false) =>
      vm.runInNewContext(
        `(${nugetPage})(${JSON.stringify(expected)},${create})`,
        context,
      ),
    values,
  };
}
test("NuGet official page adapter creates only exact owner, packages and operation", () => {
  const { run, values } = page("profile");
  assert.equal(run().ready, true);
  assert.equal(run(true).submitted, true);
  assert.equal(values.PackageOwner.Owner, "Org");
  assert.equal(values.ScopeSubjectsInput, "Org.Library");
  assert.equal(values.PushScope, "package:pushversion");
  assert.equal(values.UnlistScopeChecked, false);
  assert.equal(values.StageScopeChecked, false);
  assert.equal(
    page("wrong").run(true).error,
    "wrong authenticated NuGet profile",
  );
});
test("NuGet page adapter discovers equivalent policy and refuses duplicate creation", () => {
  const policy = {
    PublisherName: () => "GitHubActions",
    Key: () => 1,
    Owner: () => "Org",
    PolicySubjects: () => expected.package_ids,
    PolicyScopes: () => expected.operations,
    IsOwnerValid: () => true,
    gitHub: Object.fromEntries(
      Object.entries({
        RepositoryOwner: "org",
        RepositoryOwnerId: 12,
        Repository: "library",
        RepositoryId: 34,
        WorkflowFile: "ci.yml",
        Environment: "",
        IsPermanentlyEnabled: false,
        EnabledDaysLeft: 3,
      }).map(([key, value]) => [key, () => value]),
    ),
  };
  const { run, values } = page("profile", [policy]);
  assert.equal(run().policies[0].activation, "temporarily-active");
  assert.match(run(true).error, /duplicate creation refused/);
  assert.equal(values.created, undefined);
});
test("NuGet browser adapter is identical in both ports", () => {
  assert.equal(
    readFileSync(new URL("../src/nuget-page.mjs", import.meta.url), "utf8"),
    readFileSync(
      new URL("../../rust/src/nuget-page.mjs", import.meta.url),
      "utf8",
    ),
  );
});

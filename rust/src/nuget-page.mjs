/* global document */
/** Runs in the official NuGet Gallery page; never reads API keys or cookies. */
export default function nugetPage(expected, create = false) {
  const username =
    document.querySelector("#account-dropdown b span")?.textContent.trim() ??
    null;
  const model = globalThis.ko?.dataFor(document.body);
  if (!model?.Policies) {
    return { username, policies: [], ready: false };
  }
  const read = (object, name) =>
    typeof object?.[name] === "function" ? object[name]() : object?.[name];
  const policies = model
    .Policies()
    .filter((policy) => read(policy, "PublisherName") === "GitHubActions")
    .map((policy) => ({
      id: read(policy, "Key"),
      package_owner: read(policy, "Owner"),
      organization: read(policy.gitHub, "RepositoryOwner"),
      repository: read(policy.gitHub, "Repository"),
      owner_id: String(read(policy.gitHub, "RepositoryOwnerId") ?? ""),
      repository_id: String(read(policy.gitHub, "RepositoryId") ?? ""),
      workflow: read(policy.gitHub, "WorkflowFile"),
      environment: read(policy.gitHub, "Environment") ?? "",
      package_ids: read(policy, "PolicySubjects") ?? [],
      operations: read(policy, "PolicyScopes") ?? [],
      activation: !read(policy, "IsOwnerValid")
        ? "inactive"
        : read(policy.gitHub, "IsPermanentlyEnabled")
          ? "active"
          : read(policy.gitHub, "EnabledDaysLeft") > 0
            ? "temporarily-active"
            : "inactive",
    }));
  const result = {
    username,
    policies,
    ready: true,
    pending: Boolean(read(model.NewPolicy(), "PendingCreateOrEdit")),
    error: read(model, "Error") ?? null,
  };
  if (!create) {
    return result;
  }
  if (username?.toLowerCase() !== expected.username.toLowerCase()) {
    return { ...result, error: "wrong authenticated NuGet profile" };
  }
  // Never create another policy for a repository/workflow without explicit review.
  if (
    policies.some((policy) =>
      ["organization", "repository", "workflow", "environment"].every(
        (field) =>
          String(policy[field] ?? "").toLowerCase() ===
          String(expected[field] ?? "").toLowerCase(),
      ),
    )
  ) {
    return {
      ...result,
      error:
        "existing repository policy requires review; duplicate creation refused",
    };
  }
  const draft = model.NewPolicy();
  const owners = read(draft, "PackageOwners") ?? [];
  const owner = owners.find(
    (item) => item.Owner.toLowerCase() === expected.package_owner.toLowerCase(),
  );
  if (
    !owner ||
    expected.operations.length !== 1 ||
    !(expected.operations[0] === "package:push"
      ? owner.CanPushNew
      : owner.CanPushExisting)
  ) {
    return {
      ...result,
      error: "exact package owner or required operation is unavailable",
    };
  }
  draft.PackageOwner(owner);
  draft.SelectedProvider("GitHubActions");
  draft.PendingPolicyName(
    `prm-${expected.organization}-${expected.repository}-${expected.workflow}`,
  );
  draft.gitHub.PendingRepositoryOwner(expected.organization);
  draft.gitHub.PendingRepository(expected.repository);
  draft.gitHub.PendingWorkflowFile(expected.workflow);
  draft.gitHub.PendingEnvironment(expected.environment ?? "");
  draft.PushScopeChecked(true);
  draft.PushScope(expected.operations[0]);
  draft.UnlistScopeChecked(false);
  draft.StageScopeChecked(false);
  draft.ScopeSubjectsInput(expected.package_ids.join("\n"));
  if (!draft.Valid()) {
    return { ...result, error: "NuGet rejected the exact policy fields" };
  }
  draft.CreateOrEdit();
  return { ...result, submitted: true };
}

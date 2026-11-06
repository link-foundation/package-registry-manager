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
  // Scope labels and radio values are rendered by the same server formatter.
  // Normalize persisted labels through that mapping, never through mutable drafts.
  const scopeLabels = new Map();
  for (const input of document.querySelectorAll?.(
    'input[type="radio"], input[type="checkbox"]',
  ) ?? []) {
    const label = input
      .closest("label")
      ?.querySelector("span")
      ?.textContent.trim();
    const binding = input.getAttribute("data-bind") ?? "";
    const scope = input.value?.startsWith("package:")
      ? input.value
      : binding.includes("UnlistScopeChecked")
        ? "package:unlist"
        : binding.includes("StageScopeChecked")
          ? "package:stage"
          : null;
    if (label && scope) {
      scopeLabels.set(label, scope);
    }
  }
  const savedScopes = (policy) =>
    (read(policy, "PolicyScopes") ?? []).map((scope) =>
      scope.startsWith("package:")
        ? scope
        : (scopeLabels.get(scope) ?? `unresolved:${scope}`),
    );
  const policies = model
    .Policies()
    .filter((policy) => read(policy, "PublisherName") === "GitHubActions")
    // UsersController.TrustedPublishing sorts the server list by Created ascending.
    .map((policy, index) => ({
      id: read(policy, "Key"),
      evaluation_order: index,
      package_owner: read(policy, "Owner"),
      organization: read(policy.gitHub, "RepositoryOwner"),
      repository: read(policy.gitHub, "Repository"),
      owner_id: String(read(policy.gitHub, "RepositoryOwnerId") ?? ""),
      repository_id: String(read(policy.gitHub, "RepositoryId") ?? ""),
      workflow: read(policy.gitHub, "WorkflowFile"),
      environment: read(policy.gitHub, "Environment") ?? "",
      package_ids: read(policy, "PolicySubjects") ?? [],
      operations: savedScopes(policy),
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
    pending: [model.NewPolicy(), ...model.Policies()].some((policy) =>
      Boolean(read(policy, "PendingCreateOrEdit")),
    ),
    error: read(model, "Error") ?? null,
  };
  if (!create) {
    return result;
  }
  if (username?.toLowerCase() !== expected.username.toLowerCase()) {
    return { ...result, error: "wrong authenticated NuGet profile" };
  }
  const matches = policies.filter(
    (policy) =>
      ["organization", "repository"].every(
        (field) =>
          String(policy[field] ?? "").toLowerCase() ===
          expected[field].toLowerCase(),
      ) &&
      ["workflow", "environment"].every(
        (field) =>
          String(policy[field] ?? "") === String(expected[field] ?? ""),
      ),
  );
  const existing =
    matches.find((policy) => policy.activation !== "inactive") ?? matches[0];
  if (
    existing &&
    (existing.package_owner?.toLowerCase() !==
      expected.package_owner.toLowerCase() ||
      ["owner_id", "repository_id"].some(
        (field) => existing[field] !== String(expected[field]),
      ))
  ) {
    return {
      ...result,
      error:
        "existing policy owner or immutable GitHub identity differs; duplicate creation refused",
    };
  }
  if (
    existing &&
    ["package_ids", "operations"].every(
      (field) =>
        JSON.stringify(
          [...existing[field]].map((s) => s.toLowerCase()).sort(),
        ) ===
        JSON.stringify([...expected[field]].map((s) => s.toLowerCase()).sort()),
    )
  ) {
    return {
      ...result,
      error: "equivalent existing policy; duplicate creation refused",
    };
  }
  const draft = existing
    ? model.Policies().find((policy) => read(policy, "Key") === existing.id)
    : model.NewPolicy();
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
  if (!existing) {
    draft.PackageOwner(owner);
    draft.SelectedProvider("GitHubActions");
  }
  // Reset any unsaved identity edits to the persisted values when repairing scopes.
  for (const [field, value] of Object.entries({
    RepositoryOwner: existing
      ? read(draft.gitHub, "RepositoryOwner")
      : expected.organization,
    Repository: existing
      ? read(draft.gitHub, "Repository")
      : expected.repository,
    WorkflowFile: existing
      ? read(draft.gitHub, "WorkflowFile")
      : expected.workflow,
    Environment: existing
      ? (read(draft.gitHub, "Environment") ?? "")
      : (expected.environment ?? ""),
  })) {
    draft.gitHub[`Pending${field}`](value);
  }
  draft.PendingPolicyName(
    `prm-${expected.organization}-${expected.repository}-${expected.workflow}`,
  );
  draft.PushScopeChecked(true);
  draft.PushScope(expected.operations[0]);
  draft.UnlistScopeChecked(false);
  if (typeof draft.StageScopeChecked === "function") {
    draft.StageScopeChecked(false);
  }
  draft.ScopeSubjectsInput(expected.package_ids.join("\n"));
  if (!draft.Valid()) {
    return { ...result, error: "NuGet rejected the exact policy fields" };
  }
  draft.CreateOrEdit();
  return {
    ...result,
    submitted: true,
    action: existing ? "edited" : "created",
    policy_id: existing?.id,
  };
}

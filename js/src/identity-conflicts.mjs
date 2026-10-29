/** Registry normalization follows the registry's identity rules, not processing order. */
export function registryIdentity(registry, name) {
  const normalized = registry === "go-modules" ? name : name.toLowerCase();
  return `${registry}:${registry === "pypi" ? normalized.replace(/[-_.]+/g, "-") : registry === "crates-io" ? normalized.replaceAll("_", "-") : normalized}`;
}

/** Keep all source evidence so a collision cannot be hidden by --package filtering. */
export function identityConflicts(inspections) {
  const groups = new Map();
  for (const inspection of inspections) {
    const repo = inspection.repository ?? {};
    const repository =
      repo.github_owner && repo.github_repository
        ? `${repo.github_owner}/${repo.github_repository}`
        : (repo.root ?? ".");
    for (const item of inspection.packages.filter((p) => p.publishable)) {
      const identity = registryIdentity(item.registry, item.name);
      const source = {
        repository,
        manifest: item.manifest,
        source_artifact: item.generated_alias?.source_manifest ?? item.manifest,
        version: item.version,
        registry_repository: item.registry_repository,
        registry_owners: item.registry_owners,
        permissions: "unknown",
        shared_artifact: item.shared_artifact,
      };
      if (!groups.has(identity)) {
        groups.set(identity, []);
      }
      const group = groups.get(identity);
      if (
        !group.some(
          (entry) =>
            entry.repository === repository && entry.manifest === item.manifest,
        )
      ) {
        group.push(source);
      }
    }
  }
  return [...groups]
    .filter(([, sources]) => sources.length > 1)
    .map(([identity, sources]) => ({ identity, sources }));
}

/** Annotate each source before selection or external mutation. */
export function markIdentityConflicts(inspections) {
  const conflicts = identityConflicts(inspections);
  for (const inspection of inspections) {
    for (const item of inspection.packages) {
      const conflict = conflicts.find(
        (group) =>
          group.identity === registryIdentity(item.registry, item.name),
      );
      if (conflict) {
        item.identity_conflicts = conflict.sources;
      }
    }
  }
  return conflicts;
}

export function validateIdentities(plans) {
  const conflicts = identityConflicts(
    plans.map((plan) => ({
      repository: plan.repository,
      packages: [plan.package],
    })),
  );
  const annotated = plans.filter(
    (plan) => plan.package.identity_conflicts?.length,
  );
  const unresolved = [
    ...conflicts,
    ...annotated.map((p) => ({
      identity: registryIdentity(p.registry, p.package.name),
      sources: p.package.identity_conflicts,
    })),
  ].filter((group) => !reviewedMapping(group, plans));
  if (unresolved.length) {
    throw new Error(
      `conflicting package identities require distinct configured names before setup: ${JSON.stringify(unresolved)}`,
    );
  }
}

/** Require a complete, identical source/version inventory and one selected canonical publisher. */
function reviewedMapping(group, plans) {
  const mapping = group.sources[0].shared_artifact;
  if (
    !mapping?.reviewed ||
    typeof mapping.artifact !== "string" ||
    !mapping.artifact ||
    !mapping.publisher ||
    !Array.isArray(mapping.sources)
  ) {
    return false;
  }
  const key = (source) =>
    JSON.stringify([
      source.repository,
      source.manifest,
      source.version ?? null,
    ]);
  if (
    new Set(mapping.sources.map(key)).size !== group.sources.length ||
    !group.sources.every(
      (source) =>
        mapping.sources.some((entry) => key(entry) === key(source)) &&
        JSON.stringify(source.shared_artifact) === JSON.stringify(mapping),
    )
  ) {
    return false;
  }
  return plans
    .filter(
      (plan) =>
        registryIdentity(plan.registry, plan.package.name) === group.identity,
    )
    .every((plan) => {
      const repo = plan.repository;
      return (
        (repo.github_owner
          ? `${repo.github_owner}/${repo.github_repository}`
          : repo.root) === mapping.publisher.repository &&
        plan.package.manifest === mapping.publisher.manifest
      );
    });
}

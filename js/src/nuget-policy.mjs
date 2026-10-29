/** Identity fields are independent: NuGet profile, package owner and GitHub subject. */
export function validateNugetPolicy(policy) {
  const required = [
    "username",
    "package_owner",
    "organization",
    "repository",
    "workflow",
    "owner_id",
    "repository_id",
  ];
  for (const field of required) {
    if (
      !String(policy[field] ?? "").trim() ||
      /^(?:MyPackage|Your[ -]?Name|your[-_].*|OWNER|REPO|example|placeholder)$/i.test(
        String(policy[field]),
      )
    ) {
      throw new Error(`NuGet policy requires an actual ${field}`);
    }
  }
  if (
    !["owner_id", "repository_id"].every((field) =>
      /^\d+$/.test(String(policy[field])),
    )
  ) {
    throw new Error("NuGet policy requires immutable numeric GitHub IDs");
  }
  if (!/^[\w.-]+\.ya?ml$/.test(policy.workflow)) {
    throw new Error("NuGet policy requires a workflow filename");
  }
  if (
    !Array.isArray(policy.package_ids) ||
    !policy.package_ids.length ||
    policy.package_ids.some(
      (id) =>
        !/^[a-z0-9_.-]+$/i.test(id) ||
        /^(MyPackage|PackageName|example)$/i.test(id),
    )
  ) {
    throw new Error("NuGet policy requires exact, non-template package IDs");
  }
  if (
    !Array.isArray(policy.operations) ||
    policy.operations.length !== 1 ||
    policy.operations.some(
      (operation) =>
        !["package:push", "package:pushversion"].includes(operation),
    )
  ) {
    throw new Error("NuGet policy requires one explicit publish operation");
  }
  return policy;
}

/** Discover an equivalent policy before any creation; activation is fresh evidence. */
export function nugetPolicyState(policies, expected, actualUsername) {
  validateNugetPolicy(expected);
  if (actualUsername?.toLowerCase() !== expected.username.toLowerCase()) {
    return { status: "wrong-account", verified: false };
  }
  const fields = ["package_owner", "organization", "repository"];
  const matches = policies.filter(
    (policy) =>
      fields.every(
        (field) =>
          String(policy[field] ?? "").toLowerCase() ===
          String(expected[field] ?? "").toLowerCase(),
      ) &&
      ["workflow", "environment"].every(
        (field) =>
          String(policy[field] ?? "") === String(expected[field] ?? ""),
      ) &&
      ["package_ids", "operations"].every(
        (field) =>
          JSON.stringify(
            [...(policy[field] ?? [])].map((s) => s.toLowerCase()).sort(),
          ) ===
          JSON.stringify(
            [...expected[field]].map((s) => s.toLowerCase()).sort(),
          ),
      ),
  );
  if (matches.length > 1) {
    return { status: "duplicate", verified: false };
  }
  if (!matches.length) {
    return { status: "missing-or-mismatched", verified: false };
  }
  const policy = matches[0];
  const bound = ["owner_id", "repository_id"].every(
    (field) => String(policy[field] ?? "") === String(expected[field]),
  );
  return {
    status: bound
      ? (policy.activation ?? "unknown")
      : "immutable-identity-unverified",
    verified: bound && policy.activation === "active",
    policy_id: policy.id,
  };
}

/** PackageVerify does not establish push authorization, especially for OIDC push-only keys. */
export function nugetVerificationEvidence(status, body, options = {}) {
  const secrets = options.secrets ?? [];
  let diagnostic = String(body ?? "");
  for (const secret of secrets.filter(Boolean)) {
    diagnostic = diagnostic.replaceAll(secret, "[REDACTED]");
  }
  return {
    http_status: status,
    diagnostic: diagnostic.slice(0, 4000),
    authentication: [401, 403].includes(status)
      ? "rejected-by-verification-endpoint"
      : status >= 200 && status < 300
        ? "accepted-by-verification-endpoint"
        : "unknown",
    authorization: "unknown",
    publication: "unverified",
    reason: options.derivedKey
      ? "derived verification key changes the subject; original policy package scope is unproven"
      : status === 400
        ? "PackageVerify may require package:verify; a push-only key is not proven invalid"
        : "verification endpoint evidence is separate from actual push acceptance",
  };
}

/** OIDC always wins; never validate or rotate a fallback when OIDC succeeded. */
export async function publishNugetArtifact(artifact, adapter) {
  const attempts = [];
  const oidc = adapter.username
    ? await adapter
        .login()
        .catch(() => ({ status: "failed", reason: "OIDC exchange failed" }))
    : { status: "unavailable", reason: "NuGet profile username is missing" };
  attempts.push({
    method: "trusted-publishing",
    status: oidc.status,
    reason: oidc.reason,
  });
  const candidates = [
    { method: "trusted-publishing", value: oidc.key },
    { method: "token-fallback", value: adapter.fallback },
  ].filter((c) => c.value);
  const fingerprint = await adapter.fingerprint(artifact);
  for (const candidate of candidates) {
    if ((await adapter.fingerprint(artifact)) !== fingerprint) {
      throw new Error("validated NuGet artifact changed before retry");
    }
    const result = await adapter.push(artifact, candidate.value);
    attempts.push({
      method: candidate.method,
      status: result.status,
      reason: result.reason,
    });
    if (result.status === "accepted") {
      return {
        selected_method: candidate.method,
        attempts,
        publication: "accepted",
        trust_repaired: candidate.method === "trusted-publishing",
      };
    }
    if (
      !["authentication-failed", "authorization-failed"].includes(result.status)
    ) {
      return {
        selected_method: candidate.method,
        attempts,
        publication: "unverified",
        trust_repaired: false,
      };
    }
    const accepted = await adapter.exactVersionExists(artifact);
    if (accepted === true) {
      return {
        selected_method: candidate.method,
        attempts,
        publication: "already-present",
        trust_repaired: false,
      };
    }
    if (accepted !== false) {
      return {
        selected_method: candidate.method,
        attempts,
        publication: "unknown",
        trust_repaired: false,
      };
    }
  }
  return {
    selected_method: "none",
    attempts,
    publication: "unverified",
    trust_repaired: false,
  };
}

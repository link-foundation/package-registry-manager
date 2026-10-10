/** Registry-specific policy; tokens are never a fallback for OIDC registries. */
export function credentialPolicy(registry) {
  if (
    ["npm", "pypi", "crates-io", "rubygems", "nuget", "jsr"].includes(registry)
  ) {
    return {
      mode: "trusted",
      description: `${registry}: trusted publishing; remove unused long-lived tokens after verification`,
    };
  }
  if (registry === "ghcr") {
    return {
      mode: "github-token",
      description:
        "ghcr: GITHUB_TOKEN with packages: write; no registry secret",
    };
  }
  if (["go-modules", "packagist"].includes(registry)) {
    return {
      mode: "tags",
      description: `${registry}: public repository tags; no publishing token secret`,
    };
  }
  return {
    mode: "token",
    description: `${registry}: scoped expiring token; gh-manager stores selected-repository organization secrets by default`,
  };
}

/** Unknown expiry also requires attention; GitHub cannot return secret values. */
export function needsRotation(metadata, now = Date.now(), leadDays = 7) {
  const expires = Date.parse(metadata.expires_at ?? "");
  return (
    metadata.present !== true ||
    metadata.valid === false ||
    !Number.isFinite(expires) ||
    expires <= now + leadDays * 86_400_000
  );
}

/** Provision, store, validate, then revoke. A failed validation preserves the old token. */
export async function rotateCredential(previous, adapter) {
  let created;
  let stored = false;
  try {
    created = await adapter.create();
    if (
      !created.value ||
      !created.id ||
      !Number.isFinite(Date.parse(created.expires_at)) ||
      Date.parse(created.expires_at) <= Date.now()
    ) {
      throw new Error("registry returned no usable expiring credential");
    }
    stored = true; // A failed ensure can have stored the secret before expiry metadata failed.
    await adapter.store(created);
    await adapter.verify();
    if (previous.token_id && previous.token_id !== created.id) {
      await adapter.revoke(previous.token_id);
      if (!(await adapter.revoked(previous.token_id))) {
        throw new Error("previous token revocation could not be verified");
      }
    }
    return { token_id: created.id, expires_at: created.expires_at };
  } catch (error) {
    if (created?.id && (!stored || adapter.restore)) {
      if (stored) {
        await adapter.restore();
      }
      await adapter.revoke(created.id);
    }
    throw error;
  } finally {
    if (created) {
      created.value = "";
    }
  }
}

/** Token-registry plan; setup obtains the value from its dedicated browser. */
export function credentialSteps(registry, secret) {
  return [
    {
      id: "manage-registry-token",
      title: `Ensure and rotate ${secret}`,
      kind: "api",
      description: `${credentialPolicy(registry).description}. Check gh-manager CI health; keep healthy secrets, create missing or auth-failing credentials in the browser, ensure organization access with repository fallback, test through gh-manager, then revoke the replaced token. Retry once only on an authentication failure.`,
      confirm: true,
    },
  ];
}

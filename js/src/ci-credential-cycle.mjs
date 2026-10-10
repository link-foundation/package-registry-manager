/** Resolve a caller-selected name before making any GitHub or browser mutations. */
export function secretName(template, registry, slug) {
  const [owner, repo] = slug.split("/");
  const normalize = (value) =>
    value.toUpperCase().replaceAll(/[^A-Z0-9_]/g, "_");
  const values = {
    REGISTRY: normalize(registry),
    REPO: normalize(repo),
    ORG: normalize(owner),
    OWNER: normalize(owner),
  };
  const name = template.replaceAll(
    /\{([A-Z]+)\}/g,
    (match, key) => values[key] ?? match,
  );
  if (!/^[A-Z_][A-Z0-9_]{0,244}$/.test(name) || name.startsWith("GITHUB_")) {
    throw new Error("invalid registry secret name or naming template");
  }
  return name;
}

/** CI drives rotation. Uncertain tests preserve every existing credential. */
export async function cycleCredential(adapter) {
  const health = await adapter.health();
  if (health.status === "ok") {
    return { status: "ok", changed: false };
  }
  const previous = await adapter.metadata();
  if (health.status === "unknown" && previous) {
    const tested = await adapter.test();
    if (tested.status === "ok") {
      return { status: "ok", changed: false };
    }
    if (tested.status !== "auth-failing") {
      throw new Error(
        "credential verification is unknown; existing credential retained",
      );
    }
  }
  const candidates = [];
  try {
    for (let attempt = 0; attempt < 2; attempt += 1) {
      const candidate = await adapter.create();
      candidates.push(candidate);
      if (
        !candidate.value ||
        !candidate.id ||
        (candidate.expires_at &&
          !(Date.parse(candidate.expires_at) > Date.now()))
      ) {
        throw new Error("registry returned no usable credential");
      }
      const stored = await adapter.ensure(candidate, {
        status: "auth-failing",
      });
      candidate.value = "";
      const tested = await adapter.test(stored);
      if (tested.status === "ok") {
        const ids = new Set([
          previous?.token_id,
          ...(previous?.token_ids ?? []),
          ...candidates.slice(0, -1).map((item) => item.id),
        ]);
        ids.delete(undefined);
        ids.delete(candidate.id);
        for (const id of ids) {
          await adapter.revoke(id);
          if (!(await adapter.revoked(id))) {
            throw new Error("previous token revocation could not be verified");
          }
        }
        return {
          ...stored,
          status: "ok",
          changed: true,
          token_id: candidate.id,
        };
      }
      if (tested.status !== "auth-failing") {
        throw new Error(
          "credential verification is unknown; old and replacement credentials retained",
        );
      }
    }
    throw new Error(
      "credential verification is auth-failing after two candidates; old credentials retained",
    );
  } finally {
    for (const candidate of candidates) {
      candidate.value = "";
    }
  }
}

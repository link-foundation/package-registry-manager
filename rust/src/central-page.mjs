/* global document */
/** Official Portal UI endpoints, evaluated only in the dedicated signed-in page. */
export default async function centralPage(expected, action = "read") {
  const request = async (path, method = "GET", body) => {
    try {
      const response = await fetch(path, {
        method,
        credentials: "same-origin",
        cache: "no-store",
        headers: {
          accept: "application/json",
          ...(body ? { "content-type": "application/json" } : {}),
        },
        ...(body ? { body: JSON.stringify(body) } : {}),
        signal: AbortSignal.timeout(10000),
      });
      return {
        status: response.status,
        body: response.ok ? await response.json().catch(() => null) : null,
      };
    } catch {
      return { status: "transport" };
    }
  };
  const user = await request("/api/auth/me");
  const account = user.body?.sub;
  if (!account) {
    return {
      status: [401, 403].includes(user.status)
        ? "auth-required"
        : "transport-unknown",
      reason: `Central account HTTP ${user.status}`,
    };
  }
  if (expected.account && account !== expected.account) {
    return { status: "wrong-account", account };
  }
  if (action === "tokens") {
    const tokens = await request("/api/v2/usertokens/list?page_size=100");
    const entries = tokens.body?.tokens;
    const matches = Array.isArray(entries)
      ? entries.filter((token) =>
          expected.token_id
            ? token.id === expected.token_id
            : token.name_code === expected.username,
        )
      : [];
    if (matches.length !== 1) {
      return {
        status: "unknown",
        account,
        reason: `Central token lifetime unreadable (HTTP ${tokens.status})`,
      };
    }
    const token = matches[0];
    const expiry = token.expiration_timestamp;
    return {
      status: "read",
      account,
      token_id: token.id,
      token_name: token.display_name,
      expires_at:
        typeof expiry === "string" &&
        /^\d{4}-\d\d-\d\dT/.test(expiry) &&
        Number.isFinite(Date.parse(expiry))
          ? new Date(expiry).toISOString()
          : null,
      revoked: token.revoked === true,
    };
  }
  const base = "/api/internal/publisher/namespace";
  if (action === "register" || action === "verify") {
    const result = await request(
      action === "register"
        ? base
        : `${base}/verify?namespace=${encodeURIComponent(expected.namespace)}`,
      "POST",
      action === "register"
        ? { namespace: expected.namespace, state: "UNVERIFIED" }
        : undefined,
    );
    if (![200, 201, 202, 204].includes(result.status)) {
      return {
        status: "unverified",
        account,
        reason: `Central ${action} HTTP ${result.status}`,
      };
    }
  }
  const result = await request(base);
  const entries = Array.isArray(result.body)
    ? result.body
    : result.body?.namespaces;
  if (Array.isArray(entries)) {
    const matches = entries.filter(
      (entry) => entry.name === expected.namespace,
    );
    if (matches.length > 1) {
      return { status: "ambiguous", account };
    }
    const entry = matches[0];
    if (
      entry &&
      expected.organization &&
      entry?.owningOrgName !== expected.organization
    ) {
      return {
        status: "wrong-organization",
        account,
        namespace: expected.namespace,
      };
    }
    return {
      status: !entry
        ? "missing"
        : entry.state === "VERIFIED" && !entry.userSuspended
          ? "verified"
          : "unverified",
      account,
      namespace: expected.namespace,
      state: entry?.state,
      verification_key: entry?.verificationKey,
      organization: entry?.owningOrgName,
      namespace_id: entry?.id,
      source: "authenticated-portal",
    };
  }
  // Fallback to the actual signed-in UI. Require a unique exact namespace card.
  const cards = [
    ...document.querySelectorAll('[data-test="namespace-item"]'),
  ].filter((card) =>
    [...card.querySelectorAll('[data-test="namespace-name"]')].some(
      (node) => node.textContent.trim() === expected.namespace,
    ),
  );
  if (
    !expected.organization &&
    cards.length === 1 &&
    /\bVerified\b/.test(cards[0].textContent) &&
    !/Unverified|Verification Pending|Your access suspended/.test(
      cards[0].textContent,
    )
  ) {
    return {
      status: "verified",
      account,
      namespace: expected.namespace,
      source: "authenticated-portal-ui",
    };
  }
  return {
    status: "transport-unknown",
    account,
    reason: `Central namespaces HTTP ${result.status}; authenticated read-back unavailable`,
  };
}

/* global document, CSS */
/** Read the one-time dialog and match its username to server lifetime metadata. */
export default async function readCentralPair() {
  const root =
    document.querySelector('[data-test="view-token-modal"], [role="dialog"]') ??
    document.body;
  const text = (element) =>
    (element?.value ?? element?.textContent ?? "").trim();
  const field = (label) => {
    for (const node of root.querySelectorAll("label, dt, th, span, p, div")) {
      if (
        node.children.length === 0 &&
        new RegExp(`^${label}:?$`, "i").test(text(node))
      ) {
        const target = node.htmlFor
          ? root.querySelector(`#${CSS.escape(node.htmlFor)}`)
          : null;
        const next =
          target ??
          node.nextElementSibling ??
          node.parentElement?.nextElementSibling;
        const value = text(next?.querySelector?.("input, code, pre") ?? next);
        if (value && !/\s/.test(value)) {
          return value;
        }
      }
    }
    return null;
  };
  const pair = { username: field("username"), password: field("password") };
  if (!pair.username || !pair.password) {
    return pair;
  }
  try {
    const response = await fetch("/api/v2/usertokens/list?page_size=100", {
      credentials: "same-origin",
      cache: "no-store",
      signal: AbortSignal.timeout(10000),
    });
    const result = response.ok ? await response.json() : null;
    const matches = (result?.tokens ?? []).filter(
      (entry) => entry.name_code === pair.username,
    );
    if (matches.length !== 1) {
      return { ...pair, expiry_status: "unknown" };
    }
    const token = matches[0];
    const expiry = token.expiration_timestamp;
    const expiresAt =
      typeof expiry === "string" &&
      /^\d{4}-\d\d-\d\dT/.test(expiry) &&
      Number.isFinite(Date.parse(expiry))
        ? new Date(expiry).toISOString()
        : null;
    return {
      ...pair,
      token_id: token.id,
      token_name: token.display_name,
      expires_at: expiresAt,
      expiry_status: expiresAt ? "server-reported" : "unknown",
      revoked: token.revoked === true,
    };
  } catch {
    return { ...pair, expiry_status: "unknown" };
  }
}

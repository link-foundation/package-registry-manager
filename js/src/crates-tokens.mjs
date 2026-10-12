import { pageFetchScript } from "./crates-api.mjs";

/** Fetch private token metadata through the authenticated browser, without values. */
export async function listCratesTokens(page) {
  const result = await page.evaluate(
    pageFetchScript("GET", "/api/v1/me/tokens?expired_days=365"),
  );
  if (result.status !== 200 || !Array.isArray(result.body?.api_tokens)) {
    throw new Error(`crates.io token inventory unavailable (${result.status})`);
  }
  return result.body.api_tokens.map(
    ({ id, name, created_at, expired_at, crate_scopes, endpoint_scopes }) => ({
      id,
      name,
      created_at,
      expired_at,
      crate_scopes,
      endpoint_scopes,
    }),
  );
}

/** GitHub cannot reveal a secret value: refuse absent or ambiguous metadata matches. */
export function selectCratesToken(tokens, secret, metadata, id) {
  const updated = Date.parse(metadata?.updated_at ?? "");
  let matches;
  if (id !== undefined && id !== null) {
    matches = tokens.filter((token) => String(token.id) === String(id));
  } else {
    const named = tokens.filter(
      (token) => token.name?.toUpperCase() === secret.toUpperCase(),
    );
    matches = named.length ? named : tokens;
    if (Number.isFinite(updated)) {
      matches = matches.filter(
        (token) => Math.abs(Date.parse(token.created_at) - updated) <= 300_000,
      );
    } else if (!named.length) {
      matches = [];
    }
  }
  if (
    matches.length !== 1 ||
    matches[0].id === undefined ||
    matches[0].id === null
  ) {
    throw new Error(
      `cannot uniquely identify crates.io token for ${secret}; configure tokens.crates-io.token_id`,
    );
  }
  return matches[0];
}

/** Preserve even legacy null scopes; only the name and expiry change. */
export function replacementTokenRequest(previous, days = 30, now = Date.now()) {
  if (!Number.isInteger(days) || days < 1 || days > 90) {
    throw new Error("token expiry_days must be between 1 and 90");
  }
  for (const key of ["crate_scopes", "endpoint_scopes"]) {
    if (
      previous[key] !== null &&
      (!Array.isArray(previous[key]) ||
        previous[key].some((scope) => typeof scope !== "string"))
    ) {
      throw new Error(
        `crates.io token has no usable ${key}; refusing to broaden scopes`,
      );
    }
  }
  return {
    api_token: {
      name: previous.name,
      crate_scopes: previous.crate_scopes,
      endpoint_scopes: previous.endpoint_scopes,
      expired_at: new Date(now + days * 86_400_000).toISOString(),
    },
  };
}

/** Include expired tokens and the inclusive lead-time boundary. */
export function tokenExpiryFindings(
  tokens,
  account,
  days = 7,
  now = Date.now(),
) {
  if (!Number.isInteger(days) || days < 0 || days > 365) {
    throw new Error("--token-expiry-days must be an integer between 0 and 365");
  }
  return tokens.flatMap((token) => {
    const expiry = Date.parse(token.expired_at ?? "");
    return Number.isFinite(expiry) && expiry <= now + days * 86_400_000
      ? [
          {
            repository: account,
            registry: "crates-io",
            type:
              expiry <= now
                ? "registry-token-expired"
                : "registry-token-expiring",
            evidence: {
              token_id: token.id,
              name: token.name,
              expires_at: token.expired_at,
            },
          },
        ]
      : [];
  });
}

import { npmName, registryEndpoint, USER_AGENT } from "./registry-state.mjs";

/** Bounded punctuation variants, including npm's punctuation-free moniker. */
export function nameVariants(name) {
  const slash = name.lastIndexOf("/");
  const scope = slash < 0 ? "" : name.slice(0, slash + 1);
  const leaf = name.slice(slash + 1);
  const pieces = leaf.split(/[._-]+/).filter(Boolean);
  let variants = [pieces[0]];
  for (const piece of pieces.slice(1)) {
    variants = variants
      .flatMap((prefix) =>
        ["", "-", ".", "_"].map((separator) => prefix + separator + piece),
      )
      .slice(0, 64);
  }
  return [...new Set(variants.map((variant) => scope + variant))].filter(
    (variant) => variant !== name,
  );
}

/** npm dry-run cannot validate unpublished server-side block/similarity rules. */
export async function checkNamePolicy(name, options = {}) {
  if (
    name.length > 214 ||
    !/^(?:@[a-z0-9][a-z0-9._-]*\/)?[a-z0-9][a-z0-9._-]*$/.test(name) ||
    ["node_modules", "favicon.ico"].includes(name)
  ) {
    throw new Error(
      `npm name '${name}' is invalid; choose a lowercase descriptive name or @owner/name`,
    );
  }
  const base = registryEndpoint("npm", options.env ?? process.env);
  for (const variant of [name, ...nameVariants(name)]) {
    const response = await (options.fetch ?? globalThis.fetch)(
      `${base}/${npmName(variant)}?_=${Date.now()}`,
      {
        headers: {
          accept: "application/json",
          "cache-control": "no-cache, no-store",
          "user-agent": USER_AGENT,
        },
        signal: AbortSignal.timeout(options.timeoutMs ?? 10_000),
      },
    );
    if (response.status === 404) {
      continue;
    }
    if (!response.ok) {
      throw new Error(
        `npm name policy lookup failed (${response.status}); no approval requested`,
      );
    }
    if (variant !== name) {
      throw new Error(
        `npm name '${name}' is similar to existing '${variant}'; use @owner/${name} or a longer descriptive name`,
      );
    }
    return;
  }
}

/** Report npm policy refusal once, without entering authentication retry logic. */
export function policyRefusal(output) {
  return (
    /\bE403\b|403 Forbidden/i.test(output) &&
    /too similar|forbidden|blocked|not allowed/i.test(output)
  );
}

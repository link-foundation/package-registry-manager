import { npmName, registryEndpoint, USER_AGENT } from "./registry-state.mjs";

/** Compare punctuation-free monikers and a bounded Damerau edit distance. */
export function similarName(name, candidate) {
  if (name.startsWith("@") || candidate.startsWith("@")) {
    return false;
  }
  const normalize = (value) => value.toLowerCase().replaceAll(/[._-]/g, "");
  const a = normalize(name),
    b = normalize(candidate);
  if (a === b) {
    return true;
  }
  if (Math.abs(a.length - b.length) > 2 || a.length > 214 || b.length > 214) {
    return false;
  }
  const rows = Array.from({ length: a.length + 1 }, (_, i) =>
    Array.from({ length: b.length + 1 }, (_, j) =>
      i === 0 ? j : j === 0 ? i : 0,
    ),
  );
  for (let i = 1; i <= a.length; i += 1) {
    for (let j = 1; j <= b.length; j += 1) {
      rows[i][j] = Math.min(
        rows[i - 1][j] + 1,
        rows[i][j - 1] + 1,
        rows[i - 1][j - 1] + Number(a[i - 1] !== b[j - 1]),
      );
      if (i > 1 && j > 1 && a[i - 1] === b[j - 2] && a[i - 2] === b[j - 1]) {
        rows[i][j] = Math.min(rows[i][j], rows[i - 2][j - 2] + 1);
      }
    }
  }
  return (
    rows[a.length][b.length] <= (Math.min(a.length, b.length) >= 6 ? 2 : 1)
  );
}

async function lookup(base, route, options) {
  return (options.fetch ?? globalThis.fetch)(`${base}/${route}`, {
    headers: {
      accept: "application/json",
      "cache-control": "no-cache, no-store",
      "user-agent": USER_AGENT,
    },
    signal: AbortSignal.timeout(options.timeoutMs ?? 10_000),
  });
}

/** Check a finite set of proposed names; only a 404 means currently free. */
export async function freeAlternatives(name, options = {}) {
  const leaf = name.split("/").at(-1);
  const scope = options.npmScope?.replace(/^@/, "");
  const candidates = [
    ...new Set([
      ...(options.nameAlternatives ?? []),
      ...(scope ? [`@${scope}/${leaf}`] : []),
      `${leaf}-package`,
      `${leaf}-toolkit`,
    ]),
  ]
    .filter((value) => value !== name)
    .slice(0, 8);
  const base = registryEndpoint("npm", options.env ?? process.env);
  const free = [];
  for (const candidate of candidates) {
    if (
      !/^(?:@[a-z0-9][a-z0-9._-]*\/)?[a-z0-9][a-z0-9._-]*$/.test(candidate) ||
      candidate.length > 214
    ) {
      continue;
    }
    try {
      if (
        (await lookup(base, `${npmName(candidate)}?_=${Date.now()}`, options))
          .status === 404
      ) {
        free.push(candidate);
      }
    } catch {
      /* Unknown availability is never reported as free. */
    }
  }
  return free;
}

/** A naming error includes verified alternatives without requesting credentials. */
export async function namingError(name, conflicting, options = {}) {
  const free = await freeAlternatives(name, options);
  return new Error(
    `npm name '${name}' may be refused as too similar to existing '${conflicting}'; ${free.length ? `currently free alternatives: ${free.join(", ")} (scope ownership and npm policy still apply)` : "choose your own npm scope or a longer descriptive name; availability could not be confirmed"}. Rename package.json and retry; no authentication retry was attempted.`,
  );
}

/** Public npm search is a bounded heuristic, not the private registry policy. */
export async function checkSimilarity(name, options = {}) {
  if (name.startsWith("@")) {
    return;
  }
  const base = registryEndpoint("npm", options.env ?? process.env);
  // npm search does not always correct typos; verify common public monikers.
  for (const candidate of [
    "lodash",
    "express",
    "react",
    "typescript",
    "webpack",
    "eslint",
    "prettier",
    "commander",
    "axios",
    "chalk",
    "debug",
    "dotenv",
    "yargs",
    "semver",
    "glob",
    "minimist",
    "moment",
    "jquery",
    "rxjs",
    "tslib",
  ]) {
    if (
      similarName(name, candidate) &&
      (await lookup(base, candidate, options)).ok
    ) {
      throw await namingError(name, candidate, options);
    }
  }
  const queries = [...new Set([name, name.split(/[._-]/).at(-1)])].filter(
    Boolean,
  );
  for (const query of queries) {
    const response = await lookup(
      base,
      `-/v1/search?text=${encodeURIComponent(query)}&size=250&popularity=1&quality=0&maintenance=0`,
      options,
    );
    if (!response.ok) {
      throw new Error(
        `npm similarity search failed (${response.status}); no approval requested`,
      );
    }
    const document = await response.json();
    for (const item of (document.objects ?? []).slice(0, 250)) {
      const candidate = item.package?.name;
      if (candidate && similarName(name, candidate)) {
        throw await namingError(name, candidate, options);
      }
    }
  }
}

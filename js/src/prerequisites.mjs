import { exec } from "command-stream";

import { automatedDescription } from "./browser-options.mjs";
import { getJson, registryEndpoint } from "./registry-state.mjs";

/** The Node.js versions npm 11 runs on. */
export const NPM_11_ENGINES = "^20.17.0 || >=22.9.0";
/** The Node.js versions npm 12 runs on. */
export const NPM_12_ENGINES = "^22.22.2 || ^24.15.0 || >=26.0.0";
/** Where an npm account turns on two-factor authentication. */
export const NPM_TFA_URL =
  "https://docs.npmjs.com/configuring-two-factor-authentication/";
/** The npm that runs `npm trust` when Node.js cannot run npm 12. */
export const DEFAULT_TRUST_NPM = "npm@^11.10";

/** Parses `v24.15.0` or `24.15.0` into numbers, or returns null. */
export function parseVersion(text) {
  const match = /^v?(\d+)\.(\d+)\.(\d+)/.exec(String(text ?? "").trim());
  return match ? match.slice(1, 4).map(Number) : null;
}

function atLeast(version, minimum) {
  for (let index = 0; index < 3; index += 1) {
    if (version[index] !== minimum[index]) {
      return version[index] > minimum[index];
    }
  }
  return true;
}

/** Whether a Node.js version satisfies npm 11's engines. */
export function supportsNpm11(nodeVersion) {
  const version = parseVersion(nodeVersion);
  return Boolean(
    version &&
    ((version[0] === 20 && atLeast(version, [20, 17, 0])) ||
      atLeast(version, [22, 9, 0])),
  );
}

/** Whether a Node.js version satisfies npm 12's engines. */
export function supportsNpm12(nodeVersion) {
  const version = parseVersion(nodeVersion);
  return Boolean(
    version &&
    ((version[0] === 22 && atLeast(version, [22, 22, 2])) ||
      (version[0] === 24 && atLeast(version, [24, 15, 0])) ||
      version[0] >= 26),
  );
}

/**
 * The npm package spec that runs `npm trust` through npx: npm 12 only when the
 * Node.js on PATH satisfies its engines, otherwise npm 11.10 or newer.
 */
export function trustNpmSpec(nodeVersion) {
  return supportsNpm12(nodeVersion) ? "npm@^12" : DEFAULT_TRUST_NPM;
}

/**
 * The version a `npm@^N` spec resolves to, from the registry's `next-N`
 * dist-tag (or `latest` when it has the same major version).
 */
export function resolveTrustNpm(spec, distTags) {
  const major = /\^(\d+)/.exec(spec)?.[1];
  if (!major || !distTags) {
    return null;
  }
  const candidates = [distTags[`next-${major}`], distTags.latest];
  return (
    candidates.find((version) => parseVersion(version)?.[0] === +major) ?? null
  );
}

/**
 * Reads `tfa` from `npm profile get --json`: the 2FA mode, or null when
 * two-factor authentication is off or still pending. Throws on bad JSON.
 */
export function twoFactorMode(output) {
  const tfa = JSON.parse(String(output)).tfa;
  return tfa && typeof tfa === "object" && !tfa.pending && tfa.mode
    ? String(tfa.mode)
    : null;
}

/** Summarizes `gh auth status --json hosts`, or returns null. */
export function githubAuth(output) {
  let hosts;
  try {
    hosts = JSON.parse(String(output)).hosts ?? {};
  } catch {
    return null;
  }
  const accounts = [
    ...(hosts["github.com"] ?? []),
    ...Object.entries(hosts)
      .filter(([host]) => host !== "github.com")
      .flatMap(([, entries]) => entries),
  ];
  const account =
    accounts.find((entry) => entry.active && entry.state === "success") ?? null;
  if (!account) {
    return { login: null, scopes: [] };
  }
  return {
    login: account.login ?? null,
    scopes: String(account.scopes ?? "")
      .split(",")
      .map((scope) => scope.trim())
      .filter(Boolean),
  };
}

async function probe(program, args, verbose) {
  if (verbose) {
    console.error(`+ ${[program, ...args].join(" ")}`);
  }
  try {
    const result = await exec(program, args, {
      capture: true,
      mirror: false,
      stdin: "ignore",
    });
    return { code: result.code, stdout: String(result.stdout ?? "") };
  } catch {
    return { code: 127, stdout: "" };
  }
}

/**
 * Looks up the local tools the setup flows depend on: the Node.js and npm on
 * PATH, the npm that runs `npm trust`, the npm account's 2FA state, and the
 * GitHub CLI's scopes. Network lookups are skipped with `offline`.
 */
export async function probeEnvironment(options = {}) {
  const { offline = false, verbose = false, npm = true } = options;
  const [node, npmVersion, profile, gh] = await Promise.all([
    probe("node", ["--version"], verbose),
    npm ? probe("npm", ["--version"], verbose) : null,
    npm && !offline
      ? probe("npm", ["profile", "get", "--json"], verbose)
      : null,
    offline
      ? null
      : probe("gh", ["auth", "status", "--json", "hosts"], verbose),
  ]);
  const nodeVersion = node.code === 0 ? node.stdout.trim() : null;
  const spec = trustNpmSpec(nodeVersion);
  const distTags =
    npm && !offline
      ? await getJson(`${registryEndpoint("npm")}/-/package/npm/dist-tags`, {
          verbose,
        })
      : null;
  return {
    offline,
    node: nodeVersion,
    npm: npmVersion?.code === 0 ? npmVersion.stdout.trim() : null,
    trustNpm: spec,
    trustNpmVersion: resolveTrustNpm(spec, distTags),
    twoFactor: twoFactorState(profile),
    github: githubState(gh),
  };
}

function twoFactorState(result) {
  if (!result) {
    return { state: "unchecked" };
  }
  if (result.code !== 0) {
    return { state: "unknown" };
  }
  try {
    const mode = twoFactorMode(result.stdout);
    return mode ? { state: "enabled", mode } : { state: "off" };
  } catch {
    return { state: "unknown" };
  }
}

function githubState(result) {
  if (!result) {
    return { state: "unchecked" };
  }
  if (result.code === 127) {
    return { state: "missing" };
  }
  const auth = githubAuth(result.stdout);
  if (!auth) {
    return { state: "unknown" };
  }
  return auth.login
    ? { state: "signed-in", login: auth.login, scopes: auth.scopes }
    : { state: "signed-out" };
}

const prerequisite = (id, title, detected, required, ok) => ({
  id,
  title,
  detected,
  required,
  ok,
});

/**
 * The manual prerequisites of a plan, listed before its steps: tool versions,
 * the npm account's 2FA state, the GitHub CLI scopes, and where browser pages
 * open. `browser` is `{ mode, channel, profile }` with mode `default`,
 * `automated`, or `none`.
 */
export function planPrerequisites(plan, environment, browser = {}) {
  if (!environment || plan.steps.length === 0) {
    return [];
  }
  const items = [];
  const unchecked = environment.offline
    ? "not checked (--offline)"
    : "not checked";
  if (plan.registry === "npm") {
    const node = environment.node;
    const nodeOk = node ? supportsNpm11(node) : false;
    items.push(
      prerequisite(
        "node",
        "Node.js",
        node ?? "not found",
        NPM_11_ENGINES,
        nodeOk,
      ),
      prerequisite(
        "npm",
        "npm",
        environment.npm ?? "not found",
        "installed",
        Boolean(environment.npm),
      ),
      prerequisite(
        "npm-trust",
        "npm for npm trust",
        environment.trustNpmVersion
          ? `${environment.trustNpm}, resolves to ${environment.trustNpmVersion}`
          : environment.trustNpm,
        `npm 11.10 or newer; npm 12 only on Node.js ${NPM_12_ENGINES}`,
        nodeOk,
      ),
    );
    const tfa = environment.twoFactor;
    const tfaDetected = {
      enabled: tfa.mode,
      off: "off",
      unknown: "unknown (sign in to npm first)",
    }[tfa.state];
    items.push(
      prerequisite(
        "npm-2fa",
        "npm two-factor authentication",
        tfaDetected ?? unchecked,
        `enabled at ${NPM_TFA_URL}; npm trust requires it`,
        { enabled: true, off: false }[tfa.state] ?? null,
      ),
    );
  }
  if (plan.steps.some((step) => step.command?.program === "gh")) {
    const github = environment.github;
    const scopes =
      github.scopes?.length > 0 ? github.scopes.join(", ") : "none listed";
    const detected = {
      "signed-in": `signed in as ${github.login} (scopes: ${scopes})`,
      "signed-out": "not signed in",
      missing: "not installed",
      unknown: "unknown",
    }[github.state];
    let ok = null;
    if (github.state === "signed-in" && github.scopes.length > 0) {
      ok = github.scopes.includes("repo");
    } else if (["signed-out", "missing"].includes(github.state)) {
      ok = false;
    }
    items.push(
      prerequisite(
        "gh",
        "GitHub CLI",
        detected ?? unchecked,
        "signed in with the repo scope, for gh secret and gh run",
        ok,
      ),
    );
  }
  items.push(
    prerequisite(
      "browser",
      "Browser",
      browserDescription(browser),
      "signed in to the registry, or ready to sign in",
      null,
    ),
  );
  return items;
}

function browserDescription({ mode = "default", ...browser }) {
  if (mode === "none") {
    return "none; URLs are printed (--no-browser)";
  }
  if (mode === "automated") {
    return automatedDescription(browser);
  }
  return `your default browser; forms open in ${automatedDescription({ ...browser, profile: undefined })}`;
}

/** Renders prerequisites as indented text lines. */
export function renderPrerequisites(items) {
  if (items.length === 0) {
    return [];
  }
  return [
    "  prerequisites:",
    ...items.map(
      (item) =>
        `    - ${item.title}: ${item.detected}; needs ${item.required}${item.ok === false ? " [action needed]" : ""}`,
    ),
  ];
}

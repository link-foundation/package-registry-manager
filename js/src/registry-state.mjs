/** Identifies the tool to registry APIs, as crates.io asks of clients. */
export const USER_AGENT =
  "package-registry-manager (+https://github.com/link-foundation/package-registry-manager)";

const ENDPOINTS = {
  npm: ["PACKAGE_REGISTRY_MANAGER_NPM_REGISTRY", "https://registry.npmjs.org"],
  "crates-io": [
    "PACKAGE_REGISTRY_MANAGER_CRATES_IO_API",
    "https://crates.io/api/v1",
  ],
  pypi: ["PACKAGE_REGISTRY_MANAGER_PYPI_API", "https://pypi.org"],
  "docker-hub": [
    "PACKAGE_REGISTRY_MANAGER_DOCKER_HUB_API",
    "https://hub.docker.com/v2",
  ],
};

/** Returns the registry API base URL, honoring test overrides. */
export function registryEndpoint(registry, env = process.env) {
  const [variable, fallback] = ENDPOINTS[registry];
  return trimTrailingSlashes(env[variable] || fallback);
}

// A loop instead of /\/+$/, which backtracks quadratically on long runs of "/".
function trimTrailingSlashes(value) {
  let end = value.length;
  while (end > 0 && value[end - 1] === "/") {
    end -= 1;
  }
  return value.slice(0, end);
}

/** Returns the public URL that answers whether a package exists. */
export function registryStateUrl(packageInfo, env = process.env) {
  const { registry, name } = packageInfo;
  if (!ENDPOINTS[registry]) {
    return null;
  }
  const base = registryEndpoint(registry, env);
  switch (registry) {
    case "npm":
      return `${base}/${npmName(name)}/latest`;
    case "crates-io":
      return `${base}/crates/${encodeURIComponent(name)}`;
    case "pypi":
      return `${base}/pypi/${encodeURIComponent(name)}/json`;
    default: {
      const [namespace, repository] = name.split("/");
      return `${base}/namespaces/${encodeURIComponent(namespace)}/repositories/${encodeURIComponent(repository)}`;
    }
  }
}

/** Encodes an npm package name for a registry document URL. */
export function npmName(name) {
  return name.startsWith("@") ? `@${encodeURIComponent(name.slice(1))}` : name;
}

/**
 * Looks up whether each publishable package already exists and whether its
 * latest release was published through trusted publishing. Unknown state is
 * left unset so plans keep every conditional step.
 */
export async function probeRegistryState(inspection, options = {}) {
  const probed = structuredClone(inspection);
  await Promise.all(
    probed.packages.map(async (packageInfo) => {
      if (!packageInfo.publishable) {
        return;
      }
      const state = await probePackage(packageInfo, options);
      if (state.exists !== undefined) {
        packageInfo.exists_on_registry = state.exists;
      }
      if (state.trusted !== undefined) {
        packageInfo.trusted_publishing = state.trusted;
      }
    }),
  );
  return probed;
}

/**
 * Probes one package; returns `{ exists, trusted, version }` with unknown
 * fields unset.
 */
export async function probePackage(packageInfo, options = {}) {
  const env = options.env ?? process.env;
  const url = registryStateUrl(packageInfo, env);
  if (!url) {
    return {};
  }
  const document = await getJson(url, options);
  if (document === undefined) {
    return {};
  }
  if (document === null) {
    return packageInfo.registry === "docker-hub"
      ? { exists: false }
      : { exists: false, trusted: false };
  }
  switch (packageInfo.registry) {
    case "npm":
      return {
        exists: true,
        trusted: Boolean(document._npmUser?.trustedPublisher),
        version: document.version,
      };
    case "crates-io":
      return {
        exists: true,
        trusted: (document.versions ?? []).some((item) =>
          Boolean(item.trustpub_data),
        ),
      };
    case "pypi":
      return { exists: true, trusted: await pypiProvenance(document, options) };
    default:
      return { exists: true };
  }
}

async function pypiProvenance(document, options) {
  const file = document.urls?.[0]?.filename;
  const version = document.info?.version;
  if (!file || !version) {
    return false;
  }
  const base = registryEndpoint("pypi", options.env ?? process.env);
  const url = `${base}/integrity/${encodeURIComponent(document.info.name)}/${encodeURIComponent(version)}/${encodeURIComponent(file)}/provenance`;
  return Boolean(await getJson(url, options));
}

/**
 * Fetches a JSON document: returns the parsed body, `null` for 404, or
 * `undefined` when the state cannot be determined.
 */
export async function getJson(url, options = {}) {
  const fetcher = options.fetch ?? globalThis.fetch;
  try {
    const response = await fetcher(url, {
      headers: { accept: "application/json", "user-agent": USER_AGENT },
      signal: AbortSignal.timeout(options.timeoutMs ?? 10_000),
    });
    if (options.verbose) {
      console.error(`GET ${url} -> ${response.status}`);
    }
    if (response.status === 404) {
      return null;
    }
    return response.ok ? await response.json() : undefined;
  } catch (error) {
    if (options.verbose) {
      console.error(`GET ${url} failed: ${error.message}`);
    }
    return undefined;
  }
}

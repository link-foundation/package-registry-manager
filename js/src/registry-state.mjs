import {
  compareRepositories,
  identityCommand,
  inspectManifestRepositories,
  provenanceRepositories,
  publisherIdentities,
  publisherMatches,
  resolveRepository,
} from "./repository-identity.mjs";

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
  await resolveRepository(probed, options);
  await inspectManifestRepositories(probed);
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
      const evidence = { ...packageInfo, ...state };
      if (state.configured_publishers !== undefined) {
        packageInfo.configured_publishers = state.configured_publishers;
      }
      if (state.provenance_repositories?.length) {
        packageInfo.provenance_repositories = state.provenance_repositories;
      }
      if (packageInfo.registry === "npm" && state.exists) {
        const output = await identityCommand(
          "npm",
          ["trust", "list", packageInfo.name, "--browser=false"],
          { ...options, repository: inspection.repository.root },
        );
        if (output !== undefined) {
          evidence.configured_publishers = publisherIdentities(output);
          packageInfo.configured_publishers = evidence.configured_publishers;
        }
      }
      compareRepositories(packageInfo, probed.repository, evidence);
      if (
        state.trusted &&
        ["npm", "crates-io", "pypi"].includes(packageInfo.registry)
      ) {
        const expected = {
          organization: probed.repository.github_owner,
          repository: probed.repository.github_repository,
          workflow: packageInfo.workflow,
          environment: packageInfo.environment,
        };
        packageInfo.publisher_settings_verified = Boolean(
          evidence.configured_publishers?.some((publisher) =>
            publisherMatches(publisher, expected),
          ),
        );
        if (!packageInfo.publisher_settings_verified) {
          (packageInfo.warnings ??= []).push(
            "configured trusted publisher could not be verified; setup must check the repository, workflow and environment in registry settings",
          );
        }
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
    case "npm": {
      const attestations = document.dist?.attestations?.url;
      const provenance = attestations
        ? await getJson(attestations, options)
        : undefined;
      const sources = provenanceRepositories(provenance);
      return {
        exists: true,
        trusted: Boolean(document._npmUser?.trustedPublisher),
        version: document.version,
        ...(sources.length ? { provenance_repositories: sources } : {}),
      };
    }
    case "crates-io": {
      const latest =
        (document.versions ?? []).find(
          (item) => item.num === document.crate?.max_version,
        ) ?? document.versions?.[0];
      const settings = await getJson(
        `${registryEndpoint("crates-io", env)}/trusted_publishing/github_configs?crate=${encodeURIComponent(packageInfo.name)}`,
        options,
      );
      return {
        exists: true,
        trusted: Boolean(latest?.trustpub_data),
        provenance_repositories: publisherIdentities(latest?.trustpub_data).map(
          (publisher) => publisher.repository,
        ),
        ...(Array.isArray(settings?.github_configs) && !settings.meta?.next_page
          ? { configured_publishers: publisherIdentities(settings) }
          : {}),
      };
    }
    case "pypi": {
      const provenance = await pypiProvenance(document, options);
      return {
        exists: true,
        trusted: Boolean(provenance),
        provenance_repositories: provenanceRepositories(provenance),
      };
    }
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
  return getJson(url, options);
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

/** Normalize NuGet identity versions for flat-container comparisons. */
export function normalizedNugetVersion(version) {
  const release =
    /^([0-9]+(?:\.[0-9]+){0,3})(-[0-9a-z-]+(?:\.[0-9a-z-]+)*)?(?:\+[0-9a-z-]+(?:\.[0-9a-z-]+)*)?$/i.exec(
      version ?? "",
    );
  if (!release) {
    return undefined;
  }
  const parts = release[1].split(".").map((part) => BigInt(part).toString());
  while (parts.length < 3) {
    parts.push("0");
  }
  if (parts.length === 4 && parts[3] === "0") {
    parts.pop();
  }
  return parts.join(".") + (release[2]?.toLowerCase() ?? "");
}

/** Exact-version evidence is independent from name presence and publisher settings. */
export async function versionEvidence(item, document, url, options, getJson) {
  const expected = item.version;
  const state = { registry_evidence_url: url };
  let versions;
  switch (item.registry) {
    case "npm": {
      state.latest_version = document.version;
      state.registry_owners = (document.maintainers ?? []).map(
        (owner) => owner.name,
      );
      state.registry_repository =
        document.repository?.url ?? document.repository;
      if (expected) {
        state.version_evidence_url = `${url.slice(0, -"latest".length)}${encodeURIComponent(expected)}`;
        const exact = await getJson(state.version_evidence_url, options);
        if (exact === null || exact?.version === expected) {
          state.manifest_version_exists = exact !== null;
        } else {
          state.registry_state_reason =
            "exact-version lookup failed or returned invalid metadata";
        }
      }
      break;
    }
    case "crates-io":
      state.latest_version = document.crate?.max_version;
      state.registry_repository = document.crate?.repository;
      state.version_evidence_url = expected
        ? `${url}/${encodeURIComponent(expected)}`
        : undefined;
      if (expected) {
        const exact = await getJson(state.version_evidence_url, options);
        if (exact === null || exact?.version?.num === expected) {
          state.manifest_version_exists = exact !== null;
        } else {
          state.registry_state_reason =
            "exact-version lookup failed or returned invalid metadata";
        }
      }
      break;
    case "pypi":
      state.latest_version = document.info?.version;
      versions = Object.keys(document.releases ?? {});
      break;
    case "nuget":
      versions = document.versions;
      state.latest_version = versions?.at(-1);
      break;
    case "packagist":
      versions = document.packages?.[item.name]?.map((value) =>
        value.version.replace(/^v/, ""),
      );
      state.latest_version = document.packages?.[item.name]?.[0]?.version;
      break;
    case "jsr":
      state.latest_version = document.latest;
      versions = Object.keys(document.versions ?? {});
      break;
    default:
      state.registry_state_reason =
        "exact-version lookup is unsupported for this provider";
  }
  if (expected && Array.isArray(versions)) {
    state.manifest_version_exists = versions.some((version) =>
      item.registry === "nuget"
        ? normalizedNugetVersion(expected) !== undefined &&
          normalizedNugetVersion(version) === normalizedNugetVersion(expected)
        : version ===
          (item.registry === "packagist"
            ? expected.replace(/^v/, "")
            : expected),
    );
    state.version_evidence_url = url;
  }
  if (
    expected &&
    state.manifest_version_exists === undefined &&
    state.registry_state_reason
  ) {
    state.version_state = "manifest-version-unknown";
  }
  if (!expected) {
    state.version_state = "manifest-version-unresolved";
  } else if (state.manifest_version_exists === false) {
    state.version_state = olderVersion(expected, state.latest_version)
      ? "stale-manifest-version"
      : "manifest-version-absent";
  }
  return Object.fromEntries(
    Object.entries(state).filter(([, value]) => value !== undefined),
  );
}

/** Compare numeric release components only; incomparable versions stay absent, not stale. */
export function olderVersion(expected, latest) {
  if (
    !/^v?\d+\.\d+\.\d+$/.test(expected) ||
    !/^v?\d+\.\d+\.\d+$/.test(latest ?? "")
  ) {
    return false;
  }
  const left = expected.replace(/^v/, "").split(".").map(BigInt);
  const right = latest.replace(/^v/, "").split(".").map(BigInt);
  for (let i = 0; i < 3; i += 1) {
    if (left[i] !== right[i]) {
      return left[i] < right[i];
    }
  }
  return false;
}

/** Read-only providers whose metadata is not JSON, with explicit uncertainty. */
export async function distributionState(item, options, base) {
  const fetcher = options.fetch ?? globalThis.fetch;
  const read = async (url) => {
    try {
      const response = await fetcher(url, {
        signal: AbortSignal.timeout(10_000),
      });
      if (response.status === 404) {
        return null;
      }
      if (!response.ok) {
        options.onError?.({
          stage: "registry",
          http_status: response.status,
          message: `registry lookup returned HTTP ${response.status}`,
        });
        return undefined;
      }
      return await response.text();
    } catch {
      return undefined;
    }
  };
  if (item.registry === "maven-central") {
    if (!/^[\w.-]+:[\w.-]+$/.test(item.name)) {
      return { registry_state_reason: "unresolved Maven coordinates" };
    }
    const [group, artifact] = item.name.split(":");
    const directory = `${base}/${group.replaceAll(".", "/")}/${artifact}`;
    const url = `${directory}/maven-metadata.xml`;
    const metadata = await read(url);
    const state = { registry_evidence_url: url };
    if (
      metadata === null ||
      (typeof metadata === "string" && /<metadata[\s>]/.test(metadata))
    ) {
      state.exists = metadata !== null;
    }
    state.latest_version = /<(?:release|latest)>([^<]+)<\//.exec(
      metadata ?? "",
    )?.[1];
    if (item.version && !/[${}/\\]/.test(item.version)) {
      state.version_evidence_url = `${directory}/${encodeURIComponent(item.version)}/${artifact}-${encodeURIComponent(item.version)}.pom`;
      const exact = await read(state.version_evidence_url);
      if (
        exact === null ||
        (typeof exact === "string" && /<project[\s>]/.test(exact))
      ) {
        state.manifest_version_exists = exact !== null;
      } else {
        state.registry_state_reason =
          "Maven exact-version lookup failed or returned invalid metadata";
      }
      if (state.manifest_version_exists) {
        state.exists = true;
      }
    }
    if (state.exists === undefined) {
      state.registry_state_reason =
        "Maven lookup failed or returned invalid metadata";
    }
    if (!item.version) {
      state.version_state = "manifest-version-unresolved";
    } else if (state.manifest_version_exists === false) {
      state.version_state = olderVersion(item.version, state.latest_version)
        ? "stale-manifest-version"
        : "manifest-version-absent";
    }
    return state;
  }
  if (item.registry === "go-modules") {
    const module = item.name
      .replace(/[A-Z]/g, (c) => `!${c.toLowerCase()}`)
      .split("/")
      .map(encodeURIComponent)
      .join("/");
    const url = `${base}/${module}/@v/list`;
    const list = await read(url);
    const latest = await read(`${base}/${module}/@latest`);
    let version;
    try {
      const candidate = JSON.parse(latest)?.Version;
      if (
        typeof candidate === "string" &&
        /^v[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9a-z-]+(?:\.[0-9a-z-]+)*)?(?:\+[0-9a-z-]+(?:\.[0-9a-z-]+)*)?$/i.test(
          candidate,
        )
      ) {
        version = candidate;
      }
    } catch {
      /* Non-JSON is unknown. */
    }
    return {
      registry_evidence_url: url,
      ...(!version && !(list === null && latest === null)
        ? {
            registry_state_reason:
              "Go proxy lookup failed or returned invalid metadata",
          }
        : {}),
      ...(version
        ? { exists: true, latest_version: version }
        : list === null && latest === null
          ? { exists: false }
          : {}),
      ...(typeof list === "string"
        ? { tagged_release_exists: list.trim().length > 0 }
        : {}),
      ...(typeof list === "string" && !list.trim() && version
        ? { version_state: "no-tagged-release" }
        : {}),
    };
  }
  return {
    registry_state_reason:
      "public registry lookup is unsupported; publication is unverified",
  };
}

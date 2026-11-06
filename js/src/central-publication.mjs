/** Verify the exact public Gradle graph without executing Gradle or publishing. */
export async function verifyCentralPublication(session, step) {
  const { group, artifact, version } = step.coordinates ?? {};
  const valid = (value) =>
    typeof value === "string" && /^[A-Za-z0-9_.+-]+$/.test(value);
  if (![group, artifact, version].every(valid)) {
    return {
      status: "unknown",
      reason: "the exact coordinates are unresolved",
    };
  }
  const base = "https://repo1.maven.org/maven2/";
  const path = (g, a, ext) =>
    `${base}${g.replaceAll(".", "/")}/${a}/${version}/${a}-${version}.${ext}`;
  const request = async (url, json = false) => {
    try {
      const response = await (session.options.fetch ?? globalThis.fetch)(url, {
        method: json ? "GET" : "HEAD",
        signal: AbortSignal.timeout(session.options.timeoutMs ?? 10000),
      });
      if (session.options.verbose) {
        console.error(`${json ? "GET" : "HEAD"} ${url} -> ${response.status}`);
      }
      return {
        status: response.status,
        body:
          json && response.ok ? await response.json().catch(() => null) : null,
      };
    } catch {
      return { status: "transport" };
    }
  };
  const root = await request(path(group, artifact, "pom"));
  if (root.status !== 200) {
    return {
      status: root.status === 404 ? "unpublished" : "transport-unknown",
      reason: `root POM HTTP ${root.status}`,
    };
  }
  const evidence = {
    root_pom: "available",
    artifacts: "unverified",
    signatures: "unverified",
    consumers: "unverified",
    release: "unverified",
  };
  if (!session.plan.package.gradle?.multiplatform) {
    return { status: "verified", evidence };
  }
  const queue = [[group, artifact]];
  const seen = new Set();
  const files = new Set();
  const queued = new Set([`${group}:${artifact}`]);
  let rootMissing = false;
  const missing = [];
  const malformed = [];
  const transport = [];
  const rootUrl = path(group, artifact, "module");
  const resolve = (url, parent) => {
    if (typeof url !== "string" || !url || /[?#\\]/.test(url)) {
      return null;
    }
    try {
      const resolved = new URL(url, parent);
      return resolved.href.startsWith(base) &&
        resolved.origin === new URL(base).origin
        ? resolved.href
        : null;
    } catch {
      return null;
    }
  };
  for (let index = 0; index < queue.length; index++) {
    const [g, a] = queue[index];
    const coordinate = `${g}:${a}`;
    if (seen.has(coordinate)) {
      continue;
    }
    seen.add(coordinate);
    if (seen.size > 128) {
      malformed.push("metadata graph exceeds 128 coordinates");
      break;
    }
    const url = path(g, a, "module");
    if (index) {
      const pom = await request(path(g, a, "pom"));
      if (pom.status !== 200) {
        (pom.status === 404 ? missing : transport).push(
          `${coordinate} POM (HTTP ${pom.status})`,
        );
      }
    }
    const response = await request(url, true);
    if (response.status !== 200) {
      if (index === 0 && response.status === 404) {
        rootMissing = true;
      }
      (response.status === 404 ? missing : transport).push(
        `${coordinate} module (HTTP ${response.status})`,
      );
      continue;
    }
    const doc = response.body;
    if (
      !doc ||
      doc.formatVersion !== "1.1" ||
      !Array.isArray(doc.variants) ||
      !doc.variants.length ||
      doc.variants.length > 4096
    ) {
      malformed.push(`${coordinate}: invalid Gradle 1.1 metadata`);
      continue;
    }
    const component = doc.component;
    const own =
      component?.group === g &&
      component?.module === a &&
      component?.version === version;
    const redirected =
      component?.group === group &&
      component?.module === artifact &&
      component?.version === version &&
      resolve(component?.url, url) === rootUrl;
    if (!own && !redirected) {
      malformed.push(`${coordinate}: incorrect component identity`);
      continue;
    }
    for (const variant of doc.variants) {
      if (
        !variant ||
        typeof variant.name !== "string" ||
        (variant["available-at"] && variant.files)
      ) {
        malformed.push(`${coordinate}: invalid variant`);
        continue;
      }
      const pointer = variant["available-at"];
      if (pointer !== undefined) {
        if (
          !pointer ||
          ![pointer.group, pointer.module, pointer.version].every(valid) ||
          pointer.version !== version
        ) {
          malformed.push(
            `${coordinate}: available-at must name a group/module at exact version ${version}`,
          );
          continue;
        }
        if (
          resolve(pointer.url, url) !==
          path(pointer.group, pointer.module, "module")
        ) {
          malformed.push(
            `${coordinate}: available-at URL disagrees with coordinates`,
          );
          continue;
        }
        const target = `${pointer.group}:${pointer.module}`;
        if (!queued.has(target)) {
          queued.add(target);
          queue.push([pointer.group, pointer.module]);
        }
      }
      if (variant.files !== undefined && !Array.isArray(variant.files)) {
        malformed.push(`${coordinate}: invalid files`);
        continue;
      }
      for (const file of variant.files ?? []) {
        const fileUrl = resolve(file?.url, url);
        if (
          !fileUrl ||
          !fileUrl.startsWith(url.slice(0, url.lastIndexOf("/") + 1)) ||
          typeof file.name !== "string" ||
          !file.name ||
          (file.size !== undefined &&
            (!Number.isSafeInteger(file.size) || file.size < 0))
        ) {
          malformed.push(`${coordinate}: invalid file record`);
          continue;
        }
        if (files.has(fileUrl)) {
          continue;
        }
        files.add(fileUrl);
        if (files.size > 4096) {
          malformed.push("metadata graph exceeds 4096 files");
          break;
        }
        const result = await request(fileUrl);
        if (result.status !== 200) {
          (result.status === 404 ? missing : transport).push(
            `${coordinate}/${file.name} (HTTP ${result.status})`,
          );
        }
      }
    }
  }
  for (const expected of step.variants ?? []) {
    if (!seen.has(expected)) {
      missing.push(`${expected} (not in module metadata)`);
    }
  }
  const variants = [...seen].filter((name) => name !== `${group}:${artifact}`);
  return {
    status: malformed.length
      ? "malformed-metadata"
      : transport.length
        ? "transport-unknown"
        : rootMissing
          ? "root-metadata-missing"
          : missing.length
            ? "partial"
            : "verified",
    ...(missing.length ? { missing } : {}),
    ...(malformed.length ? { malformed } : {}),
    ...(transport.length ? { transport } : {}),
    ...(malformed.length || transport.length || missing.length
      ? { reason: "exact Gradle publication graph is incomplete" }
      : {}),
    variants,
    files: files.size,
    evidence: {
      ...evidence,
      artifacts:
        malformed.length || transport.length || missing.length
          ? "incomplete"
          : "referenced-files-available",
      hashes: "unverified",
    },
  };
}

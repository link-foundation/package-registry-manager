import { readFile } from "node:fs/promises";
import path from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { ANSI } from "./auth-urls.mjs";

const execute = promisify(execFile);
const SLUG = /^[a-z\d_.-]+\/[a-z\d_.-]+$/i;

/** Normalize GitHub HTTPS, SSH, npm shorthand and provenance source URLs. */
export function githubSlug(value) {
  if (typeof value !== "string") {
    return null;
  }
  let text = value
    .trim()
    .replace(/^git\+/, "")
    .replace(/^github:/, "");
  text = text
    .replace(/^git@github\.com:/, "https://github.com/")
    .replace(/^ssh:\/\/git@github\.com\//, "https://github.com/");
  if (text.startsWith("https://github.com/")) {
    text = text
      .slice(19)
      .split(/[?#]/)[0]
      .split("/")
      .slice(0, 2)
      .join("/")
      .split("@")[0];
  }
  text = text.replace(/\.git\/?$/, "");
  return SLUG.test(text) ? text : null;
}

export function repositorySlug(repository) {
  return repository.github_owner && repository.github_repository
    ? `${repository.github_owner}/${repository.github_repository}`
    : null;
}

/** Read only declared repository metadata, preserving all PyPI source URLs. */
export function manifestUrls(contents, manifest) {
  contents = contents.replace(/^\uFEFF/, "");
  if (manifest.endsWith("package.json")) {
    let metadata;
    try {
      metadata = JSON.parse(contents).repository;
    } catch {
      return [];
    }
    return [typeof metadata === "string" ? metadata : metadata?.url].filter(
      Boolean,
    );
  }
  let table = "";
  let inlineUrls = null;
  const urls = [];
  for (const line of contents.split(/\r?\n/)) {
    const header = /^\s*\[([^\]]+)\]/.exec(line);
    if (header) {
      table = header[1].trim();
    }
    if (
      ["project", "tool.poetry"].includes(table) &&
      /^\s*urls\s*=\s*\{/.test(line)
    ) {
      inlineUrls = line.slice(line.indexOf("{") + 1);
    } else if (inlineUrls !== null) {
      inlineUrls += `\n${line}`;
    }
    if (inlineUrls?.includes("}")) {
      for (const field of inlineUrls
        .slice(0, inlineUrls.indexOf("}"))
        .matchAll(/(?:[\w-]+|"[^"]+"|'[^']+')\s*=\s*(["'])(.*?)\1/g)) {
        urls.push(field[2]);
      }
      inlineUrls = null;
    }
    const field =
      /^\s*([\w-]+|"[^"]+"|'[^']+')\s*=\s*(["'])(.*?)\2\s*(?:#.*)?$/.exec(line);
    if (!field) {
      continue;
    }
    const key = field[1].replaceAll(/["']/g, "").toLowerCase();
    if (
      (table === "package" || table === "tool.poetry") &&
      key === "repository"
    ) {
      urls.push(field[3]);
    }
    if (["project.urls", "tool.poetry.urls"].includes(table)) {
      urls.push(field[3]);
    }
  }
  return urls;
}

/** Findings name the evidence source; provenance is never treated as configuration. */
export function compareRepositories(packageInfo, repository, evidence = {}) {
  const slug = repositorySlug(repository);
  if (!slug) {
    return packageInfo;
  }
  const pairs = [
    ...(evidence.manifest_urls ?? []).map((url) => [
      "manifest",
      githubSlug(url),
    ]),
    ...(evidence.provenance_repositories ?? []).map((url) => [
      "provenance",
      githubSlug(url),
    ]),
    ...(evidence.configured_publishers ?? []).map((publisher) => [
      "trusted publisher",
      publisher.repository,
    ]),
  ];
  const findings = pairs
    .filter(([, old]) => old && old.toLowerCase() !== slug.toLowerCase())
    .map(([source, old]) => ({ source, repository: old }));
  packageInfo.repository_mismatches = [
    ...new Map(
      findings.map((finding) => [
        `${finding.source}:${finding.repository.toLowerCase()}`,
        finding,
      ]),
    ).values(),
  ];
  if (!packageInfo.repository_mismatches.length) {
    delete packageInfo.repository_mismatches;
  }
  const warnings = (packageInfo.warnings ?? []).filter(
    (warning) => !warning.startsWith("repository mismatch:"),
  );
  for (const finding of packageInfo.repository_mismatches ?? []) {
    warnings.push(
      `repository mismatch: ${finding.source} still names ${finding.repository}; repository being set up is ${slug}`,
    );
  }
  if (warnings.length) {
    packageInfo.warnings = warnings;
  } else {
    delete packageInfo.warnings;
  }
  return packageInfo;
}

/** Inspect local metadata even offline; retain URLs only when a fix is needed. */
export async function inspectManifestRepositories(inspection) {
  for (const packageInfo of inspection.packages) {
    if (!["npm", "crates-io", "pypi"].includes(packageInfo.registry)) {
      continue;
    }
    const contents = await readFile(
      path.join(inspection.repository.root, packageInfo.manifest),
      "utf8",
    ).catch(() => null);
    if (contents === null) {
      continue;
    }
    const urls = manifestUrls(contents, packageInfo.manifest);
    compareRepositories(packageInfo, inspection.repository, {
      ...packageInfo,
      manifest_urls: urls,
    });
    if (
      packageInfo.repository_mismatches?.some(
        (finding) => finding.source === "manifest",
      )
    ) {
      packageInfo.manifest_urls = urls;
    }
  }
  return inspection;
}

/** Run read-only identity queries without opening a browser or exposing auth output. */
export async function identityCommand(program, args, options = {}) {
  if (options.runIdentity) {
    return options.runIdentity(program, args);
  }
  try {
    const { stdout } = await execute(program, args, {
      cwd: options.repository,
      timeout: 10_000,
      maxBuffer: 2 * 1024 * 1024,
      env: options.env ?? process.env,
    });
    return stdout;
  } catch {
    if (options.verbose) {
      console.error(`${program} repository identity lookup unavailable`);
    }
    return undefined;
  }
}

/** Resolve redirects through GitHub's canonical full_name before comparison. */
export async function resolveRepository(inspection, options = {}) {
  const slug = repositorySlug(inspection.repository);
  if (!slug) {
    return;
  }
  const output = await identityCommand(
    "gh",
    ["api", `repos/${slug}`, "--jq", ".full_name"],
    options,
  );
  const canonical = githubSlug(output);
  if (canonical) {
    [
      inspection.repository.github_owner,
      inspection.repository.github_repository,
    ] = canonical.split("/");
  }
}

/** Convert npm JSON/human output and crates.io/PyPI settings to public identities. */
export function publisherIdentities(document) {
  if (typeof document === "string") {
    try {
      return publisherIdentities(JSON.parse(document));
    } catch {
      document = document.replaceAll(ANSI, "");
      const blocks = document
        .split(/(?=^\s*type:)/m)
        .filter((block) => /^\s*type:/m.test(block));
      if (blocks.length > 1) {
        return blocks.flatMap(publisherIdentities);
      }
      const values = {};
      for (const line of document.replaceAll(ANSI, "").split(/\r?\n/)) {
        const field =
          /^\s*(id|type|repository|file|environment):\s*(\S.*)$/.exec(line);
        if (field) {
          values[field[1]] = field[2].trim();
        }
      }
      return publisherIdentities(values);
    }
  }
  if (Array.isArray(document)) {
    return document.flatMap(publisherIdentities);
  }
  if (!document || typeof document !== "object") {
    return [];
  }
  if (document.type && document.type !== "github") {
    return [];
  }
  for (const key of ["github_configs", "publishers", "trustedPublishers"]) {
    if (Array.isArray(document[key])) {
      return publisherIdentities(document[key]);
    }
  }
  const repository = githubSlug(
    document.claims?.repository ??
      document.repository ??
      (document.repository_owner && document.repository_name
        ? `${document.repository_owner}/${document.repository_name}`
        : null),
  );
  if (!repository) {
    return [];
  }
  return [
    {
      ...(document.id !== undefined ? { id: String(document.id) } : {}),
      repository,
      workflow:
        document.claims?.workflow_ref?.file ??
        document.workflow_filename ??
        document.workflow ??
        document.file ??
        null,
      environment: document.claims?.environment ?? document.environment ?? null,
    },
  ];
}

export function publisherMatches(publisher, expected) {
  return (
    publisher.repository.toLowerCase() ===
      `${expected.organization}/${expected.repository}`.toLowerCase() &&
    publisher.workflow === expected.workflow &&
    (publisher.environment ?? null) === (expected.environment ?? null)
  );
}

/** Extract source identity from npm SLSA envelopes and PyPI publisher bundles. */
export function provenanceRepositories(document) {
  const urls = [];
  for (const bundle of document?.attestation_bundles ?? []) {
    if (bundle.publisher?.kind === "GitHub") {
      urls.push(bundle.publisher.repository);
    }
  }
  for (const attestation of document?.attestations ?? []) {
    const payload = attestation.bundle?.dsseEnvelope?.payload;
    if (typeof payload !== "string" || payload.length > 2 * 1024 * 1024) {
      continue;
    }
    try {
      const statement = JSON.parse(
        Buffer.from(payload, "base64").toString("utf8"),
      );
      const predicate = statement.predicate;
      urls.push(
        predicate?.invocation?.configSource?.uri,
        predicate?.buildDefinition?.externalParameters?.workflow?.repository,
      );
    } catch {
      /* Malformed provenance leaves source identity unknown. */
    }
  }
  return [...new Set(urls.map(githubSlug).filter(Boolean))];
}

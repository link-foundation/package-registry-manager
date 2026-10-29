import { probePackage } from "./registry-state.mjs";
import { repositorySlug } from "./repository-identity.mjs";

/** The successful npm CLI line binds each name to the actual released version. */
export function publishedNpmVersions(source, names) {
  const result = new Map();
  for (const match of source.matchAll(
    /(?:^|\s)\+\s+((?:@[a-z0-9._-]+\/)?[a-z0-9][a-z0-9._-]*)@([0-9][0-9A-Za-z.+-]*)(?=\s|$)/g,
  )) {
    if (names.includes(match[1])) {
      result.set(match[1], match[2]);
    }
  }
  return result;
}

/** Verify every generated name against its own exact public version after CI. */
export async function verifyNpmRelease(session, runId) {
  const item = session.plan.package;
  const names = (item.generated_alias ?? item.generated_artifact)?.names ?? [
    item.name,
  ];
  const slug = repositorySlug(session.plan.repository);
  const capture = (args) =>
    session.capture({
      command: {
        program: "gh",
        args: ["run", "view", String(runId), "--repo", slug, ...args],
      },
      cwd: ".",
    });
  const result = await capture(["--json", "jobs"]);
  if (result.code !== 0) {
    throw new Error(
      "release jobs are unavailable; exact alias versions remain unverified",
    );
  }
  const versions = new Map();
  for (const job of JSON.parse(result.stdout).jobs ?? []) {
    const log = await capture(["--job", String(job.databaseId), "--log"]);
    if (log.code !== 0) {
      continue;
    }
    for (const entry of publishedNpmVersions(String(log.stdout), names)) {
      versions.set(...entry);
    }
  }
  const evidence = [];
  for (const name of names) {
    const version = versions.get(name);
    if (!version) {
      throw new Error(
        `successful release log does not establish the actual version for ${name}`,
      );
    }
    const state = await (session.options.probePackage ?? probePackage)(
      { ...item, name, version },
      session.options,
    );
    if (state.manifest_version_exists !== true) {
      throw new Error(
        `exact released version ${name}@${version} remains unverified`,
      );
    }
    evidence.push({ name, version, url: state.version_evidence_url });
  }
  return evidence;
}

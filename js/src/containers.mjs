import { grantsPackagesWrite, publishingWorkflow } from "./workflows.mjs";

export const CONTAINER_FILES = new Set(["Dockerfile", "Containerfile"]);

const HOSTS = new Map([
  ["docker-hub", "docker\\.io"],
  ["ghcr", "ghcr\\.io"],
]);

/**
 * Describes Docker Hub and GHCR images built from a Dockerfile and pushed by a
 * GitHub Actions workflow.
 */
export function containerPackages(dockerfiles, workflows, coordinates) {
  if (dockerfiles.length === 0) {
    return [];
  }
  const manifest = [...dockerfiles].sort(
    (left, right) =>
      left.split("/").length - right.split("/").length ||
      left.localeCompare(right),
  )[0];
  const packages = [];
  for (const [registry, host] of HOSTS) {
    const workflow = publishingWorkflow(workflows, registry);
    if (!workflow) {
      continue;
    }
    const name =
      literalImage(workflow.contents, host) ?? defaultImage(coordinates);
    const item = {
      registry,
      name: name ?? "unknown-image",
      version: null,
      manifest,
      publishable: Boolean(name),
      workflow: workflow.name,
    };
    if (!name) {
      item.problems = [
        "the image name could not be derived from the workflow or a GitHub remote",
      ];
    }
    if (registry === "ghcr" && !grantsPackagesWrite(workflow.contents)) {
      item.warnings = [
        `${workflow.name} does not grant packages: write, so GITHUB_TOKEN cannot push to ghcr.io`,
      ];
    }
    packages.push(item);
  }
  return packages;
}

function literalImage(contents, host) {
  const match = new RegExp(
    `\\b${host}/([a-z0-9][a-z0-9._-]*/[a-z0-9][a-z0-9._/-]*[a-z0-9])`,
    "i",
  ).exec(contents);
  return match ? match[1].toLowerCase() : null;
}

function defaultImage({ github_owner, github_repository }) {
  return github_owner && github_repository
    ? `${github_owner}/${github_repository}`.toLowerCase()
    : null;
}

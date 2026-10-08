import { mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { exec } from "command-stream";
import {
  githubSlug,
  manifestUrls,
  repositorySlug,
} from "./repository-identity.mjs";

function runner(options) {
  return (
    options.run ??
    (async (program, args, cwd = options.repository) => {
      const result = await exec(program, args, {
        cwd,
        env: process.env,
        capture: true,
        mirror: false,
        stdin: "ignore",
      });
      if (result.code !== 0) {
        throw new Error(
          `${program} failed while preparing the manifest repository proposal`,
        );
      }
      return String(result.stdout ?? "").trim();
    })
  );
}

function safeManifest(manifest) {
  if (
    !manifest ||
    manifest
      .split("/")
      .some((part) => !part || part === "." || part === "..") ||
    path.isAbsolute(manifest) ||
    manifest.includes("\\")
  ) {
    throw new Error("unsafe manifest path for repository repair");
  }
}

/** Change only stale repository values; preserve npm's repository.directory. */
export function manifestRepositoryProposal(contents, manifest, slug) {
  if (!githubSlug(slug)) {
    throw new Error("invalid canonical repository");
  }
  const urls = manifestUrls(contents, manifest);
  const stale = urls.filter(
    (url) =>
      githubSlug(url) && githubSlug(url).toLowerCase() !== slug.toLowerCase(),
  );
  if (!stale.length) {
    return contents;
  }
  const url = `https://github.com/${slug}`;
  if (manifest.endsWith("package.json")) {
    const document = JSON.parse(contents.replace(/^\uFEFF/, ""));
    if (typeof document.repository === "string") {
      document.repository = url;
    } else {
      document.repository.url = `git+${url}.git`;
    }
    return `${JSON.stringify(document, null, 2)}\n`;
  }
  let proposal = contents;
  for (const old of stale) {
    proposal = proposal.replaceAll(old, old.replace(githubSlug(old), slug));
  }
  return proposal;
}

/** Check remote metadata before retrying; the original run may use an old SHA. */
export async function verifyRemoteManifest(plan, options, reference) {
  safeManifest(plan.package.manifest);
  const run = runner(options);
  const slug = repositorySlug(plan.repository);
  const ref =
    reference ??
    (await run("gh", ["api", `repos/${slug}`, "--jq", ".default_branch"]));
  if (!ref || ref.startsWith("-") || /[\s:]/.test(ref)) {
    throw new Error("invalid remote manifest reference");
  }
  await run("git", ["fetch", "origin", ref]);
  const contents = await run("git", [
    "show",
    `FETCH_HEAD:${plan.package.manifest}`,
  ]);
  return (
    manifestRepositoryProposal(contents, plan.package.manifest, slug) ===
    contents
  );
}

/** Offer a reviewed branch and draft PR from the current remote default branch. */
export async function offerManifestRepository(plan, options) {
  const run = runner(options);
  const slug = repositorySlug(plan.repository);
  const manifest = plan.package.manifest;
  safeManifest(manifest);
  const remote = githubSlug(await run("git", ["remote", "get-url", "origin"]));
  if (!remote) {
    throw new Error(
      "origin must be a GitHub repository for the manifest proposal",
    );
  }
  const canonical = await run("gh", [
    "api",
    `repos/${remote}`,
    "--jq",
    ".full_name",
  ]);
  if (canonical.toLowerCase() !== slug.toLowerCase()) {
    throw new Error("origin must resolve to the repository being repaired");
  }
  const base = await run("gh", [
    "api",
    `repos/${slug}`,
    "--jq",
    ".default_branch",
  ]);
  if (!base || base.startsWith("-") || /[\s:]/.test(base)) {
    throw new Error("invalid default branch");
  }
  await run("git", ["fetch", "origin", base]);
  const contents = await run("git", ["show", `FETCH_HEAD:${manifest}`]);
  const proposal = manifestRepositoryProposal(contents, manifest, slug);
  if (proposal === contents) {
    return undefined;
  }
  console.log(`Proposed ${manifest}:\n${proposal}`);
  if (
    !options.yes &&
    !/^y(?:es)?$/i.test(
      await options.prompt(
        "Create a branch and draft pull request with this repository URL fix? [y/N] ",
      ),
    )
  ) {
    return { status: "blocked" };
  }
  const branch = `prm/repository-url-${Date.now()}`;
  const temporary = await mkdtemp(path.join(os.tmpdir(), "prm-manifest-"));
  const checkout = path.join(temporary, "checkout");
  let created = false;
  try {
    await run("git", ["worktree", "add", "-b", branch, checkout, "FETCH_HEAD"]);
    created = true;
    const target = path.join(checkout, manifest);
    const root = await realpath(checkout);
    const physical = await realpath(target);
    if (
      !physical.startsWith(`${root}${path.sep}`) ||
      physical !== path.join(root, manifest)
    ) {
      throw new Error(
        "manifest must stay inside the proposal worktree without symlinks",
      );
    }
    const current = await readFile(target, "utf8");
    const updated = manifestRepositoryProposal(current, manifest, slug);
    await writeFile(target, updated);
    await run("git", ["add", "--", manifest], checkout);
    await run(
      "git",
      ["commit", "-m", "Fix manifest repository URL after repository transfer"],
      checkout,
    );
    await run("git", ["push", "origin", `HEAD:refs/heads/${branch}`], checkout);
    const url = await run(
      "gh",
      [
        "pr",
        "create",
        "--repo",
        slug,
        "--base",
        base,
        "--head",
        branch,
        "--draft",
        "--title",
        "Fix manifest repository URL after repository transfer",
        "--body",
        `Update ${manifest} to name ${slug}. Review and merge this change, then re-run package-registry-manager setup to verify the repository metadata and retry the release.`,
      ],
      checkout,
    );
    console.log(
      `Review and merge ${url}, then re-run setup. Release retries are paused.`,
    );
    return { status: "manifest-pr", url };
  } finally {
    if (created) {
      await run("git", ["worktree", "remove", "--force", checkout]);
    }
    await rm(temporary, { recursive: true, force: true });
  }
}

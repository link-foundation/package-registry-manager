import { exec } from "./exec.mjs";

async function github(args) {
  try {
    const result = await exec("gh", args, {
      capture: true,
      mirror: false,
      stdin: "ignore",
    });
    if (result.code !== 0) {
      return undefined;
    }
    const output = String(result.stdout ?? "").trim();
    return args[0] === "api" ? JSON.parse(output) : output;
  } catch {
    return undefined;
  }
}

/** Prefer the checkout's organization or a sole membership, then the signed-in user. */
export async function defaultScope(options = {}) {
  if (options.repository !== undefined || options.ref || options.offline) {
    return {};
  }
  const run = options.run ?? github;
  const slug = await run([
    "repo",
    "view",
    "--json",
    "nameWithOwner",
    "--jq",
    ".nameWithOwner",
  ]);
  if (typeof slug === "string" && /^[\w.-]+\/[\w.-]+$/.test(slug)) {
    const repository = await run(["api", `repos/${slug}`]);
    if (repository?.owner?.type === "Organization") {
      return { org: repository.owner.login };
    }
  }
  const orgs = await run([
    "api",
    "user/orgs",
    "--paginate",
    "--slurp",
    "--jq",
    "flatten",
  ]);
  if (Array.isArray(orgs) && orgs.length === 1) {
    return { org: orgs[0].login };
  }
  const user = await run(["api", "user"]);
  return user?.login ? { user: user.login } : {};
}

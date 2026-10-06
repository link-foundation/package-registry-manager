import {
  mkdir,
  mkdtemp,
  writeFile,
  rm,
  realpath,
  lstat,
} from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { exec } from "command-stream";
import { packageDirectory } from "./plan.mjs";
import { parseWorkflow } from "./publishers.mjs";
import { readWorkflows } from "./workflows.mjs";

const quote = (text) => `'${text.replaceAll("'", "''")}'`;
const RELEASE_IF =
  "github.event_name == 'release' || ((github.event_name == 'push' || github.event_name == 'workflow_dispatch') && github.ref == format('refs/heads/{0}', github.event.repository.default_branch))";

function job(packageInfo, index, environment) {
  if (
    !/^(?:[\w@.-]+\/)*[\w@.-]+$/.test(packageInfo.manifest) ||
    packageInfo.manifest.split("/").some((part) => ["..", "."].includes(part))
  ) {
    throw new Error("unsafe manifest path for publishing job");
  }
  const directory = packageDirectory(packageInfo.manifest);
  const lines = [
    `  prm-publish-${packageInfo.registry}-${index + 1}:`,
    `    if: ${RELEASE_IF}`,
    "    runs-on: ubuntu-latest",
    "    timeout-minutes: 15",
    "    permissions:",
    "      contents: read",
    "      id-token: write",
    ...(environment ? [`    environment: ${quote(environment)}`] : []),
    "    defaults:",
    "      run:",
    `        working-directory: ${quote(directory)}`,
    "    steps:",
    "      - uses: actions/checkout@v6",
    "        with:",
    "          persist-credentials: false",
  ];
  switch (packageInfo.registry) {
    case "npm":
      lines.push(
        "      - uses: actions/setup-node@v6",
        "        with:",
        "          node-version: '22'",
        "      - run: npm install --global npm@^11",
        "      - run: npm install",
        "      - run: npm run build --if-present",
        "      - run: npm publish --provenance --access public",
      );
      break;
    case "crates-io":
      lines.push(
        "      - uses: dtolnay/rust-toolchain@stable",
        "      - uses: rust-lang/crates-io-auth-action@v1",
        "        id: auth",
        "      - run: cargo publish",
        "        env:",
        "          CARGO_REGISTRY_TOKEN: ${{ steps.auth.outputs.token }}",
      );
      break;
    case "pypi":
      lines.push("      - uses: actions/setup-python@v6", "        with:");
      lines.push(
        packageInfo.manifest.endsWith("pyproject.toml") &&
          packageInfo.requires_python
          ? `          python-version-file: ${quote(packageInfo.manifest)}`
          : "          python-version: '3.x'",
      );
      lines.push(
        "      - run: python -m pip install build",
        "      - run: python -m build",
        "      - uses: pypa/gh-action-pypi-publish@release/v1",
        "        with:",
        `          packages-dir: ${quote(`${directory === "." ? "" : `${directory}/`}dist/`)}`,
      );
      break;
    default:
      throw new Error(`no publishing job template for ${packageInfo.registry}`);
  }
  return `${lines.join("\n")}\n`;
}

/** Generates reviewable workflow contents without modifying the checkout. */
export function workflowProposal(
  inspection,
  packages,
  workflows,
  override,
  chosenEnvironment,
) {
  const known = [
    ...new Set(
      inspection.packages
        .filter(
          (item) =>
            item.publishable !== false &&
            ["npm", "crates-io", "pypi"].includes(item.registry),
        )
        .map((item) => item.workflow)
        .filter(Boolean),
    ),
  ];
  if (!override && known.length > 1) {
    throw new Error(
      "publishing uses several files; pass --add-publish-job --workflow <file> to choose the shared workflow",
    );
  }
  const workflow =
    override ??
    known[0] ??
    inspection.repository.release_workflow ??
    "release.yml";
  if (!/^[\w.-]+\.ya?ml$/.test(workflow)) {
    throw new Error("invalid publishing workflow name");
  }
  let contents =
    workflows.find((item) => item.name === workflow)?.contents ??
    "name: Package release\non:\n  workflow_dispatch:\n  release:\n    types: [published]\npermissions:\n  contents: read\njobs:\n";
  const parsed = parseWorkflow(contents);
  const environments = [
    ...new Set(
      inspection.packages
        .filter((item) => item.workflow === workflow)
        .map((item) => item.environment ?? null),
    ),
  ];
  if (chosenEnvironment === undefined && environments.length > 1) {
    throw new Error(
      "publishing jobs use different environments; choose a shared environment with --environment",
    );
  }
  let addition = packages
    .map((item, index) =>
      job(item, index, chosenEnvironment ?? environments[0]),
    )
    .join("\n");
  for (const item of parseWorkflow(`jobs:\n${addition}`).jobs) {
    if (parsed.jobs.some((existing) => existing.name === item.name)) {
      throw new Error(
        `publishing job ${item.name} already exists; inspect the workflow before adding it again`,
      );
    }
  }
  const lines = contents.split(/\r?\n/);
  const start = lines.findIndex((line) => /^jobs\s*:\s*$/.test(line));
  if (start === -1) {
    throw new Error(
      "workflow has no editable jobs block; add a publishing job manually",
    );
  }
  let end = lines.findIndex((line, index) => index > start && /^\S/.test(line));
  end = end === -1 ? lines.length : end;
  // Match the existing job indentation without reformatting unrelated YAML.
  const indent = parsed.jobs.length
    ? (lines
        .slice(start + 1, end)
        .find((line) => /^\s+\S/.test(line) && !/^\s*#/.test(line))
        ?.search(/\S/) ?? 2)
    : 2;
  if (indent !== 2) {
    if (indent < 2) {
      throw new Error("unsupported jobs indentation");
    }
    addition = addition
      .split("\n")
      .map((line) => (line ? `${" ".repeat(indent - 2)}${line}` : line))
      .join("\n");
  }
  contents = `${lines.slice(0, end).join("\n").trimEnd()}\n\n${addition}${lines.slice(end).join("\n")}`;
  return { workflow, contents };
}

/** Creates a branch and draft PR; never writes or pushes the default branch. */
export async function offerWorkflow(plans, options) {
  const inspection = options.inspection ?? {
    repository: plans[0].repository,
    packages: plans.map((plan) => plan.package),
  };
  const proposal = workflowProposal(
    inspection,
    plans.map((plan) => plan.package),
    await readWorkflows(options.repository),
    options.workflow,
    options.publisherEnvironment,
  );
  console.log(
    `Proposed .github/workflows/${proposal.workflow}:\n${proposal.contents}`,
  );
  if (
    !options.yes &&
    !/^y(?:es)?$/i.test(
      (
        await options.prompt(
          "Create a branch and draft pull request with these publishing jobs? [y/N] ",
        )
      ).trim(),
    )
  ) {
    return { status: "blocked" };
  }
  const run =
    options.run ??
    (async (program, args, cwd = options.repository) => {
      if (options.verbose) {
        console.error(`+ ${program} ${args.join(" ")}`);
      }
      const result = await exec(program, args, {
        cwd,
        capture: true,
        mirror: false,
        stdin: "ignore",
      });
      if (result.code !== 0) {
        throw new Error(
          `${program} failed: ${String(result.stderr ?? result.stdout ?? "")}`,
        );
      }
      return String(result.stdout ?? "").trim();
    });
  const remote = await run("git", ["remote", "get-url", "origin"]);
  const slug = `${inspection.repository.github_owner}/${inspection.repository.github_repository}`;
  if (
    !inspection.repository.github_owner ||
    !inspection.repository.github_repository ||
    !["https://github.com/", "git@github.com:", "ssh://git@github.com/"].some(
      (prefix) =>
        remote === `${prefix}${slug}` || remote === `${prefix}${slug}.git`,
    )
  ) {
    throw new Error(
      "origin must match the inspected GitHub repository before proposing a workflow",
    );
  }
  const base = await run("gh", [
    "repo",
    "view",
    slug,
    "--json",
    "defaultBranchRef",
    "--jq",
    ".defaultBranchRef.name",
  ]);
  await run("git", ["fetch", "origin", base]);
  const branch = `prm/publish-jobs-${Date.now()}`;
  const temporary = await mkdtemp(path.join(os.tmpdir(), "prm-workflow-"));
  const checkout = path.join(temporary, "checkout");
  let created = false;
  try {
    await run("git", [
      "worktree",
      "add",
      "-b",
      branch,
      checkout,
      `origin/${base}`,
    ]);
    created = true;
    // Recompute from the default branch to preserve edits made since inspection.
    const current = workflowProposal(
      inspection,
      plans.map((plan) => plan.package),
      await readWorkflows(checkout),
      proposal.workflow,
      options.publisherEnvironment,
    );
    if (current.contents !== proposal.contents) {
      console.log(`Updated proposal from ${base}:\n${current.contents}`);
      if (
        !options.yes &&
        !/^y(?:es)?$/i.test(
          (await options.prompt("Use this updated proposal? [y/N] ")).trim(),
        )
      ) {
        return { status: "blocked" };
      }
    }
    const file = `.github/workflows/${current.workflow}`;
    await mkdir(path.dirname(path.join(checkout, file)), { recursive: true });
    const target = path.join(checkout, file);
    const physicalParent = await realpath(path.dirname(target));
    if (
      !physicalParent.startsWith(`${await realpath(checkout)}${path.sep}`) ||
      (await lstat(target).catch(() => null))?.isSymbolicLink()
    ) {
      throw new Error(
        "workflow must stay inside the proposal worktree without symlinks",
      );
    }
    await writeFile(target, current.contents);
    await run("git", ["add", "--", file], checkout);
    await run(
      "git",
      ["commit", "-m", "Add missing trusted publishing jobs"],
      checkout,
    );
    await run("git", ["push", "origin", `HEAD:refs/heads/${branch}`], checkout);
    const body = `Add missing publishing jobs for ${plans.map((plan) => `${plan.registry}: ${plan.package.name}`).join(", ")} in ${current.workflow}. Review the build, versioning, triggers and job dependencies before merging. Then re-run setup to attach the trusted publishers to this file.`;
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
        "Add missing trusted publishing jobs",
        "--body",
        body,
      ],
      checkout,
    );
    console.log(
      `Review ${url}, merge it, then re-run setup. Trusted publishers have not been attached.`,
    );
    return { status: "workflow-pr", url };
  } finally {
    if (created) {
      await run("git", ["worktree", "remove", "--force", checkout]);
    }
    await rm(temporary, { recursive: true, force: true });
  }
}

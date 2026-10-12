import { spawnSync } from "node:child_process";
import { readFile, rm, stat } from "node:fs/promises";
import path from "node:path";
import { withAccountScan, renderScan } from "./organization-scan.mjs";
import { buildPlans } from "./plan.mjs";
import { probeEnvironment } from "./prerequisites.mjs";
import { executePlans } from "./setup-batch.mjs";
import { secretName } from "./ci-credential-cycle.mjs";
import { TOKEN_PROVIDERS } from "./credential-browser.mjs";
import { listCratesTokens, tokenExpiryFindings } from "./crates-tokens.mjs";
import { SetupSession } from "./setup.mjs";
import { runCratesApiStep } from "./crates-api.mjs";
import { readWorkflows } from "./workflows.mjs";

async function prepareRepository(item) {
  const root = item.inspection.repository.root;
  await rm(`${root}/.git`, { recursive: true, force: true });
  const commands = [
    ["init", "--quiet"],
    ["remote", "add", "origin", `https://github.com/${item.repository}.git`],
    ["fetch", "--quiet", "--depth=1", "origin", item.default_branch],
    ["checkout", "--quiet", "--force", "--detach", "FETCH_HEAD"],
  ];
  for (const args of commands) {
    const result = spawnSync("git", args, { cwd: root, stdio: "pipe" });
    if (result.status !== 0) {
      throw new Error(`cannot prepare ${item.repository} for setup`);
    }
  }
}

/** Plan from API snapshots; fetch worktrees only for repositories being changed. */
export async function accountCommand(command, options, outputPlans) {
  if (options.tokenExpiryDays !== undefined && !options.cratesTokens) {
    if (options.offline) {
      throw new Error(
        "--token-expiry-days requires an online browser inventory",
      );
    }
    const session = new SetupSession(
      { registry: "crates-io", package: {}, steps: [] },
      {
        ...options,
        repository: options.repository ?? ".",
        quietBrowser: true,
        log: console.error,
      },
    );
    try {
      await runCratesApiStep(session, { id: "crates-sign-in" });
      options.cratesTokens = await listCratesTokens(
        await session.automatedPage(options.browserOptions, true),
      );
    } finally {
      await session.automation?.close();
    }
  }
  if (command === "inspect" && options.format !== "json") {
    process.stdout.write("Repository\tRegistry\tPackage\tFinding\n");
    if (options.cratesTokens) {
      process.stdout.write(
        renderScan({
          repositories: [],
          findings: tokenExpiryFindings(
            options.cratesTokens,
            options.org ?? options.user,
            options.tokenExpiryDays,
          ),
        })
          .split("\n")
          .slice(1)
          .join("\n"),
      );
    }
  }
  const scanOptions = {
    ...options,
    onRepository: async (item, findings, index, total) => {
      console.error(
        `[${index}/${total}] ${item.repository}${item.error ? `: ${item.error}` : ""}`,
      );
      if (command === "inspect" && options.format !== "json") {
        process.stdout.write(
          renderScan({ repositories: [item], findings })
            .split("\n")
            .slice(1)
            .join("\n"),
        );
      }
      await options.onRepository?.(item, findings, index, total);
    },
  };
  return withAccountScan(scanOptions, async (scan, github) => {
    if (command === "inspect") {
      const errors = scan.repositories.filter((item) => item.error);
      for (const item of scan.repositories) {
        if (item.inspection) {
          item.inspection.repository.root = `https://github.com/${item.repository}`;
        }
      }
      process.stdout.write(
        options.format === "json"
          ? `${JSON.stringify(scan, null, 2)}\n`
          : `Scanned ${scan.repositories.length} repositories; ${errors.length} with errors.\n${errors.map((item) => `${item.repository}: ${item.error}\n`).join("")}`,
      );
      if (errors.length) {
        process.exitCode = 1;
      }
      return scan;
    }
    let items = scan.repositories.filter((item) => item.inspection);
    const errors = scan.repositories.filter((item) => item.error);
    for (const item of items) {
      const workflow = options.planOptions?.workflow;
      if (workflow !== undefined) {
        if (!/^[\w.-]+\.ya?ml$/.test(workflow)) {
          throw new Error(
            "--workflow must be a workflow file name in .github/workflows",
          );
        }
        if (
          !options.planOptions.addPublishJob &&
          !(await stat(
            path.join(
              item.inspection.repository.root,
              ".github/workflows",
              workflow,
            ),
          ).then(
            (metadata) => metadata.isFile(),
            () => false,
          ))
        ) {
          const message = `--workflow: ${item.repository} has no .github/workflows/${workflow}`;
          (item.errors ??= []).push({ stage: "plan", workflow, message });
          item.error = [item.error, message].filter(Boolean).join("; ");
          if (!errors.includes(item)) {
            errors.push(item);
          }
          items = items.filter((candidate) => candidate !== item);
        }
      }
    }
    if (errors.length) {
      console.error(renderScan({ repositories: errors, findings: [] }));
    }
    if (options.migrate && errors.length) {
      throw new Error("migration requires a complete account scan");
    }
    const packages = items.flatMap((item) => item.inspection.packages);
    const environment = await (options.probeEnvironment ?? probeEnvironment)({
      offline: options.offline,
      verbose: options.verbose,
      npm: packages.some((item) => item.registry === "npm"),
      python: packages.some((item) => item.registry === "pypi"),
    });
    const rotationConsumers = new Set();
    if (options.rotateToken) {
      const reference = new RegExp(`\\bsecrets\\.${options.rotateToken}\\b`);
      for (const item of items) {
        if (
          (await readWorkflows(item.inspection.repository.root)).some(
            (workflow) => reference.test(workflow.contents),
          )
        ) {
          rotationConsumers.add(item.repository);
        }
      }
    }
    let plans = items
      .flatMap((item) => {
        const relevant = scan.findings.filter(
          (finding) => finding.repository === item.repository,
        );
        const inspection = {
          ...item.inspection,
          packages: item.inspection.packages.filter(
            (pkg) =>
              (!options.package || pkg.name === options.package) &&
              (!options.rotateToken ||
                (pkg.registry === "crates-io" &&
                  rotationConsumers.has(item.repository))) &&
              (command === "plan" ||
                options.migrate ||
                options.rotateToken ||
                relevant.some(
                  (finding) => !finding.package || finding.package === pkg.name,
                )),
          ),
        };
        return buildPlans(inspection, options.registries ?? [], {
          ...options.planOptions,
          rotateToken: options.rotateToken,
          environment,
        }).filter((plan) => plan.package.publishable);
      })
      .sort(
        (a, b) => Number(b.registry === "npm") - Number(a.registry === "npm"),
      );
    const display = structuredClone(plans);
    for (const plan of display) {
      plan.repository.root = `https://github.com/${plan.repository.github_owner}/${plan.repository.github_repository}`;
    }
    if (options.format === "json") {
      const report = structuredClone(scan);
      for (const item of report.repositories) {
        if (item.inspection) {
          item.inspection.repository.root = `https://github.com/${item.repository}`;
        }
      }
      process.stdout.write(
        `${JSON.stringify({ ...report, plans: display }, null, 2)}\n`,
      );
    } else {
      outputPlans(display, options.format);
    }
    if (options.package && !plans.length) {
      throw new Error(`package '${options.package}' was not found`);
    }
    if (command === "plan" || plans.length === 0) {
      if (errors.length) {
        process.exitCode = 1;
      }
      if (!plans.length && options.format !== "json") {
        console.log("No package findings require setup.");
      }
      return [];
    }
    if (options.execute) {
      for (const item of items.filter((item) =>
        plans.some(
          (plan) => plan.repository.root === item.inspection.repository.root,
        ),
      )) {
        try {
          await (options.prepareRepository ?? prepareRepository)(item);
        } catch (error) {
          item.error = error.message;
          console.error(`${item.repository}: ${item.error}`);
          plans = plans.filter(
            (plan) => plan.repository.root !== item.inspection.repository.root,
          );
        }
      }
      if (!plans.length) {
        throw new Error(
          "some repositories could not be prepared; see scan errors",
        );
      }
      if (options.migrate && scan.repositories.some((item) => item.error)) {
        throw new Error("migration requires a complete account scan");
      }
    }
    const secretRepositories = {};
    for (const plan of plans) {
      const provider =
        TOKEN_PROVIDERS[plan.registry] ??
        (options.rotateToken && plan.registry === "crates-io"
          ? { secret: options.rotateToken }
          : null);
      if (provider) {
        const config = JSON.parse(
          await readFile(
            path.join(plan.repository.root, ".package-registry-manager.json"),
            "utf8",
          ).catch((error) => {
            if (error.code === "ENOENT") {
              return "{}";
            }
            throw error;
          }),
        );
        if (config.tokens?.[plan.registry]?.level === "repo") {
          continue;
        }
        const slug = `${plan.repository.github_owner}/${plan.repository.github_repository}`;
        const name = secretName(
          options.rotateToken ??
            options.secretName ??
            config.tokens?.[plan.registry]?.secret ??
            plan.package.token_secrets?.[0] ??
            provider.secret,
          plan.registry,
          slug,
        );
        const targets = (secretRepositories[`${plan.registry}:${name}`] ??= []);
        if (!targets.includes(slug)) {
          targets.push(slug);
        }
      }
    }
    const outcomes = await (options.executePlans ?? executePlans)(
      plans,
      {
        ...options,
        github,
        accountScan: true,
        scanErrors: scan.repositories.some((item) => item.error),
        browser: options.openWith ? options.browser : "automated",
        quietBrowser: true,
        secretRepositories,
        repository: items[0]?.inspection.repository.root,
        workflow: options.planOptions?.workflow,
        publisherEnvironment: options.planOptions?.publisherEnvironment,
      },
      true,
    );
    if (scan.repositories.some((item) => item.error)) {
      throw new Error(
        "some repositories could not be scanned; see scan errors",
      );
    }
    return outcomes;
  });
}

#!/usr/bin/env node

import { readFileSync } from "node:fs";
import { stat } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { parseArgs } from "node:util";

import { launchChannels } from "./browser-catalogue.mjs";
import { parseBrowserOptions } from "./browser-options.mjs";
import { isDirectExecution } from "./direct-execution.mjs";
import { inspectReference } from "./bootstrap-reference.mjs";
import { inspectRepository } from "./discovery.mjs";
import { parseRegistry } from "./model.mjs";
import { buildPlans } from "./plan.mjs";
import { probeEnvironment, renderPrerequisites } from "./prerequisites.mjs";
import { probeRegistryState } from "./registry-state.mjs";
import {
  BROWSER_MODES,
  defaultBrowserProfile,
  executePlans,
} from "./setup.mjs";
import { importSources } from "./sign-in-import.mjs";

export { isDirectExecution };

const HELP = `Usage: package-registry-manager-js [global options] <command>

Commands:
  inspect                         Discover supported package manifests
  plan [--registry <registry>]    Print ordered setup plans
  setup --registry <registry>     Bootstrap or attach trusted publishing
  setup --all                     Set up every publishable package in one session

Global options:
  -h, --help                      Print this help
  -V, --version                   Print the version
  --repository <path>             Repository to inspect (default: .)
  --format <text|json>            Output format (default: text)
  --offline                       Do not look up packages on registries
  --verbose                       Print command details and output, and
                                  list the manifests inspection skipped

Plan and setup options:
  --verify-release                Also watch the release workflow and confirm
                                  that the next version has provenance
  --workflow <file>               Trusted-publisher workflow file in
                                  .github/workflows, when detection finds
                                  several or the wrong one
  --add-publish-job                Propose missing publishing jobs in a draft PR;
                                  --workflow chooses its target file
  --environment <name>            GitHub environment of the trusted publisher
  --ref <branch|pr>               Bootstrap a pushed branch or PR head before merge
  --package <name>                Plan only this package, or select it for
                                  setup when a registry has several

Setup options:
  --dry-run                       Print the flow without running it (default)
  --execute                       Run the flow and open a visible browser
  --yes                           Confirm publishing, secret changes, and
                                  form submission in advance
  --no-browser                    Print the setup URL instead
  --open-with <app>               Open sign-in and approval pages in this
                                  application instead of the default browser,
                                  such as "Google Chrome" (macOS) or firefox
  --manual                        crates.io: create, use, and revoke the
                                  first-publish token by hand instead of
                                  through the crates.io API
  --keep-session                  Stay signed in after setup: npm keeps its
                                  session token in npm's user configuration
                                  until npm logout; crates.io keeps only the
                                  automated profile's browser session
  --browser <default|automated>   Open sign-in and approval pages in your
                                  default browser, or in the automated
                                  profile too (default: default)
  --browser-channel <channel>     Installed browser channel (default: chrome):
                                  ${launchChannels().join(", ")}
  --browser-executable <path>     Installed browser executable to launch
                                  instead of the channel's
  --browser-profile <path>        Dedicated automation profile, used to fill
                                  forms (default: per-user state directory)
  --browser-import <browser>[:<profile>]|default|auto
                                  Copy data from your installed browser profile:
                                  ${importSources().join(", ")}
                                  into the automated profile; default takes the
                                  system default browser, auto the first
                                  browser signed in to the registry
  --browser-import-scope <full|domains>
                                  Import the whole profile, or only the
                                  registry's sign-in cookies (default: full
                                  for a named browser, domains otherwise)
  --browser-attach <mode>         Fill forms in your own browser instead:
                                  snapshot[:<profile>] launches a temporary
                                  copy of your profile, extension drives your
                                  running browser through the Browser
                                  Commander extension
  --browser-pref <key=value>      Browser preference for the automated
                                  profile, such as intl.accept_languages=en;
                                  may be repeated
  --browser-restriction <name>    Launch restriction or preset, such as
                                  no-extensions; may be repeated
`;

export async function main(args = process.argv.slice(2)) {
  const { values, positionals } = parseArgs({
    args,
    allowPositionals: true,
    strict: true,
    options: {
      ref: { type: "string" },
      repository: { type: "string", default: "." },
      format: { type: "string", default: "text" },
      verbose: { type: "boolean", default: false },
      offline: { type: "boolean", default: false },
      "verify-release": { type: "boolean", default: false },
      workflow: { type: "string" },
      "add-publish-job": { type: "boolean", default: false },
      environment: { type: "string" },
      "dry-run": { type: "boolean", default: false },
      registry: { type: "string", multiple: true },
      package: { type: "string" },
      execute: { type: "boolean", default: false },
      all: { type: "boolean", default: false },
      yes: { type: "boolean", default: false },
      "no-browser": { type: "boolean", default: false },
      "open-with": { type: "string" },
      "keep-session": { type: "boolean", default: false },
      manual: { type: "boolean", default: false },
      browser: { type: "string", default: "default" },
      "browser-channel": { type: "string", default: "chrome" },
      "browser-profile": { type: "string" },
      "browser-executable": { type: "string" },
      "browser-import": { type: "string" },
      "browser-import-scope": { type: "string" },
      "browser-attach": { type: "string" },
      "browser-pref": { type: "string", multiple: true },
      "browser-restriction": { type: "string", multiple: true },
      help: { type: "boolean", short: "h", default: false },
      version: { type: "boolean", short: "V", default: false },
    },
  });
  if (values.help) {
    process.stdout.write(HELP);
    return;
  }
  if (values.version) {
    process.stdout.write(`package-registry-manager ${packageVersion()}\n`);
    return;
  }
  if (positionals.length !== 1) {
    throw new Error("expected exactly one command: inspect, plan, or setup");
  }
  if (!["text", "json"].includes(values.format)) {
    throw new Error("--format must be 'text' or 'json'");
  }
  if (!BROWSER_MODES.includes(values.browser)) {
    throw new Error("--browser must be 'default' or 'automated'");
  }
  const browserOptions = parseBrowserOptions({
    channel: values["browser-channel"],
    executable: values["browser-executable"],
    importFrom: values["browser-import"],
    importScope: values["browser-import-scope"],
    attach: values["browser-attach"],
    preferences: values["browser-pref"],
    restrictions: values["browser-restriction"],
    profileGiven: values["browser-profile"] !== undefined,
  });

  const repository = path.resolve(values.repository);
  const discovered = values.ref
    ? await inspectReference(repository, values.ref, {
        includeSkipped: values.verbose,
      })
    : await inspectRepository(repository, { includeSkipped: values.verbose });
  const inspection = values.offline
    ? discovered
    : await probeRegistryState(discovered, { verbose: values.verbose });
  const command = positionals[0];
  if (command === "inspect") {
    outputInspection(inspection, values.format);
    return;
  }

  const registries = (values.registry ?? []).map(parseRegistry);
  if (!["plan", "setup"].includes(command)) {
    throw new Error(`unknown command '${command}'`);
  }
  const environment = await probeEnvironment({
    offline: values.offline,
    verbose: values.verbose,
    python: inspection.packages.some(
      (item) =>
        item.registry === "pypi" &&
        item.publishable &&
        (registries.length === 0 || registries.includes("pypi")),
    ),
    npm: inspection.packages.some(
      (item) =>
        item.registry === "npm" &&
        item.publishable &&
        (registries.length === 0 || registries.includes("npm")),
    ),
  });
  const planOptions = {
    ref: values.ref,
    verifyRelease: values["verify-release"],
    manual: values.manual,
    workflow: await workflowOverride(
      repository,
      values.workflow,
      values["add-publish-job"],
    ),
    addPublishJob: values["add-publish-job"],
    publisherEnvironment: nonEmpty(values.environment, "--environment"),
    environment,
    browser: browserSummary(command, values, browserOptions),
  };
  if (command === "plan") {
    const plans = filterPlans(
      buildPlans(inspection, registries, planOptions),
      values.package,
    );
    outputPlans(plans, values.format);
    return;
  }
  if (values.all && values.package) {
    throw new Error("--all and --package are mutually exclusive");
  }
  if (!values.all && registries.length !== 1) {
    throw new Error("setup requires exactly one --registry <registry>");
  }
  if (values["dry-run"] && values.execute) {
    throw new Error("--dry-run and --execute are mutually exclusive");
  }
  if (values.yes && !values.execute) {
    throw new Error("--yes requires --execute");
  }
  if (values["no-browser"] && !values.execute) {
    throw new Error("--no-browser requires --execute");
  }
  if (values["open-with"] !== undefined && values["no-browser"]) {
    throw new Error("--open-with and --no-browser are mutually exclusive");
  }

  const plans = buildPlans(inspection, registries, planOptions);
  const selected = values.all
    ? plans.filter((plan) => plan.package.publishable)
    : [selectPlan(plans, values.package)];
  outputPlans(selected, values.format);
  await executePlans(
    selected,
    {
      inspection,
      workflow: planOptions.workflow,
      publisherEnvironment: planOptions.publisherEnvironment,
      repository,
      execute: values.execute,
      yes: values.yes,
      noBrowser: values["no-browser"],
      openWith: nonEmpty(values["open-with"], "--open-with"),
      keepSession: values["keep-session"],
      browser: values.browser,
      browserOptions,
      browserProfile: path.resolve(
        values["browser-profile"] ??
          defaultBrowserProfile({ channel: browserOptions.channel }),
      ),
      verbose: values.verbose,
      verifyRelease: values["verify-release"],
    },
    values.all,
  );
}

async function workflowOverride(repository, workflow, adding = false) {
  if (workflow === undefined) {
    return undefined;
  }
  if (!/^[\w.-]+\.ya?ml$/.test(workflow)) {
    throw new Error(
      "--workflow must be a workflow file name in .github/workflows, such as release.yml",
    );
  }
  const file = path.join(repository, ".github/workflows", workflow);
  const found = await stat(file).then(
    (metadata) => metadata.isFile(),
    () => false,
  );
  if (!found && !adding) {
    throw new Error(`--workflow: .github/workflows/${workflow} does not exist`);
  }
  return workflow;
}

function nonEmpty(value, option) {
  if (value !== undefined && !value.trim()) {
    throw new Error(`${option} must not be empty`);
  }
  return value?.trim();
}

function browserSummary(command, values, browserOptions) {
  if (command === "setup" && values["no-browser"]) {
    return { mode: "none" };
  }
  if (command !== "setup") {
    return { mode: "default", channel: values["browser-channel"] };
  }
  return {
    mode: values.browser,
    channel: browserOptions.channel,
    profile:
      values.browser === "automated" && !browserOptions.attach
        ? path.resolve(
            values["browser-profile"] ??
              defaultBrowserProfile({ channel: browserOptions.channel }),
          )
        : undefined,
    import: browserOptions.import ?? undefined,
    attach: browserOptions.attach ?? undefined,
  };
}

/** Keeps the plans of `packageName`, or every plan without one. */
function filterPlans(plans, packageName) {
  if (plans.length === 0) {
    throw new Error("no matching package manifests were found");
  }
  if (!packageName) {
    return plans;
  }
  const named = plans.filter((plan) => plan.package.name === packageName);
  if (named.length === 0) {
    throw new Error(`package '${packageName}' was not found`);
  }
  return named;
}

function selectPlan(plans, packageName) {
  if (packageName) {
    const plan = plans.find(
      (candidate) => candidate.package.name === packageName,
    );
    if (!plan) {
      throw new Error(
        `package '${packageName}' was not found for this registry`,
      );
    }
    return plan;
  }
  if (plans.length === 0) {
    throw new Error("no matching package manifests were found");
  }
  const publishable = plans.filter((plan) => plan.package.publishable);
  const candidates = publishable.length > 0 ? publishable : plans;
  if (candidates.length > 1) {
    throw new Error(
      "multiple packages use this registry; select one with --package <name>",
    );
  }
  return candidates[0];
}

function outputInspection(inspection, format) {
  if (format === "json") {
    process.stdout.write(`${JSON.stringify(inspection, null, 2)}\n`);
    return;
  }
  process.stdout.write(`Repository: ${inspection.repository.root}\n`);
  if (inspection.packages.length === 0) {
    process.stdout.write("No supported package manifests found.\n");
  }
  for (const packageInfo of inspection.packages) {
    const details = [
      packageInfo.manifest,
      packageInfo.publishable ? "publishable" : "not publishable",
    ];
    if (packageInfo.exists_on_registry !== undefined) {
      details.push(
        packageInfo.exists_on_registry ? "published" : "not published yet",
      );
    }
    if (packageInfo.trusted_publishing) {
      details.push("trusted publishing");
    }
    process.stdout.write(
      `- ${packageInfo.registry}: ${packageInfo.name} (${details.join(", ")})\n`,
    );
    if (packageInfo.workflow && packageInfo.publishable) {
      process.stdout.write(`  workflow: ${publisherSummary(packageInfo)}\n`);
    }
    for (const warning of packageInfo.warnings ?? []) {
      process.stdout.write(`  warning: ${warning}\n`);
    }
  }
  for (const item of inspection.skipped ?? []) {
    process.stdout.write(`skipped ${item.manifest}: ${item.reason}\n`);
  }
}

function publisherSummary({ workflow, workflow_jobs, environment }) {
  const details = [];
  if (workflow_jobs?.length) {
    details.push(
      `${workflow_jobs.length > 1 ? "jobs" : "job"} ${workflow_jobs.join(", ")}`,
    );
  }
  if (environment) {
    details.push(`environment ${environment}`);
  }
  return details.length > 0 ? `${workflow} (${details.join("; ")})` : workflow;
}

function outputPlans(plans, format) {
  if (format === "json") {
    process.stdout.write(`${JSON.stringify(plans, null, 2)}\n`);
    return;
  }
  for (const plan of plans) {
    const mode = plan.mode ? ` (${plan.mode})` : "";
    process.stdout.write(`${plan.registry}: ${plan.package.name}${mode}\n`);
    process.stdout.write(
      `  credentials: ${plan.credential_policy.description}\n`,
    );
    if (plan.mode === "complete") {
      process.stdout.write("  trusted publishing is already in use\n");
    }
    if (plan.skipped_reason) {
      process.stdout.write(`  skipped: ${plan.skipped_reason}\n`);
    }
    if (plan.trusted_publisher) {
      process.stdout.write(
        `  trusted publisher: ${publisherSummary({
          ...plan.package,
          workflow: plan.trusted_publisher.workflow,
          environment: plan.trusted_publisher.environment,
          workflow_jobs:
            plan.trusted_publisher.workflow === plan.package.workflow
              ? plan.package.workflow_jobs
              : undefined,
        })}\n`,
      );
    }
    for (const warning of plan.package.warnings ?? []) {
      process.stdout.write(`  warning: ${warning}\n`);
    }
    for (const line of renderPrerequisites(plan.prerequisites ?? [])) {
      process.stdout.write(`${line}\n`);
    }
    plan.steps.forEach((step, index) => {
      const when = step.when ? ` [when ${step.when}]` : "";
      process.stdout.write(`  ${index + 1}. ${step.title}${when}\n`);
      if (step.command) {
        const cwd = step.cwd && step.cwd !== "." ? `(in ${step.cwd}) ` : "";
        process.stdout.write(
          `     $ ${cwd}${step.command.program} ${step.command.args.join(" ")}\n`,
        );
      }
      if (step.url) {
        process.stdout.write(`     ${step.url}\n`);
      }
    });
  }
}

function packageVersion() {
  const manifest = new URL("../package.json", import.meta.url);
  return JSON.parse(readFileSync(manifest, "utf8")).version;
}

if (isDirectExecution(import.meta.url, process.argv[1])) {
  main().catch((error) => {
    process.stderr.write(`error: ${error.message}\n`);
    process.exitCode = 1;
  });
}

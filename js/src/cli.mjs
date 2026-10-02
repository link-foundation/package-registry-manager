#!/usr/bin/env node

import path from "node:path";
import process from "node:process";
import { pathToFileURL } from "node:url";
import { parseArgs } from "node:util";

import { parseBrowserOptions } from "./browser-options.mjs";
import { inspectRepository } from "./discovery.mjs";
import { parseRegistry } from "./model.mjs";
import { buildPlans } from "./plan.mjs";
import { probeEnvironment, renderPrerequisites } from "./prerequisites.mjs";
import { probeRegistryState } from "./registry-state.mjs";
import { BROWSER_MODES, defaultBrowserProfile, executePlan } from "./setup.mjs";

const HELP = `Usage: package-registry-manager-js [global options] <command>

Commands:
  inspect                         Discover supported package manifests
  plan [--registry <registry>]    Print ordered setup plans
  setup --registry <registry>     Bootstrap or attach trusted publishing

Global options:
  --repository <path>             Repository to inspect (default: .)
  --format <text|json>            Output format (default: text)
  --offline                       Do not look up packages on registries
  --verbose                       Print command details and output

Plan and setup options:
  --verify-release                Also watch the release workflow and confirm
                                  that the next version has provenance

Setup options:
  --package <name>                Select one of multiple packages
  --dry-run                       Print the flow without running it (default)
  --execute                       Run the flow and open a visible browser
  --yes                           Confirm publishing, secret changes, and
                                  form submission in advance
  --no-browser                    Print the setup URL instead
  --browser <default|automated>   Open sign-in and approval pages in your
                                  default browser, or in the automated
                                  profile too (default: default)
  --browser-channel <channel>     Installed browser channel: chrome, chromium,
                                  brave, msedge, msedge-beta, msedge-dev, or
                                  msedge-canary (default: chrome)
  --browser-executable <path>     Installed browser executable to launch
                                  instead of the channel's
  --browser-profile <path>        Dedicated automation profile, used to fill
                                  forms (default: per-user state directory)
  --browser-import <browser>[:<profile>]
                                  Copy cookies, history, and other data from
                                  your chrome, edge, brave, or firefox profile
                                  into the automated profile first
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
      repository: { type: "string", default: "." },
      format: { type: "string", default: "text" },
      verbose: { type: "boolean", default: false },
      offline: { type: "boolean", default: false },
      "verify-release": { type: "boolean", default: false },
      "dry-run": { type: "boolean", default: false },
      registry: { type: "string", multiple: true },
      package: { type: "string" },
      execute: { type: "boolean", default: false },
      yes: { type: "boolean", default: false },
      "no-browser": { type: "boolean", default: false },
      browser: { type: "string", default: "default" },
      "browser-channel": { type: "string", default: "chrome" },
      "browser-profile": { type: "string" },
      "browser-executable": { type: "string" },
      "browser-import": { type: "string" },
      "browser-attach": { type: "string" },
      "browser-pref": { type: "string", multiple: true },
      "browser-restriction": { type: "string", multiple: true },
      help: { type: "boolean", short: "h", default: false },
    },
  });
  if (values.help) {
    process.stdout.write(HELP);
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
    attach: values["browser-attach"],
    preferences: values["browser-pref"],
    restrictions: values["browser-restriction"],
    profileGiven: values["browser-profile"] !== undefined,
  });

  const repository = path.resolve(values.repository);
  const discovered = await inspectRepository(repository);
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
    npm: inspection.packages.some(
      (item) =>
        item.registry === "npm" &&
        item.publishable &&
        (registries.length === 0 || registries.includes("npm")),
    ),
  });
  const planOptions = {
    verifyRelease: values["verify-release"],
    environment,
    browser: browserSummary(command, values, browserOptions),
  };
  if (command === "plan") {
    const plans = buildPlans(inspection, registries, planOptions);
    if (plans.length === 0) {
      throw new Error("no matching package manifests were found");
    }
    outputPlans(plans, values.format);
    return;
  }
  if (registries.length !== 1) {
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

  const plans = buildPlans(inspection, registries, planOptions);
  const plan = selectPlan(plans, values.package);
  outputPlans([plan], values.format);
  await executePlan(plan, {
    repository,
    execute: values.execute,
    yes: values.yes,
    noBrowser: values["no-browser"],
    browser: values.browser,
    browserOptions,
    browserProfile: path.resolve(
      values["browser-profile"] ?? defaultBrowserProfile(),
    ),
    verbose: values.verbose,
    verifyRelease: values["verify-release"],
  });
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
        ? path.resolve(values["browser-profile"] ?? defaultBrowserProfile())
        : undefined,
    import: browserOptions.import ?? undefined,
    attach: browserOptions.attach ?? undefined,
  };
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
    for (const warning of packageInfo.warnings ?? []) {
      process.stdout.write(`  warning: ${warning}\n`);
    }
  }
}

function outputPlans(plans, format) {
  if (format === "json") {
    process.stdout.write(`${JSON.stringify(plans, null, 2)}\n`);
    return;
  }
  for (const plan of plans) {
    const mode = plan.mode ? ` (${plan.mode})` : "";
    process.stdout.write(`${plan.registry}: ${plan.package.name}${mode}\n`);
    if (plan.mode === "complete") {
      process.stdout.write("  trusted publishing is already in use\n");
    }
    if (plan.skipped_reason) {
      process.stdout.write(`  skipped: ${plan.skipped_reason}\n`);
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

export function isDirectExecution(moduleUrl, entryPath) {
  return Boolean(entryPath) && moduleUrl === pathToFileURL(entryPath).href;
}

if (isDirectExecution(import.meta.url, process.argv[1])) {
  main().catch((error) => {
    process.stderr.write(`error: ${error.message}\n`);
    process.exitCode = 1;
  });
}

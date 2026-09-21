#!/usr/bin/env node

import path from 'node:path';
import process from 'node:process';
import { parseArgs } from 'node:util';

import { inspectRepository } from './discovery.mjs';
import { parseRegistry } from './model.mjs';
import { buildPlans } from './plan.mjs';
import { defaultBrowserProfile, executePlan } from './setup.mjs';

const HELP = `Usage: package-registry-manager-js [global options] <command>

Commands:
  inspect                         Discover supported package manifests
  plan [--registry <registry>]    Print ordered setup plans
  setup --registry <registry>     Validate and open registry setup

Global options:
  --repository <path>             Repository to inspect (default: .)
  --format <text|json>            Output format (default: text)
  --verbose                       Print command details and output

Setup options:
  --package <name>                Select one of multiple packages
  --execute                       Run checks and open a visible browser
  --yes                           Confirm npm form submission
  --no-browser                    Print the setup URL instead
  --browser-channel <channel>     Installed browser channel (default: chrome)
  --browser-profile <path>        Dedicated automation profile
`;

export async function main(args = process.argv.slice(2)) {
  const { values, positionals } = parseArgs({
    args,
    allowPositionals: true,
    strict: true,
    options: {
      repository: { type: 'string', default: '.' },
      format: { type: 'string', default: 'text' },
      verbose: { type: 'boolean', default: false },
      registry: { type: 'string', multiple: true },
      package: { type: 'string' },
      execute: { type: 'boolean', default: false },
      yes: { type: 'boolean', default: false },
      'no-browser': { type: 'boolean', default: false },
      'browser-channel': { type: 'string', default: 'chrome' },
      'browser-profile': { type: 'string' },
      help: { type: 'boolean', short: 'h', default: false },
    },
  });
  if (values.help) {
    process.stdout.write(HELP);
    return;
  }
  if (positionals.length !== 1) {
    throw new Error('expected exactly one command: inspect, plan, or setup');
  }
  if (!['text', 'json'].includes(values.format)) {
    throw new Error("--format must be 'text' or 'json'");
  }

  const repository = path.resolve(values.repository);
  const inspection = await inspectRepository(repository);
  const command = positionals[0];
  if (command === 'inspect') {
    outputInspection(inspection, values.format);
    return;
  }

  const registries = (values.registry ?? []).map(parseRegistry);
  if (command === 'plan') {
    const plans = buildPlans(inspection, registries);
    if (plans.length === 0) throw new Error('no matching package manifests were found');
    outputPlans(plans, values.format);
    return;
  }
  if (command !== 'setup') {
    throw new Error(`unknown command '${command}'`);
  }
  if (registries.length !== 1) {
    throw new Error('setup requires exactly one --registry <registry>');
  }
  if (values.yes && !values.execute) throw new Error('--yes requires --execute');
  if (values['no-browser'] && !values.execute) {
    throw new Error('--no-browser requires --execute');
  }

  const plans = buildPlans(inspection, registries);
  const plan = selectPlan(plans, values.package);
  outputPlans([plan], values.format);
  await executePlan(plan, {
    repository,
    execute: values.execute,
    yes: values.yes,
    noBrowser: values['no-browser'],
    browserChannel: values['browser-channel'],
    browserProfile: path.resolve(
      values['browser-profile'] ?? defaultBrowserProfile(repository)
    ),
    verbose: values.verbose,
  });
}

function selectPlan(plans, packageName) {
  if (packageName) {
    const plan = plans.find((candidate) => candidate.package.name === packageName);
    if (!plan) throw new Error(`package '${packageName}' was not found for this registry`);
    return plan;
  }
  if (plans.length === 0) throw new Error('no matching package manifests were found');
  if (plans.length > 1) {
    throw new Error('multiple packages use this registry; select one with --package <name>');
  }
  return plans[0];
}

function outputInspection(inspection, format) {
  if (format === 'json') {
    process.stdout.write(`${JSON.stringify(inspection, null, 2)}\n`);
    return;
  }
  process.stdout.write(`Repository: ${inspection.repository.root}\n`);
  if (inspection.packages.length === 0) {
    process.stdout.write('No supported package manifests found.\n');
  }
  for (const packageInfo of inspection.packages) {
    process.stdout.write(
      `- ${packageInfo.registry}: ${packageInfo.name} (${packageInfo.manifest}, ${
        packageInfo.publishable ? 'publishable' : 'not publishable'
      })\n`
    );
  }
}

function outputPlans(plans, format) {
  if (format === 'json') {
    process.stdout.write(`${JSON.stringify(plans, null, 2)}\n`);
    return;
  }
  for (const plan of plans) {
    process.stdout.write(`${plan.registry}: ${plan.package.name}\n`);
    plan.steps.forEach((step, index) => {
      process.stdout.write(`  ${index + 1}. ${step.title}\n`);
      if (step.command) {
        process.stdout.write(
          `     $ ${step.command.program} ${step.command.args.join(' ')}\n`
        );
      }
      if (step.url) process.stdout.write(`     ${step.url}\n`);
    });
  }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  main().catch((error) => {
    process.stderr.write(`error: ${error.message}\n`);
    process.exitCode = 1;
  });
}

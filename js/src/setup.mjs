import { createInterface } from 'node:readline/promises';
import path from 'node:path';
import { stdin, stdout } from 'node:process';

import { launchRealBrowser } from 'browser-commander';
import { exec } from 'command-stream';

import { npmPrefillScript } from './browser.mjs';
import { packageDirectory } from './plan.mjs';

export function defaultBrowserProfile(repository) {
  return path.join(repository, '.package-registry-manager', 'browser-profile');
}

export async function executePlan(plan, options) {
  if (!plan.package.publishable) {
    throw new Error(
      `${plan.package.name} is not publishable: ${(plan.package.problems ?? []).join('; ')}`
    );
  }
  if (!options.execute) {
    console.log(
      'Dry run only. Re-run with --execute to perform checks and open the registry.'
    );
    return;
  }

  const packageRoot = path.join(
    options.repository,
    packageDirectory(plan.package.manifest)
  );
  for (const step of plan.steps) {
    if (step.command) {
      await runCommand(step.command, packageRoot, options.verbose);
    }
  }

  const browserStep = plan.steps.find((step) => step.url);
  if (!browserStep) return;
  if (options.noBrowser) {
    console.log(`Open ${browserStep.url}`);
    return;
  }

  const connection = await launchRealBrowser({
    engine: 'playwright',
    channel: options.browserChannel,
    userDataDir: options.browserProfile,
    headless: false,
    verbose: options.verbose,
  });
  try {
    await connection.page.goto(browserStep.url);
    console.log('Sign in and navigate to the setup form in the opened browser.');
    await prompt('Press Enter when the form is visible...');
    if (plan.registry === 'npm') {
      if (!plan.trusted_publisher) {
        throw new Error(
          'npm setup needs GitHub owner, repository, and release workflow metadata'
        );
      }
      const script = npmPrefillScript(plan.trusted_publisher, false);
      let result = await connection.page.evaluate(script);
      if (result.filled?.length === 0) {
        await new Promise((resolve) => setTimeout(resolve, 500));
        result = await connection.page.evaluate(script);
      }
      console.log(`Prefill result: ${JSON.stringify(result)}`);
      const submit =
        options.yes ||
        /^(?:y|yes)$/i.test(
          await prompt('Submit this trusted-publisher configuration? [y/N] ')
        );
      if (submit) {
        const submission = await connection.page.evaluate(
          npmPrefillScript(plan.trusted_publisher, true)
        );
        if (!submission.submitted) {
          throw new Error(
            'the form was not submitted; review the visible browser and submit manually'
          );
        }
      }
    }
    await prompt(
      'Review the registry result, then press Enter to close the managed browser...'
    );
  } finally {
    await connection.browser.close();
  }
}

async function runCommand(command, cwd, verbose) {
  if (verbose) {
    console.error(`+ (cd ${cwd} && ${command.program} ${command.args.join(' ')})`);
  }
  const result = await exec(command.program, command.args, {
    cwd,
    capture: true,
    mirror: verbose,
    stdin: 'ignore',
  });
  if (!verbose && result.code !== 0) {
    if (result.stdout) stdout.write(String(result.stdout));
    if (result.stderr) process.stderr.write(String(result.stderr));
  }
  if (result.code !== 0) {
    throw new Error(`${command.program} exited with status ${result.code}`);
  }
}

async function prompt(message) {
  const terminal = createInterface({ input: stdin, output: stdout });
  try {
    return await terminal.question(message);
  } finally {
    terminal.close();
  }
}

import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { createInterface } from "node:readline/promises";
import { stdin, stdout } from "node:process";

import { launchRealBrowser } from "browser-commander";
import { exec } from "command-stream";

import {
  nodeOptionsWithShim,
  runInteractive,
  writeTtyShim,
} from "./auth-urls.mjs";
import { npmPrefillScript } from "./browser.mjs";
import { CLEANUP_CONDITIONS } from "./flows.mjs";
import { packageDirectory } from "./plan.mjs";
import { getJson, probePackage } from "./registry-state.mjs";

const PREFILLED_FORMS = new Set([
  "configure-trusted-publisher",
  "create-pending-publisher",
]);
const INTERACTIVE_CHECKS = new Set(["check-trust", "verify-trusted-publisher"]);

export function defaultBrowserProfile(repository) {
  return path.join(repository, ".package-registry-manager", "browser-profile");
}

/**
 * Runs a setup plan. Without `options.execute` it only reports a dry run.
 * Steps whose `when` condition does not hold are skipped, so a re-run after a
 * partial success resumes where the previous run stopped.
 */
export async function executePlan(plan, options) {
  if (!plan.package.publishable) {
    throw new Error(
      `${plan.package.name} is not publishable: ${(plan.package.problems ?? []).join("; ")}`,
    );
  }
  if (plan.mode === "complete") {
    console.log(
      `${plan.package.name} already publishes through trusted publishing; nothing to do.`,
    );
    return;
  }
  if (!options.execute) {
    console.log(
      "Dry run only. Re-run with --execute to run these steps and open the registry.",
    );
    return;
  }
  const session = new SetupSession(plan, options);
  try {
    await session.run();
  } finally {
    await session.cleanup();
  }
}

class SetupSession {
  constructor(plan, options) {
    this.plan = plan;
    this.options = options;
    this.conditions = new Set(["verify-release"]);
    if (plan.package.exists_on_registry === false) {
      this.conditions.add("package-missing");
    }
    if (plan.package.trusted_publishing !== true) {
      this.conditions.add("trust-missing");
    }
    this.values = {};
    this.deferred = [];
    this.connection = null;
    this.temporary = null;
    this.shim = null;
  }

  async run() {
    for (const step of this.plan.steps) {
      if (CLEANUP_CONDITIONS.has(step.when)) {
        this.deferred.push(step);
      } else if (!step.when || this.conditions.has(step.when)) {
        this.log(`==> ${step.title}`);
        await this.runStep(step);
      } else if (this.options.verbose) {
        console.error(`skip ${step.id}: condition ${step.when} does not hold`);
      }
    }
  }

  async cleanup() {
    for (const step of this.deferred) {
      if (this.conditions.has(step.when)) {
        this.log(`==> ${step.title}`);
        const result = await this.runProcess(step, true);
        if (result.code !== 0) {
          console.error(`warning: ${step.id} exited with ${result.code}`);
        }
      }
    }
    if (this.connection) {
      await this.connection.browser.close();
    }
    for (const directory of [this.temporary, this.shim?.directory]) {
      if (directory) {
        await rm(directory, { recursive: true, force: true });
      }
    }
  }

  async runStep(step) {
    switch (step.kind) {
      case "check":
        return step.command ? this.check(step) : this.checkRegistry();
      case "command":
        return this.command(step);
      case "wait":
        return this.wait(step);
      case "browser":
        return this.browser(step);
      default:
        console.log(step.description);
        if (step.url) {
          console.log(`  ${step.url}`);
        }
        await prompt("Press Enter when this is done...");
    }
  }

  async checkRegistry() {
    const state = await probePackage(this.plan.package, this.options);
    if (state.exists === undefined) {
      throw new Error(
        `could not determine whether ${this.plan.package.name} exists on ${this.plan.registry}`,
      );
    }
    toggle(this.conditions, "package-missing", !state.exists);
    if (state.trusted === true) {
      this.conditions.delete("trust-missing");
    }
    this.values.previous_version = state.version;
    if (
      !state.exists &&
      !this.plan.steps.some((s) => s.when === "package-missing")
    ) {
      throw new Error(
        `${this.plan.package.name} is missing from the registry; re-run plan to get the bootstrap steps`,
      );
    }
    console.log(
      state.exists
        ? "  The package exists; attaching trusted publishing."
        : "  The package is not published yet; bootstrapping it.",
    );
  }

  async check(step) {
    const result = INTERACTIVE_CHECKS.has(step.id)
      ? await this.runProcess(step, false)
      : await this.capture(step);
    const output = String(result.stdout ?? "");
    switch (step.id) {
      case "check-sign-in":
        toggle(this.conditions, "signed-out", result.code !== 0);
        return;
      case "check-trust":
      case "verify-trusted-publisher": {
        const trusted =
          result.code === 0 && /"(?:id|type|file)"\s*:/.test(output);
        toggle(this.conditions, "trust-missing", !trusted);
        if (step.id === "verify-trusted-publisher" && !trusted) {
          throw new Error(
            "npm does not list a trusted publisher for the package",
          );
        }
        return;
      }
      case "audit-token-secrets":
        if (result.code !== 0) {
          console.error("warning: could not list repository secrets with gh");
        } else if (
          JSON.parse(output || "[]").some((item) => item.name === "NPM_TOKEN")
        ) {
          console.log(
            "  NPM_TOKEN is no longer needed with trusted publishing.",
          );
          this.conditions.add("token-secret-present");
        }
        return;
      case "find-release-run": {
        const [run] = result.code === 0 ? JSON.parse(output || "[]") : [];
        if (!run) {
          throw new Error("no release workflow run was found");
        }
        this.values.run_id = String(run.databaseId);
        return;
      }
      default:
        if (result.code !== 0) {
          throw new Error(
            `${step.command.program} exited with status ${result.code}`,
          );
        }
    }
  }

  async command(step) {
    if (step.confirm && !this.options.yes) {
      const rendered = render(this.expand(step.command));
      const answer = await prompt(`Run \`${rendered}\`? [y/N] `);
      if (!/^(?:y|yes)$/i.test(answer)) {
        if (step.id === "first-publish") {
          throw new Error("the first publish was declined");
        }
        console.log(`  skipped ${step.id}`);
        return;
      }
    }
    if (step.id === "prepare-worktree") {
      this.temporary = await mkdtemp(path.join(os.tmpdir(), "prm-release-"));
      this.values.worktree = path.join(this.temporary, "worktree");
      this.values.pack_destination = this.temporary;
    }
    const result = await this.runProcess(step, step.id !== "pack");
    if (result.code !== 0) {
      if (step.id === "attach-trusted-publisher") {
        console.error(
          "warning: npm trust failed; falling back to the browser form",
        );
        this.conditions.add("trust-cli-failed");
        return;
      }
      if (step.id === "trigger-release") {
        console.error(
          "warning: could not dispatch the workflow; watching its latest run",
        );
        return;
      }
      throw new Error(
        `${step.command.program} exited with status ${result.code}`,
      );
    }
    if (step.id === "sign-in") {
      this.conditions.add("tool-signed-in");
    } else if (step.id === "prepare-worktree") {
      this.conditions.add("worktree-created");
    } else if (step.id === "pack") {
      this.recordPack(result.stdout);
    }
  }

  recordPack(output) {
    const [packed] = JSON.parse(output);
    for (const file of packed.files ?? []) {
      console.log(`  ${String(file.size).padStart(8)}  ${file.path}`);
    }
    console.log(
      `  ${packed.filename}: ${packed.size} bytes packed, ${packed.unpackedSize} bytes unpacked, ${packed.entryCount} files`,
    );
    this.values.tarball = path.join(
      this.values.pack_destination,
      packed.filename,
    );
    this.values.version = packed.version;
  }

  async wait(step) {
    const url = this.expandText(step.url);
    const deadline = Date.now() + (this.options.waitTimeoutMs ?? 20 * 60_000);
    const interval = this.options.pollIntervalMs ?? 5_000;
    for (;;) {
      const document = await getJson(url, this.options);
      if (
        document &&
        (step.id !== "confirm-provenance" ||
          isTrustedRelease(document, this.values))
      ) {
        console.log(`  ${url} is ready`);
        return;
      }
      if (Date.now() > deadline) {
        throw new Error(`timed out waiting for ${url}`);
      }
      await new Promise((resolve) => setTimeout(resolve, interval));
    }
  }

  async browser(step) {
    await this.open(step.url);
    if (
      PREFILLED_FORMS.has(step.id) &&
      this.plan.trusted_publisher &&
      this.connection
    ) {
      await this.prefill();
    }
    await prompt("Finish this step in the browser, then press Enter...");
    if (step.id === "create-pending-publisher") {
      this.conditions.delete("trust-missing");
    }
  }

  async prefill() {
    await prompt("Press Enter when the form is visible...");
    const page = this.connection.page;
    const script = npmPrefillScript(this.plan.trusted_publisher, false);
    let result = await page.evaluate(script);
    if (result.filled?.length === 0) {
      await new Promise((resolve) => setTimeout(resolve, 500));
      result = await page.evaluate(script);
    }
    console.log(`Prefill result: ${JSON.stringify(result)}`);
    const submit =
      this.options.yes ||
      /^(?:y|yes)$/i.test(
        await prompt("Submit this trusted-publisher configuration? [y/N] "),
      );
    if (submit) {
      const submission = await page.evaluate(
        npmPrefillScript(this.plan.trusted_publisher, true),
      );
      if (!submission.submitted) {
        throw new Error(
          "the form was not submitted; review the visible browser and submit manually",
        );
      }
    }
  }

  async open(url) {
    if (this.options.noBrowser) {
      console.log(`Open ${url}`);
      return;
    }
    if (!this.connection) {
      this.connection = await launchRealBrowser({
        engine: "playwright",
        channel: this.options.browserChannel,
        userDataDir: this.options.browserProfile,
        headless: false,
        verbose: this.options.verbose,
      });
    }
    await this.connection.page.goto(url);
  }

  cwd(step) {
    const relative = step.cwd ?? packageDirectory(this.plan.package.manifest);
    const expanded = this.expandText(relative);
    return path.isAbsolute(expanded)
      ? expanded
      : path.join(this.options.repository, expanded);
  }

  prepare(step) {
    const command = this.expand(step.command);
    const cwd = this.cwd(step);
    if (this.options.verbose) {
      console.error(`+ (cd ${cwd} && ${render(command)})`);
    }
    return { command, cwd };
  }

  async capture(step) {
    const { command, cwd } = this.prepare(step);
    const result = await exec(command.program, command.args, {
      cwd,
      capture: true,
      mirror: this.options.verbose,
      stdin: "ignore",
    });
    if (
      !this.options.verbose &&
      result.code !== 0 &&
      step.id !== "check-sign-in"
    ) {
      stdout.write(String(result.stdout ?? ""));
      process.stderr.write(String(result.stderr ?? ""));
    }
    return result;
  }

  async runProcess(step, mirror) {
    const { command, cwd } = this.prepare(step);
    let env = process.env;
    if (["npm", "npx"].includes(command.program)) {
      this.shim ??= await writeTtyShim();
      env = {
        ...process.env,
        NODE_OPTIONS: nodeOptionsWithShim(
          process.env.NODE_OPTIONS,
          this.shim.shim,
        ),
      };
    }
    return runInteractive(command, {
      cwd,
      env,
      mirror,
      onUrl: (url) => {
        this.open(url).catch((error) =>
          console.error(`warning: could not open ${url}: ${error.message}`),
        );
      },
    });
  }

  expand(command) {
    return {
      program: command.program,
      args: command.args.map((arg) => this.expandText(arg)),
    };
  }

  expandText(text) {
    return text.replaceAll(
      /\{(\w+)\}/g,
      (match, key) => this.values[key] ?? match,
    );
  }

  log(message) {
    console.log(message);
  }
}

function isTrustedRelease(document, values) {
  return (
    Boolean(document._npmUser?.trustedPublisher) &&
    Boolean(document.dist?.attestations) &&
    document.version !== values.previous_version
  );
}

function toggle(set, value, enabled) {
  if (enabled) {
    set.add(value);
  } else {
    set.delete(value);
  }
}

function render(command) {
  return [command.program, ...command.args].join(" ");
}

async function prompt(message) {
  const terminal = createInterface({ input: stdin, output: stdout });
  try {
    return await terminal.question(message);
  } finally {
    terminal.close();
  }
}

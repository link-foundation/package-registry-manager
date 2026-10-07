import { mkdtemp, readFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { createInterface } from "node:readline/promises";
import { stdin, stdout } from "node:process";

import { exec } from "command-stream";

import {
  ANSI,
  nodeOptionsWithShim,
  runInteractive,
  writeTtyShim,
} from "./auth-urls.mjs";
import {
  approvalDeadline,
  TWO_FACTOR_HINT,
  withFreshLinks,
} from "./approvals.mjs";
import { connectAutomation } from "./automation.mjs";
import { runCratesApiStep } from "./crates-api.mjs";
import {
  detachedRunner,
  npmPrefillScript,
  openInUserBrowser,
} from "./browser.mjs";
import { detectDefaultBrowser, openWithCommand } from "./default-browser.mjs";
import { CLEANUP_CONDITIONS } from "./flows.mjs";
import { setupCredential } from "./credential-setup.mjs";
import { checkNamePolicy, policyRefusal } from "./npm-policy.mjs";
import { pagesSettingsUrl, pagesState } from "./pages.mjs";
import { packageDirectory } from "./plan.mjs";
import { twoFactorMode } from "./prerequisites.mjs";
import { getJson, probePackage } from "./registry-state.mjs";
import { launchBrowser, signInDomains } from "./sign-in-import.mjs";
import { reportTokenSecrets, verifyTokenRevoked } from "./tokens.mjs";

export { defaultBrowserProfile } from "./profile.mjs";

const PREFILLED_FORMS = new Set([
  "configure-trusted-publisher",
  "create-pending-publisher",
]);
const INTERACTIVE_CHECKS = new Set(["check-trust", "verify-trusted-publisher"]);
const PAGES_CHANGES = new Set(["enable-pages", "use-pages-workflow"]);
/** Checks whose failure is an answer, so their output is not echoed. */
const QUIET_CHECKS = new Set(["check-sign-in", "check-pages"]);
/** A trusted publisher in `npm trust list` output, human-readable or JSON. */
const LISTED_PUBLISHER = /^\s*(?:id|type|file):\s*\S|"(?:id|type|file)"\s*:/m;
/** What `npm trust github` prints once the registry stored the publisher. */
const TRUST_CREATED = /Trust configuration created successfully/i;
/** npm's answer when a workflow publishes without an attached trusted publisher. */
const PUBLISH_REJECTED = /\bE404\b|404 Not Found|invalid-publisher/i;
/** Where `--browser` opens URLs that need no automation. */
export const BROWSER_MODES = ["default", "automated"];

export { executePlan, executePlans } from "./setup-batch.mjs";

export class SetupSession {
  constructor(plan, options) {
    this.plan = plan;
    this.options = options;
    this.conditions = new Set(["verify-release", "release-dispatch"]);
    if (plan.package.exists_on_registry === false) {
      this.conditions.add("package-missing");
    }
    if (plan.package.trusted_publishing !== true) {
      this.conditions.add("trust-missing");
    }
    this.values = {};
    this.automation = options.automation ?? null;
    this.temporary = null;
    this.shim = null;
  }

  async run() {
    for (const step of this.plan.steps) {
      if (CLEANUP_CONDITIONS.has(step.when)) {
        // cleanup() runs these, even after a failed step.
      } else if (!step.when || this.conditions.has(step.when)) {
        this.log(`==> ${step.title}`);
        await this.runStep(step);
      } else if (this.options.verbose) {
        console.error(`skip ${step.id}: condition ${step.when} does not hold`);
      }
    }
  }

  /**
   * Runs every cleanup step of the plan whose condition holds, including
   * those after a step that failed (#24).
   */
  async cleanup({ deferAuth = false, closeBrowser = true } = {}) {
    const deferred = [];
    for (const step of cleanupSteps(this.plan)) {
      if (
        deferAuth &&
        ["sign-out", "crates-sign-out"].includes(step.id) &&
        this.conditions.has(step.when)
      ) {
        if (step.id === "sign-out" && this.options.keepSession) {
          console.log(
            "Keeping the npm session (--keep-session); run npm logout to revoke it.",
          );
        } else {
          deferred.push(step);
        }
      } else if (step.id === "sign-out" && this.keepsSession(step)) {
        console.log(
          "Keeping the npm session (--keep-session): its token stays in npm's user configuration until you run npm logout, and the next run reuses it while npm whoami succeeds.",
        );
      } else if (this.conditions.has(step.when) && step.kind === "api") {
        this.log(`==> ${step.title}`);
        await runCratesApiStep(this, step).catch((error) =>
          console.error(`warning: ${step.id} failed: ${error.message}`),
        );
      } else if (this.conditions.has(step.when)) {
        this.log(`==> ${step.title}`);
        try {
          const result = await this.runProcess(step, true);
          if (result.code !== 0) {
            console.error(`warning: ${step.id} exited with ${result.code}`);
          }
        } catch (error) {
          console.error(`warning: ${step.id} failed: ${error.message}`);
        }
      }
    }
    if (this.automation && closeBrowser) {
      await this.automation.close();
    }
    for (const directory of [this.temporary, this.shim?.directory]) {
      if (directory) {
        await rm(directory, { recursive: true, force: true });
      }
    }
    this.shim = null;
    this.temporary = null;
    return deferred;
  }

  keepsSession(step) {
    return (
      Boolean(this.options.keepSession) &&
      this.conditions.has(step.when) &&
      step.command?.program === "npm"
    );
  }

  async runStep(step) {
    switch (step.kind) {
      case "check":
        if (step.id === "check-name-policy") {
          return checkNamePolicy(this.plan.package.name, this.options);
        }
        if (step.id === "check-registry") {
          return this.checkRegistry();
        }
        if (step.id === "verify-token-revoked") {
          return verifyTokenRevoked((message) => this.prompt(message), {
            ...this.options,
            env: this.options.env ?? process.env,
          });
        }
        return step.id === "verify-bins" ? this.verifyBins() : this.check(step);
      case "command":
        return step.id === "delete-token-secret"
          ? this.deleteTokenSecrets(step)
          : this.command(step);
      case "wait":
        return this.wait(step);
      case "browser":
        return this.browser(step);
      case "api":
        return step.id === "manage-registry-token"
          ? setupCredential(this)
          : runCratesApiStep(this, step);
      default:
        console.log(step.description);
        if (step.url) {
          console.log(`  ${step.url}`);
        }
        await this.prompt("Press Enter when this is done...");
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
        const trusted = result.code === 0 && listsTrustedPublisher(output);
        toggle(this.conditions, "trust-missing", !trusted);
        if (step.id === "verify-trusted-publisher" && !trusted) {
          throw new Error(
            "npm does not list a trusted publisher for the package",
          );
        }
        return;
      }
      case "check-2fa":
      case "verify-2fa":
        return this.checkTwoFactor(step, result);
      case "inspect-release-run":
        return this.inspectReleaseRun(result);
      case "read-release-failure":
        if (result.code !== 0) {
          console.error("warning: could not read the failed release log");
        } else if (PUBLISH_REJECTED.test(output)) {
          console.log(
            "  It failed at publish: npm rejected the workflow (E404/invalid-publisher) because no trusted publisher is attached yet. After attaching trust, its failed jobs can be re-run.",
          );
          this.conditions.add("release-failed-publish");
        } else {
          console.log(
            "  It failed for another reason, so trusted publishing alone will not fix it; it is not re-run.",
          );
        }
        return;
      case "audit-token-secrets":
        if (result.code !== 0) {
          console.error("warning: could not list repository secrets with gh");
          return;
        }
        this.tokenSecrets = reportTokenSecrets(output, this.plan.package);
        toggle(
          this.conditions,
          "token-secret-present",
          this.tokenSecrets.length > 0,
        );
        return;
      case "check-pages":
        return this.checkPages(result);
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

  checkTwoFactor(step, result) {
    let mode;
    try {
      mode = result.code === 0 ? twoFactorMode(result.stdout) : undefined;
    } catch {
      mode = undefined;
    }
    if (mode === undefined) {
      if (step.id === "verify-2fa") {
        throw new Error("could not read the npm profile to verify 2FA");
      }
      console.error(
        "warning: could not read the npm profile; npm trust requires two-factor authentication",
      );
      return;
    }
    if (mode) {
      console.log(`  Two-factor authentication is on (${mode}).`);
      this.conditions.delete("tfa-disabled");
      return;
    }
    if (step.id === "verify-2fa") {
      throw new Error(
        "two-factor authentication is still off; npm trust requires it, so nothing was published",
      );
    }
    console.log(
      "  Two-factor authentication is off; npm trust requires it, so turn it on before anything is published.",
    );
    this.conditions.add("tfa-disabled");
  }

  checkPages(result) {
    const state = pagesState(result);
    toggle(this.conditions, "pages-missing", state === "missing");
    toggle(this.conditions, "pages-legacy", state === "legacy");
    if (state === "workflow") {
      console.log(
        "  GitHub Pages is enabled with GitHub Actions as its source.",
      );
    } else if (state === "legacy") {
      console.log(
        "  GitHub Pages builds from a branch, so the workflow's deployment is not served.",
      );
    } else if (state === "missing") {
      console.log(
        "  GitHub Pages is not enabled, so the workflow's deployment fails with Not Found.",
      );
    } else {
      console.error(
        `warning: could not read the GitHub Pages site; check its source at ${this.pagesSettings()}`,
      );
    }
  }

  pagesSettings() {
    const { github_owner, github_repository } = this.plan.repository;
    return pagesSettingsUrl(`${github_owner}/${github_repository}`);
  }

  inspectReleaseRun(result) {
    if (result.code !== 0) {
      console.error("warning: could not list the release workflow runs");
      return;
    }
    const [run] = JSON.parse(String(result.stdout || "[]"));
    if (!run) {
      console.log("  The release workflow has not run yet.");
      return;
    }
    const outcome = run.conclusion || run.status;
    console.log(`  The latest release run ${outcome}: ${run.url}`);
    if (run.conclusion === "failure") {
      this.values.failed_run_id = String(run.databaseId);
      this.conditions.add("release-failed");
    }
  }

  async verifyBins() {
    const pack = this.plan.steps.find((item) => item.id === "pack");
    const source = await readManifest(
      path.join(this.cwd(pack), "package.json"),
    );
    const prefix = path.join(this.values.pack_destination, "install");
    const root = path.join(
      prefix,
      "node_modules",
      ...this.plan.package.name.split("/"),
    );
    // npm extracts the tarball's package/package.json unchanged.
    const packed = await readManifest(path.join(root, "package.json"));
    const removed = Object.keys(binEntries(source)).filter(
      (name) => !(name in binEntries(packed)),
    );
    if (removed.length > 0) {
      throw new Error(
        `the packed package.json has no bin ${removed.join(", ")}; npm removed it while packing, so correct the bin entries in package.json before the first publish`,
      );
    }
    const bins = Object.entries(binEntries(packed));
    if (bins.length === 0) {
      console.log("  The package has no bin entries.");
    }
    for (const [name, file] of bins) {
      // Windows links bins as .cmd shims, which cannot run without a shell.
      const command =
        process.platform === "win32"
          ? { program: "node", args: [path.join(root, file), "--version"] }
          : {
              program: path.join(prefix, "node_modules", ".bin", name),
              args: ["--version"],
            };
      if (this.options.verbose) {
        console.error(`+ (cd ${prefix} && ${render(command)})`);
      }
      const result = await exec(command.program, command.args, {
        cwd: prefix,
        capture: true,
        mirror: this.options.verbose,
        stdin: "ignore",
      });
      if (result.code !== 0) {
        if (!this.options.verbose) {
          stdout.write(String(result.stdout ?? ""));
          process.stderr.write(String(result.stderr ?? ""));
        }
        throw new Error(
          `bin ${name} (${file}) exited with status ${result.code} when run with --version from the installed tarball`,
        );
      }
      // A bin whose entry point never runs exits 0 without output.
      const output =
        String(result.stdout ?? "").trim() ||
        String(result.stderr ?? "").trim();
      if (!output) {
        throw new Error(
          `bin ${name} (${file}) printed nothing when run with --version from the installed tarball; make sure it runs when started through the node_modules/.bin symlink`,
        );
      }
      console.log(`  ${name} --version: ${output.split("\n")[0]}`);
    }
  }

  /** Deletes each leftover token secret, with one confirmation each. */
  async deleteTokenSecrets(step) {
    for (const name of this.tokenSecrets ?? []) {
      this.values.token_secret = name;
      await this.command(step);
    }
  }

  async command(step) {
    if (step.confirm && !this.options.yes) {
      const rendered = render(this.expand(step.command));
      const answer = await this.prompt(`Run \`${rendered}\`? [y/N] `);
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
    // Pack output is captured so npm's warnings can be reviewed.
    const result =
      step.id === "pack"
        ? await this.capture(step)
        : await this.runProcess(step, true);
    if (result.code !== 0) {
      if (
        step.id === "first-publish" &&
        policyRefusal(String(result.stderr) + String(result.stdout))
      ) {
        throw new Error(
          "npm refused this name under its policy; no retry. Choose @owner/name or a longer descriptive name.",
        );
      }
      if (step.id === "attach-trusted-publisher") {
        console.error(
          "warning: npm trust failed; falling back to the browser form",
        );
        this.conditions.add("trust-cli-failed");
        return;
      }
      if (PAGES_CHANGES.has(step.id)) {
        console.error(
          `warning: could not change GitHub Pages (this needs repository administrator rights); set its source to GitHub Actions at ${this.pagesSettings()}`,
        );
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
    if (step.id === "attach-trusted-publisher") {
      this.confirmTrustCreated(result.stdout);
    } else if (step.id === "sign-in") {
      this.conditions.add("tool-signed-in");
    } else if (step.id === "prepare-worktree") {
      this.conditions.add("worktree-created");
    } else if (step.id === "pack") {
      reportPackWarnings(result.stderr);
      this.recordPack(result.stdout);
    } else if (step.id === "rerun-release") {
      console.log("  Re-running the failed release jobs.");
      this.values.run_id = this.values.failed_run_id;
      this.conditions.delete("release-dispatch");
    }
  }

  /**
   * npm prints the stored configuration once `npm trust github` succeeds, so
   * that answer verifies it without a second 2FA-gated `npm trust list`.
   */
  confirmTrustCreated(output) {
    if (TRUST_CREATED.test(stripAnsi(output))) {
      console.log("  npm stored the trusted publisher.");
      this.conditions.delete("trust-missing");
    }
  }

  recordPack(output) {
    const packed = packedEntry(output);
    if (
      packed.name !== this.plan.package.name ||
      (this.plan.package.version &&
        packed.version !== this.plan.package.version)
    ) {
      throw new Error(
        "bootstrap ref package identity/version differs from inspection; inspect the selected ref again before publication",
      );
    }
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
    if (step.confirm && !this.options.yes) {
      console.log(`  ${step.description}`);
      const answer = await this.prompt(`${step.title}? [y/N] `);
      if (!/^(?:y|yes)$/i.test(answer)) {
        throw new Error(
          "the one-time first-publish token was declined; nothing was published",
        );
      }
    }
    // Only a form the tool fills needs the automated profile.
    const fill = PREFILLED_FORMS.has(step.id) && this.plan.trusted_publisher;
    await this.open(step.url, Boolean(fill));
    if (fill && this.automation) {
      await this.prefill();
    }
    await this.prompt("Finish this step in the browser, then press Enter...");
    if (step.id === "create-pending-publisher") {
      this.conditions.delete("trust-missing");
    }
  }

  async prefill() {
    await this.prompt("Press Enter when the form is visible...");
    const page = this.automation;
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
        await this.prompt(
          "Submit this trusted-publisher configuration? [y/N] ",
        ),
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

  async open(url, automate = false) {
    if (this.options.noBrowser) {
      console.log(`Open ${url}`);
      return;
    }
    if (!automate && this.options.browser !== "automated") {
      const app = this.options.openWith;
      console.log(`Opening ${url} in ${app ?? this.defaultBrowserLabel()}`);
      try {
        if (app) {
          const { program, args } = openWithCommand(url, app);
          await detachedRunner()(program, args);
        } else {
          await openInUserBrowser(url);
        }
      } catch (error) {
        console.error(
          `warning: could not open ${app ?? "your default browser"} (${error.message}); open the URL yourself`,
        );
      }
      return;
    }
    await (await this.automatedPage()).goto(url);
  }

  /**
   * The automated page, launched once. The crates.io sign-in step offers
   * `--browser-import default|auto` itself, once it knows a sign-in is missing.
   */
  async automatedPage(browser = this.options.browserOptions, quiet = false) {
    if (!this.automation) {
      const domains = this.options.domains ?? signInDomains(this.plan.registry);
      this.automation = await connectAutomation({
        browser: await launchBrowser(browser, domains, {
          deferred: this.plan.registry === "crates-io",
          verbose: quiet ? false : this.options.verbose,
        }),
        profile: this.options.browserProfile,
        verbose: quiet ? false : this.options.verbose,
        domains,
      });
    }
    return this.automation;
  }

  /** Relaunches the automated browser with other options, such as an import. */
  async reconnect(browser) {
    await this.automation?.close();
    this.automation = null;
    return this.automatedPage(browser);
  }

  /**
   * Names the default browser once per run, so maintainers know where to look.
   * Runs before npm starts: links are opened from a synchronous output callback.
   */
  async detectDefaultBrowser() {
    const { noBrowser, browser, openWith, verbose } = this.options;
    if (noBrowser || openWith || browser === "automated") {
      return;
    }
    this.browserName ??= (await detectDefaultBrowser({ verbose })) ?? "";
  }

  defaultBrowserLabel() {
    return this.browserName
      ? `${this.browserName}, your default browser`
      : "your default browser";
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
      !QUIET_CHECKS.has(step.id)
    ) {
      stdout.write(String(result.stdout ?? ""));
      process.stderr.write(String(result.stderr ?? ""));
    }
    return result;
  }

  /** Runs a step's command; `extra` is added to that child's environment only. */
  async runProcess(step, mirror, extra = undefined) {
    const { command, cwd } = this.prepare(step);
    let env = extra ? { ...process.env, ...extra } : process.env;
    const npm = ["npm", "npx"].includes(command.program);
    if (npm) {
      await this.detectDefaultBrowser();
      this.shim ??= await writeTtyShim();
      env = {
        ...process.env,
        NODE_OPTIONS: nodeOptionsWithShim(
          process.env.NODE_OPTIONS,
          this.shim.shim,
        ),
      };
    }
    const run = () =>
      runInteractive(command, {
        cwd,
        env,
        mirror,
        stopOnLegacyLogin: npm,
        watchStderr: npm,
        onUrl: (url, kind) => this.openLink(url, kind),
      });
    // npm's links expire after about 5 minutes; ask npm for a fresh one.
    return npm ? withFreshLinks(run) : run();
  }

  openLink(url, kind) {
    console.log(approvalDeadline(kind));
    if (kind === "approve") {
      console.log(TWO_FACTOR_HINT);
    }
    this.open(url).catch((error) =>
      console.error(`warning: could not open ${url}: ${error.message}`),
    );
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

  /** Asks on the terminal, or through `options.prompt` when given. */
  prompt(message) {
    return (this.options.prompt ?? prompt)(message);
  }
}

/** The plan's cleanup steps, which run after every other step, even a failed one. */
export function cleanupSteps(plan) {
  return plan.steps.filter((step) => CLEANUP_CONDITIONS.has(step.when));
}

/** Whether `npm trust list` output names a trusted publisher. */
export function listsTrustedPublisher(output) {
  return LISTED_PUBLISHER.test(stripAnsi(output));
}

function stripAnsi(text) {
  return String(text ?? "").replaceAll(ANSI, "");
}

/**
 * npm's warnings from `npm pack`, such as fields it auto-corrected or removed
 * from the packed package.json, without npm's advice to run its fixer, which
 * rewrites package.json in place.
 */
export function packWarnings(stderr) {
  return String(stderr ?? "")
    .split(/\r?\n/)
    .map((line) => line.replaceAll(ANSI, "").trim())
    .filter((line) => /^npm warn/i.test(line))
    .map((line) =>
      line.replace(/\s*Please run "npm pkg fix"[^.]*\.?/i, "").trim(),
    )
    .filter((line) => !/npm pkg fix/i.test(line));
}

/**
 * The first package in `npm pack --json` output: an array before npm 12, an
 * object keyed by package name since.
 */
export function packedEntry(output) {
  const parsed = JSON.parse(output);
  const [packed] = Array.isArray(parsed) ? parsed : Object.values(parsed ?? {});
  if (!packed) {
    throw new Error("npm pack printed no package");
  }
  return packed;
}

function reportPackWarnings(stderr) {
  const warnings = packWarnings(stderr);
  if (warnings.length === 0) {
    return;
  }
  console.log("  npm warned while packing:");
  for (const line of warnings) {
    console.log(`    ${line}`);
  }
  if (warnings.some((line) => /corrected|invalid and removed/i.test(line))) {
    console.log(
      "  npm changed the packed package.json; correct these fields in package.json itself.",
    );
  }
}

function binEntries(manifest) {
  if (typeof manifest.bin === "string") {
    return { [String(manifest.name).replace(/^@[^/]+\//, "")]: manifest.bin };
  }
  return manifest.bin && typeof manifest.bin === "object" ? manifest.bin : {};
}

async function readManifest(file) {
  return JSON.parse(await readFile(file, "utf8"));
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

/** Asks a question on the terminal; a closed stdin answers with "". */
export async function prompt(message) {
  const terminal = createInterface({ input: stdin, output: stdout });
  try {
    return await new Promise((resolve) => {
      terminal.once("close", () => resolve(""));
      terminal.question(message).then(resolve, () => resolve(""));
    });
  } finally {
    terminal.close();
  }
}

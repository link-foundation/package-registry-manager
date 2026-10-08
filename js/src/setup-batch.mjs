import { rm } from "node:fs/promises";
import { protectLegacyProfile } from "./profile.mjs";
import { SetupSession, prompt } from "./setup.mjs";
import { offerWorkflow } from "./workflow-proposal.mjs";
import { signInDomains } from "./sign-in-import.mjs";
import { oidcReleaseNote } from "./approvals.mjs";

/** Validates a plan before running any registry or repository mutations. */
export function validatePlan(plan, execute = true) {
  if (!plan.package.publishable) {
    throw new Error(
      `${plan.package.name} is not publishable: ${(plan.package.problems ?? []).join("; ")}`,
    );
  }
  if (
    execute &&
    plan.skipped_reason &&
    !plan.steps.some((step) => step.id === "add-publishing-workflow")
  ) {
    throw new Error(plan.skipped_reason);
  }
  const python = plan.prerequisites?.find(
    (item) => item.id === "python" && item.ok === false,
  );
  if (execute && python) {
    throw new Error(`${python.detected}; ${python.required}`);
  }
}

/** Runs one package, using the same executor as setup --all. */
export async function executePlan(plan, options) {
  return executePlans([plan], options, false);
}

/** Runs all plans with one browser session and defers sign-out until the end. */
export async function executePlans(plans, options, summary = true) {
  if (plans.length === 0) {
    throw new Error("no publishable package manifests were found");
  }
  for (const plan of plans) {
    validatePlan(plan, options.execute);
  }
  for (const plan of plans) {
    if (plan.mode === "complete") {
      console.log(
        `${plan.package.name} already publishes through trusted publishing; ${plan.steps.length ? "only the repository checks remain." : "nothing to do."}`,
      );
    }
  }
  if (
    plans.every((plan) => plan.mode === "complete" && plan.steps.length === 0)
  ) {
    const outcomes = plans.map((plan) => ({
      registry: plan.registry,
      package: plan.package.name,
      status: "complete",
    }));
    printSummary(outcomes, summary);
    return outcomes;
  }
  if (!options.execute) {
    console.log(
      "Dry run only. Re-run with --execute to run these steps and open the registry.",
    );
    return [];
  }
  const missing = plans.filter((plan) =>
    plan.steps.some((step) => step.id === "add-publishing-workflow"),
  );
  if (missing.length) {
    const result = await offerWorkflow(missing, {
      ...options,
      prompt: options.prompt ?? prompt,
    });
    const outcomes = plans.map((plan) => ({
      registry: plan.registry,
      package: plan.package.name,
      status: missing.includes(plan) ? result.status : "blocked",
      ...(result.url ? { url: result.url } : {}),
    }));
    printSummary(outcomes, summary);
    return outcomes;
  }
  if (options.browserProfile) {
    await protectLegacyProfile(
      options.repository,
      options.browserProfile,
      options,
    );
  }
  const domains = [
    ...new Set(plans.flatMap((plan) => signInDomains(plan.registry))),
  ];
  let automation = options.automation ?? null;
  const deferred = new Map();
  const outcomes = [];
  let failure;
  try {
    for (const plan of plans) {
      const session = new SetupSession(plan, {
        ...options,
        automation,
        domains,
      });
      try {
        await session.run();
        outcomes.push({
          registry: plan.registry,
          package: plan.package.name,
          status: plan.steps.length === 0 ? "complete" : "configured",
          ...session.outcome,
        });
        if (plan.trusted_publisher?.workflow) {
          console.log(`\n${oidcReleaseNote(plan.trusted_publisher.workflow)}`);
        }
      } catch (error) {
        outcomes.push({
          registry: plan.registry,
          package: plan.package.name,
          status: "failed",
        });
        failure = error;
      } finally {
        const steps = await session.cleanup({
          deferAuth: plans.length > 1,
          closeBrowser: false,
        });
        automation = session.automation;
        for (const step of steps) {
          deferred.set(`${plan.registry}:${step.id}`, { session, step });
        }
      }
      if (failure) {
        break;
      }
    }
  } finally {
    for (const { session, step } of deferred.values()) {
      session.automation = automation;
      try {
        await session.runStep(step);
      } catch (error) {
        console.error(`warning: ${step.id} failed: ${error.message}`);
      } finally {
        if (session.shim?.directory) {
          await rm(session.shim.directory, { recursive: true, force: true });
        }
      }
    }
    await automation?.close();
    for (const plan of plans.slice(outcomes.length)) {
      outcomes.push({
        registry: plan.registry,
        package: plan.package.name,
        status: "not-run",
      });
    }
    printSummary(outcomes, summary);
  }
  if (failure) {
    throw failure;
  }
  return outcomes;
}

function printSummary(outcomes, enabled) {
  if (enabled) {
    console.log("\nSetup summary:");
    for (const item of outcomes) {
      console.log(
        `- ${item.registry}: ${item.package}: ${item.status}${item.url ? ` (${item.url})` : ""}`,
      );
    }
  }
}

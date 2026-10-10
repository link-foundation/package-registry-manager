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
  const ready = [];
  let blocked;
  for (const plan of plans) {
    try {
      validatePlan(plan, options.execute);
      ready.push(plan);
    } catch (error) {
      if (!options.accountScan) {
        throw error;
      }
      blocked ??= error;
      console.error(
        `${plan.repository.github_owner}/${plan.repository.github_repository}: ${plan.registry}: ${plan.package.name}: blocked: ${error.message}`,
      );
    }
  }
  if (blocked) {
    if (ready.length) {
      await executePlans(ready, options, summary);
    }
    throw blocked;
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
    if (options.accountScan) {
      const outcomes = [];
      for (const root of new Set(missing.map((plan) => plan.repository.root))) {
        const group = missing.filter((plan) => plan.repository.root === root);
        const result = await (options.offerWorkflow ?? offerWorkflow)(group, {
          ...options,
          repository: root,
          inspection: undefined,
          prompt: options.prompt ?? prompt,
        });
        outcomes.push(
          ...group.map((plan) => ({
            registry: plan.registry,
            package: plan.package.name,
            repository: `${plan.repository.github_owner}/${plan.repository.github_repository}`,
            ...result,
          })),
        );
      }
      const ready = plans.filter((plan) => !missing.includes(plan));
      if (ready.length) {
        outcomes.push(...(await executePlans(ready, options, false)));
      }
      printSummary(outcomes, summary);
      return outcomes;
    }
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
        repository: options.accountScan
          ? plan.repository.root
          : options.repository,
        automation,
        domains,
      });
      try {
        await session.run();
        outcomes.push({
          registry: plan.registry,
          package: plan.package.name,
          repository: `${plan.repository.github_owner}/${plan.repository.github_repository}`,
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
        failure ??= error;
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
      if (failure && !options.accountScan) {
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
        `- ${item.repository ? `${item.repository}: ` : ""}${item.registry}: ${item.package}: ${item.status}${item.secret_scope ? `; ${item.secret_scope} secret` : ""}${item.fallback_reason ? `; ${item.fallback_reason}` : ""}${item.url ? ` (${item.url})` : ""}`,
      );
    }
  }
}

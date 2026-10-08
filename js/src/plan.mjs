import {
  BOOTSTRAP_CONDITIONS,
  cratesFlow,
  dockerHubFlow,
  ghcrFlow,
  npmFlow,
  pypiFlow,
} from "./flows.mjs";
import { credentialPolicy } from "./credential-cycle.mjs";
import { trustedFlow, tokenFlow } from "./extra-flows.mjs";
import { bootstrapRef } from "./bootstrap-reference.mjs";
import { pagesSteps } from "./pages.mjs";
import { planPrerequisites } from "./prerequisites.mjs";
import { TRUSTED_REGISTRIES } from "./publishers.mjs";
import { applyPython } from "./python.mjs";
import { repairSteps } from "./repository-repair.mjs";

const FLOWS = new Map([
  ["npm", npmFlow],
  ["crates-io", cratesFlow],
  ["pypi", pypiFlow],
  ["docker-hub", dockerHubFlow],
  ["ghcr", ghcrFlow],
  ["rubygems", trustedFlow],
  ["nuget", trustedFlow],
  ["jsr", trustedFlow],
]);

/**
 * Builds one setup plan per package. `options.verifyRelease` appends the
 * release-verification steps to flows that support them. With
 * `options.environment` (from `probeEnvironment`) npm trust runs through the
 * npm that the local Node.js supports, and each plan lists its prerequisites;
 * `options.browser` describes where browser pages open. `options.workflow`
 * and `options.publisherEnvironment` override the detected trusted-publisher
 * workflow file and GitHub environment.
 */
export function buildPlans(inspection, selected = [], options = {}) {
  const registries = new Set(selected);
  return inspection.packages
    .filter((item) => registries.size === 0 || registries.has(item.registry))
    .map((item) => buildPlan(inspection, item, options));
}

function buildPlan(inspection, packageInfo, options) {
  if (!packageInfo.publishable) {
    return {
      ...basePlan(inspection, packageInfo, []),
      skipped_reason: skippedReason(packageInfo),
    };
  }
  const command = (program, args) => ({ program, args });
  const check = (id, title, description, commandSpec) => ({
    id,
    title,
    kind: "check",
    description,
    command: commandSpec,
  });
  const browser = (id, title, description, url) => ({
    id,
    title,
    kind: "browser",
    description,
    url,
  });
  let steps;

  switch (packageInfo.registry) {
    case "npm":
    case "crates-io":
    case "pypi":
    case "docker-hub":
    case "ghcr":
    case "rubygems":
    case "nuget":
    case "jsr":
      return flowPlan(inspection, packageInfo, options);
    case "go-modules":
      steps = [
        check(
          "test-module",
          "Test the Go module",
          "Run all module tests before tagging a semantic version.",
          command("go", ["test", "./..."]),
        ),
        {
          id: "publish-tag",
          title: "Push a semantic-version tag",
          kind: "manual",
          description:
            "Go modules are published from repository tags; after pushing the tag, request it through proxy.golang.org.",
          url: "https://go.dev/ref/mod#publishing-a-module",
        },
      ];
      break;
    case "maven-central":
    case "vscode-marketplace":
    case "open-vsx":
    case "chrome-web-store":
      if (!packageInfo.workflow && !options.workflow) {
        return {
          ...basePlan(inspection, packageInfo, []),
          skipped_reason: `no workflow publishes ${packageInfo.name} to ${packageInfo.registry}; pass --workflow <file> for an explicit setup target`,
        };
      }
      steps = tokenFlow(packageInfo);
      break;
    case "packagist":
      steps = [
        check(
          "validate-package",
          "Validate Composer metadata",
          "Strictly validate composer.json before submitting it.",
          command("composer", ["validate", "--strict"]),
        ),
        browser(
          "submit-repository",
          "Submit the repository to Packagist",
          "Sign in and submit the public VCS repository URL. Packagist reads package versions from tags.",
          "https://packagist.org/packages/submit",
        ),
      ];
      break;
    default:
      throw new Error(`no setup plan for registry ${packageInfo.registry}`);
  }

  return basePlan(inspection, packageInfo, steps);
}

function basePlan(inspection, packageInfo, steps) {
  return {
    schema_version: 1,
    registry: packageInfo.registry,
    credential_policy: credentialPolicy(packageInfo.registry),
    package: structuredClone(packageInfo),
    repository: structuredClone(inspection.repository),
    steps,
  };
}

function flowPlan(inspection, packageInfo, options) {
  const { github_owner, github_repository } = inspection.repository;
  const slug =
    github_owner && github_repository
      ? `${github_owner}/${github_repository}`
      : null;
  const trusted = TRUSTED_REGISTRIES.includes(packageInfo.registry);
  const workflow =
    (trusted ? options.workflow : null) ?? packageInfo.workflow ?? null;
  const environment = trusted
    ? (options.publisherEnvironment ?? packageInfo.environment ?? null)
    : null;
  if (trusted && !workflow && packageInfo.workflow_candidates?.length > 1) {
    return {
      ...basePlan(inspection, packageInfo, []),
      skipped_reason: `several workflows publish to ${packageInfo.registry} (${packageInfo.workflow_candidates.join(", ")}); pass --workflow <file> to choose the trusted publisher`,
    };
  }
  if (
    trusted &&
    (!workflow ||
      (options.addPublishJob &&
        !packageInfo.workflow &&
        !packageInfo.workflow_candidates?.length))
  ) {
    return {
      ...basePlan(inspection, packageInfo, [
        {
          id: "add-publishing-workflow",
          title: "Offer a publishing job in a reviewed pull request",
          kind: "manual",
          description:
            "Add the missing publishing job to the workflow used by the other registries, on a new branch and pull request. Review and merge it, then re-run setup to attach the trusted publisher.",
          confirm: true,
        },
      ]),
      skipped_reason: `no workflow publishes ${packageInfo.name} to ${packageInfo.registry}; CI releases will not reach it; add a publishing job or pass --workflow <file> before attaching a trusted publisher`,
    };
  }
  const context = {
    directory: packageDirectory(packageInfo.manifest),
    slug,
    workflow,
    environment,
    verifyRelease: Boolean(options.verifyRelease),
    trustNpm: options.environment?.trustNpm,
    manual: Boolean(options.manual),
  };
  const mode = planMode(packageInfo);
  let steps = FLOWS.get(packageInfo.registry)(packageInfo, context);
  if (mode === "complete") {
    steps = [];
  } else if (mode === "attach") {
    steps = steps.filter((item) => !BOOTSTRAP_CONDITIONS.has(item.when));
  } else if (mode === "repair") {
    steps = repairSteps(
      packageInfo,
      context,
      packageInfo.exists_on_registry !== true
        ? steps
        : steps.filter((item) => !BOOTSTRAP_CONDITIONS.has(item.when)),
    );
  }
  if (options.ref) {
    const ref = bootstrapRef(options.ref);
    const fetch = steps.find((step) => step.id === "fetch-default-branch");
    if (fetch) {
      fetch.command.args = ["fetch", "origin", ref];
      fetch.title = `Fetch bootstrap ref ${ref}`;
    }
  }
  // Pages readiness belongs to the repository, so it is checked in every mode.
  const pages = inspection.repository.pages_workflow;
  if (slug && pages) {
    steps = steps.concat(pagesSteps(slug, pages));
  }
  const plan = basePlan(inspection, packageInfo, steps);
  if (mode) {
    plan.mode = mode;
  }
  const prerequisites = planPrerequisites(
    plan,
    options.environment,
    options.browser,
  );
  if (prerequisites.length > 0) {
    plan.prerequisites = prerequisites;
  }
  if (slug && workflow && trusted) {
    plan.trusted_publisher = {
      provider: "github-actions",
      organization: github_owner,
      repository: github_repository,
      workflow,
    };
    if (environment) {
      plan.trusted_publisher.environment = environment;
    }
    if (packageInfo.registry === "pypi") {
      plan.trusted_publisher.project = packageInfo.name;
    }
  }
  applyPython(plan, options.environment?.python);
  return plan;
}

/**
 * Chooses `bootstrap` for a package missing from its registry, `attach` for
 * one without trusted publishing, `repair` for stale or unverified repository
 * identities, and `complete` when nothing is left to do.
 * Unknown registry state leaves the mode unset.
 */
export function planMode(packageInfo) {
  if (
    packageInfo.repository_mismatches?.length ||
    packageInfo.publisher_settings_verified === false
  ) {
    return "repair";
  }
  if (packageInfo.exists_on_registry === false) {
    return "bootstrap";
  }
  if (packageInfo.exists_on_registry !== true) {
    return null;
  }
  return packageInfo.trusted_publishing === true ? "complete" : "attach";
}

export function skippedReason(packageInfo) {
  const problems = packageInfo.problems ?? [];
  return problems.length > 0
    ? problems.join("; ")
    : "the manifest marks this package as not publishable";
}

export function packageDirectory(manifest) {
  const separator = manifest.lastIndexOf("/");
  return separator === -1 ? "." : manifest.slice(0, separator);
}

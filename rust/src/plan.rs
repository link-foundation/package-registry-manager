use std::collections::BTreeSet;

use crate::flows::{
    crates_flow, docker_hub_flow, ghcr_flow, npm_flow, pypi_flow, FlowContext, BOOTSTRAP_CONDITIONS,
};
use crate::model::{
    Inspection, Package, PlanMode, Registry, SetupPlan, SetupStep, StepKind,
    TrustedPublisherPrefill,
};
use crate::pages::pages_steps;
use crate::prerequisites::{plan_prerequisites, BrowserDisplay, Environment};
use crate::publishers::TRUSTED_REGISTRIES;
use crate::registry_state::Endpoints;

/// Options that shape setup plans.
#[derive(Debug, Clone, Default)]
pub struct PlanOptions {
    /// Append release-verification steps to flows that support them.
    pub verify_release: bool,
    /// Registry API base URLs used in lookup steps.
    pub endpoints: Endpoints,
    /// The probed local tools (see `probe_environment`); with it npm trust runs
    /// through the npm the Node.js on `PATH` supports, and plans list their
    /// manual prerequisites.
    pub environment: Option<Environment>,
    /// Where browser pages open, for the prerequisites.
    pub browser: BrowserDisplay,
    /// Trusted-publisher workflow file that overrides the detected one.
    pub workflow: Option<String>,
    /// GitHub environment of the trusted publisher, overriding the detected one.
    pub publisher_environment: Option<String>,
    /// crates.io: keep the manual first-publish token checklist instead of
    /// the crates.io API (`--manual`).
    pub manual: bool,
}

/// Build setup plans for every publishable package found during inspection.
#[must_use]
pub fn build_plans(inspection: &Inspection) -> Vec<SetupPlan> {
    build_plans_for(inspection, &BTreeSet::new())
}

/// Build setup plans, optionally restricted to selected registries.
#[must_use]
pub fn build_plans_for(inspection: &Inspection, selected: &BTreeSet<Registry>) -> Vec<SetupPlan> {
    build_plans_with(
        inspection,
        selected,
        &PlanOptions {
            endpoints: Endpoints::from_env(),
            ..PlanOptions::default()
        },
    )
}

/// Build setup plans with explicit options.
#[must_use]
pub fn build_plans_with(
    inspection: &Inspection,
    selected: &BTreeSet<Registry>,
    options: &PlanOptions,
) -> Vec<SetupPlan> {
    inspection
        .packages
        .iter()
        .filter(|package| selected.is_empty() || selected.contains(&package.registry))
        .map(|package| build_plan(inspection, package, options))
        .collect()
}

fn base_plan(inspection: &Inspection, package: &Package, steps: Vec<SetupStep>) -> SetupPlan {
    SetupPlan {
        schema_version: 1,
        registry: package.registry,
        package: package.clone(),
        repository: inspection.repository.clone(),
        steps,
        mode: None,
        prerequisites: Vec::new(),
        oidc_publisher: None,
        skipped_reason: None,
    }
}

fn build_plan(inspection: &Inspection, package: &Package, options: &PlanOptions) -> SetupPlan {
    if !package.publishable {
        return SetupPlan {
            skipped_reason: Some(skipped_reason(package)),
            ..base_plan(inspection, package, Vec::new())
        };
    }
    let check = |id: &str, title: &str, description: &str, program: &str, args: &[&str]| {
        SetupStep::new(id, title, StepKind::Check, description).command(program, args)
    };
    let browser = |id: &str, title: &str, description: &str, url: &str| {
        SetupStep::new(id, title, StepKind::Browser, description).url(url)
    };

    let steps = match package.registry {
        Registry::Npm
        | Registry::CratesIo
        | Registry::PyPi
        | Registry::DockerHub
        | Registry::Ghcr => return flow_plan(inspection, package, options),
        Registry::GoModules => vec![
                check(
                    "test-module",
                    "Test the Go module",
                    "Run all module tests before tagging a semantic version.",
                    "go",
                    &["test", "./..."],
                ),
                SetupStep::new(
                    "publish-tag",
                    "Push a semantic-version tag",
                    StepKind::Manual,
                    "Go modules are published from repository tags; after pushing the tag, request it through proxy.golang.org.",
                )
                .url("https://go.dev/ref/mod#publishing-a-module"),
        ],
        Registry::NuGet => vec![
                check(
                    "pack-package",
                    "Build the NuGet package",
                    "Create the package locally without pushing it.",
                    "dotnet",
                    &["pack", "--configuration", "Release"],
                ),
                browser(
                    "configure-trusted-publishing",
                    "Configure NuGet trusted publishing",
                    "Sign in and add a GitHub Actions federated credential for this package.",
                    "https://www.nuget.org/account/TrustedPublishing"
                ),
        ],
        Registry::MavenCentral => vec![
                check(
                    "verify-build",
                    "Verify the Maven build",
                    "Run the build lifecycle without deploying an artifact.",
                    "mvn",
                    &["--batch-mode", "verify"],
                ),
                browser(
                    "verify-namespace",
                    "Verify a Central namespace",
                    "Sign in to the Central Portal and verify the namespace used by the package coordinates.",
                    "https://central.sonatype.com/publishing/namespaces"
                ),
        ],
        Registry::Packagist => vec![
                check(
                    "validate-package",
                    "Validate Composer metadata",
                    "Strictly validate composer.json before submitting it.",
                    "composer",
                    &["validate", "--strict"],
                ),
                browser(
                    "submit-repository",
                    "Submit the repository to Packagist",
                    "Sign in and submit the public VCS repository URL. Packagist reads package versions from tags.",
                    "https://packagist.org/packages/submit"
                ),
        ],
    };

    base_plan(inspection, package, steps)
}

fn flow_plan(inspection: &Inspection, package: &Package, options: &PlanOptions) -> SetupPlan {
    let repository = &inspection.repository;
    let owner_repo = repository
        .github_owner
        .as_deref()
        .zip(repository.github_repository.as_deref());
    let trusted = TRUSTED_REGISTRIES.contains(&package.registry);
    let workflow = options
        .workflow
        .clone()
        .filter(|_| trusted)
        .or_else(|| package.workflow.clone());
    let environment = if trusted {
        options
            .publisher_environment
            .clone()
            .or_else(|| package.environment.clone())
    } else {
        None
    };
    if trusted && workflow.is_none() && package.workflow_candidates.len() > 1 {
        return SetupPlan {
            skipped_reason: Some(format!(
                "several workflows publish to {} ({}); pass --workflow <file> to choose the trusted publisher",
                package.registry,
                package.workflow_candidates.join(", ")
            )),
            ..base_plan(inspection, package, Vec::new())
        };
    }
    let context = FlowContext {
        directory: package_directory(&package.manifest),
        slug: owner_repo.map(|(owner, name)| format!("{owner}/{name}")),
        workflow: workflow.clone(),
        environment: environment.clone(),
        verify_release: options.verify_release,
        endpoints: &options.endpoints,
        trust_npm: options
            .environment
            .as_ref()
            .map(|environment| environment.trust_npm.as_str()),
        manual: options.manual,
    };
    let flow = match package.registry {
        Registry::Npm => npm_flow,
        Registry::CratesIo => crates_flow,
        Registry::PyPi => pypi_flow,
        Registry::DockerHub => docker_hub_flow,
        _ => ghcr_flow,
    };
    let mode = plan_mode(package);
    let mut steps = match mode {
        Some(PlanMode::Complete) => Vec::new(),
        Some(PlanMode::Attach) => flow(package, &context)
            .into_iter()
            .filter(|step| {
                step.when
                    .as_deref()
                    .is_none_or(|when| !BOOTSTRAP_CONDITIONS.contains(&when))
            })
            .collect(),
        _ => flow(package, &context),
    };
    // Pages readiness belongs to the repository, so it is checked in every mode.
    if let (Some(slug), Some(pages)) = (&context.slug, &repository.pages_workflow) {
        steps.extend(pages_steps(slug, pages));
    }
    let mut plan = base_plan(inspection, package, steps);
    plan.mode = mode;
    plan.prerequisites = plan_prerequisites(&plan, options.environment.as_ref(), &options.browser);
    if let (Some((owner, name)), Some(workflow), true) = (owner_repo, workflow, trusted) {
        plan.oidc_publisher = Some(TrustedPublisherPrefill {
            provider: "github-actions".to_owned(),
            organization: owner.to_owned(),
            repository: name.to_owned(),
            workflow,
            environment,
            project: (package.registry == Registry::PyPi).then(|| package.name.clone()),
        });
    }
    plan
}

/// Choose `bootstrap` for a package missing from its registry, `attach` for one
/// without trusted publishing, and `complete` when nothing is left to do.
/// Unknown registry state leaves the mode unset.
#[must_use]
pub const fn plan_mode(package: &Package) -> Option<PlanMode> {
    match (package.exists_on_registry, package.trusted_publishing) {
        (Some(false), _) => Some(PlanMode::Bootstrap),
        (Some(true), Some(true)) => Some(PlanMode::Complete),
        (Some(true), _) => Some(PlanMode::Attach),
        (None, _) => None,
    }
}

/// Explain why a package that must not be published has no setup steps.
#[must_use]
pub fn skipped_reason(package: &Package) -> String {
    if package.problems.is_empty() {
        "the manifest marks this package as not publishable".to_owned()
    } else {
        package.problems.join("; ")
    }
}

/// Return the directory of a manifest relative to the repository root, or `.`.
#[must_use]
pub fn package_directory(manifest: &str) -> &str {
    manifest
        .rsplit_once('/')
        .map_or(".", |(directory, _)| directory)
}

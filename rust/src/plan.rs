use std::collections::BTreeSet;

use crate::model::{
    CommandSpec, Inspection, Package, Registry, SetupPlan, SetupStep, StepKind,
    TrustedPublisherPrefill,
};

/// Build setup plans for every publishable package found during inspection.
#[must_use]
pub fn build_plans(inspection: &Inspection) -> Vec<SetupPlan> {
    build_plans_for(inspection, &BTreeSet::new())
}

/// Build setup plans, optionally restricted to selected registries.
#[must_use]
pub fn build_plans_for(inspection: &Inspection, selected: &BTreeSet<Registry>) -> Vec<SetupPlan> {
    inspection
        .packages
        .iter()
        .filter(|package| selected.is_empty() || selected.contains(&package.registry))
        .map(|package| build_plan(inspection, package))
        .collect()
}

fn build_plan(inspection: &Inspection, package: &Package) -> SetupPlan {
    let directory = package_directory(&package.manifest);
    let command = |program: &str, args: &[&str]| CommandSpec {
        program: program.to_owned(),
        args: args.iter().map(|value| (*value).to_owned()).collect(),
    };
    let check = |id: &str, title: &str, description: &str, command: CommandSpec| SetupStep {
        id: id.to_owned(),
        title: title.to_owned(),
        kind: StepKind::Check,
        description: description.to_owned(),
        command: Some(command),
        url: None,
    };
    let browser = |id: &str, title: &str, description: &str, url: String| SetupStep {
        id: id.to_owned(),
        title: title.to_owned(),
        kind: StepKind::Browser,
        description: description.to_owned(),
        command: None,
        url: Some(url),
    };

    let (steps, trusted_publisher) = match package.registry {
        Registry::Npm => {
            let mut steps = vec![check(
                "validate-package",
                "Validate npm metadata",
                "Read the package metadata with npm before changing registry settings.",
                command("npm", &["pkg", "get", "name", "version", "repository"]),
            )];
            let url = format!(
                "https://www.npmjs.com/package/{}/access",
                url_path_segment(&package.name)
            );
            steps.push(browser(
                "configure-trusted-publisher",
                "Configure npm trusted publishing",
                "Sign in in the isolated browser profile, review the prefilled GitHub Actions identity, and explicitly confirm submission.",
                url,
            ));
            let publisher = match (
                &inspection.repository.github_owner,
                &inspection.repository.github_repository,
                &inspection.repository.release_workflow,
            ) {
                (Some(owner), Some(repository), Some(workflow)) => {
                    Some(TrustedPublisherPrefill {
                        provider: "github-actions".to_owned(),
                        organization: owner.clone(),
                        repository: repository.clone(),
                        workflow: workflow.clone(),
                        environment: None,
                    })
                }
                _ => None,
            };
            (steps, publisher)
        }
        Registry::CratesIo => (
            vec![
                check(
                    "validate-package",
                    "Validate the crate",
                    "Package the crate without uploading it.",
                    command("cargo", &["publish", "--dry-run"]),
                ),
                browser(
                    "review-account",
                    "Review crates.io account settings",
                    "Sign in with GitHub and review API-token or trusted-publishing settings. The tool never creates or prints a token.",
                    "https://crates.io/settings/tokens".to_owned(),
                ),
            ],
            None,
        ),
        Registry::PyPi => (
            vec![
                check(
                    "build-package",
                    "Build the Python distribution",
                    "Build source and wheel distributions locally.",
                    command("python", &["-m", "build"]),
                ),
                browser(
                    "configure-trusted-publisher",
                    "Configure a PyPI trusted publisher",
                    "Sign in and add the repository's GitHub Actions workflow as a trusted publisher.",
                    format!(
                        "https://pypi.org/manage/project/{}/settings/publishing/",
                        url_path_segment(&package.name)
                    ),
                ),
            ],
            None,
        ),
        Registry::GoModules => (
            vec![
                check(
                    "test-module",
                    "Test the Go module",
                    "Run all module tests before tagging a semantic version.",
                    command("go", &["test", "./..."]),
                ),
                SetupStep {
                    id: "publish-tag".to_owned(),
                    title: "Push a semantic-version tag".to_owned(),
                    kind: StepKind::Manual,
                    description: "Go modules are published from repository tags; after pushing the tag, request it through proxy.golang.org."
                        .to_owned(),
                    command: None,
                    url: Some("https://go.dev/ref/mod#publishing-a-module".to_owned()),
                },
            ],
            None,
        ),
        Registry::NuGet => (
            vec![
                check(
                    "pack-package",
                    "Build the NuGet package",
                    "Create the package locally without pushing it.",
                    command("dotnet", &["pack", "--configuration", "Release"]),
                ),
                browser(
                    "configure-trusted-publishing",
                    "Configure NuGet trusted publishing",
                    "Sign in and add a GitHub Actions federated credential for this package.",
                    "https://www.nuget.org/account/TrustedPublishing".to_owned(),
                ),
            ],
            None,
        ),
        Registry::MavenCentral => (
            vec![
                check(
                    "verify-build",
                    "Verify the Maven build",
                    "Run the build lifecycle without deploying an artifact.",
                    command("mvn", &["--batch-mode", "verify"]),
                ),
                browser(
                    "verify-namespace",
                    "Verify a Central namespace",
                    "Sign in to the Central Portal and verify the namespace used by the package coordinates.",
                    "https://central.sonatype.com/publishing/namespaces".to_owned(),
                ),
            ],
            None,
        ),
        Registry::Packagist => (
            vec![
                check(
                    "validate-package",
                    "Validate Composer metadata",
                    "Strictly validate composer.json before submitting it.",
                    command("composer", &["validate", "--strict"]),
                ),
                browser(
                    "submit-repository",
                    "Submit the repository to Packagist",
                    "Sign in and submit the public VCS repository URL. Packagist reads package versions from tags.",
                    "https://packagist.org/packages/submit".to_owned(),
                ),
            ],
            None,
        ),
    };

    let mut package = package.clone();
    if directory != "." {
        for step in &mut package.problems {
            *step = format!("{step} (manifest directory: {directory})");
        }
    }
    SetupPlan {
        schema_version: 1,
        registry: package.registry,
        package,
        repository: inspection.repository.clone(),
        steps,
        trusted_publisher,
    }
}

#[must_use]
pub fn package_directory(manifest: &str) -> &str {
    manifest
        .rsplit_once('/')
        .map_or(".", |(directory, _)| directory)
}

fn url_path_segment(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'@') {
                vec![char::from(byte)].into_iter()
            } else {
                format!("%{byte:02X}")
                    .chars()
                    .collect::<Vec<_>>()
                    .into_iter()
            }
        })
        .collect()
}

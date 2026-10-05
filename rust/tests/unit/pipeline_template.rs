use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use package_registry_manager::{
    build_plans_for, inspect_repository, inspect_repository_with, InspectOptions, Inspection,
    Package, PlanMode, Registry, SetupPlan, Skipped,
};
use tempfile::TempDir;

pub fn fixture() -> (TempDir, PathBuf) {
    let temporary = TempDir::new().expect("create temporary repository");
    let root = temporary.path().join("pipeline-template");
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/pipeline-template"),
        &root,
    );
    fs::create_dir_all(root.join(".git")).expect("create fixture git directory");
    fs::write(
        root.join(".git/config"),
        "[remote \"origin\"]\n\turl = https://github.com/acme/pipeline-app.git\n",
    )
    .expect("write fixture git config");
    (temporary, root)
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create fixture directory");
    for entry in fs::read_dir(source).expect("read fixture directory") {
        let entry = entry.expect("read fixture entry");
        let target = destination.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("copy fixture file");
        }
    }
}

#[test]
fn skips_the_example_app_listing_it_only_when_asked() {
    let (_temporary, root) = fixture();
    let inspection = inspect_repository(&root).expect("inspect fixture");
    assert_eq!(inspection.skipped, [] as [Skipped; 0]);
    assert!(inspection
        .packages
        .iter()
        .all(|package| !package.manifest.starts_with("examples/")));
    let verbose = inspect_repository_with(
        &root,
        InspectOptions {
            include_skipped: true,
        },
    )
    .expect("inspect fixture");
    assert_eq!(
        verbose.skipped,
        [Skipped {
            manifest: "examples/universal-app/package.json".to_owned(),
            reason: "under examples/, a test or example directory, and no workflow publishes it"
                .to_owned(),
        }]
    );
    assert_eq!(verbose.packages, inspection.packages);
}

#[test]
fn plans_no_actionable_steps_for_unpublishable_packages() {
    let (_temporary, root) = fixture();
    let mut inspection = inspect_repository(&root).expect("inspect fixture");
    inspection.packages.insert(
        0,
        Package::new(
            Registry::Npm,
            "universal-example-app".to_owned(),
            Some("0.0.0".to_owned()),
            "apps/universal-app/package.json".to_owned(),
        )
        .unpublishable("package.json marks this package as private"),
    );
    let plans = build_plans_for(&inspection, &BTreeSet::from([Registry::Npm]));
    let private = plans
        .iter()
        .find(|plan| plan.package.name == "universal-example-app")
        .expect("private plan");
    assert!(!private.package.publishable);
    assert_eq!(
        private.steps,
        [] as [package_registry_manager::SetupStep; 0]
    );
    assert!(private.oidc_publisher.is_none());
    assert_eq!(
        private.skipped_reason.as_deref(),
        Some("package.json marks this package as private")
    );

    let public = plans
        .iter()
        .find(|plan| plan.package.name == "pipeline-app")
        .expect("public plan");
    assert!(public.skipped_reason.is_none());
    assert_ne!(public.steps, [] as [package_registry_manager::SetupStep; 0]);
    assert_eq!(
        public
            .oidc_publisher
            .as_ref()
            .map(|publisher| publisher.workflow.as_str()),
        Some("release.yml")
    );
}

#[test]
fn detects_docker_hub_and_ghcr_images_from_the_dockerfile_and_release_workflow() {
    let (_temporary, root) = fixture();
    let inspection = inspect_repository(&root).expect("inspect fixture");
    let mut actual = serde_json::to_value(&inspection).expect("serialize inspection");
    actual["repository"]["root"] = serde_json::Value::String("<ROOT>".to_owned());
    let expected: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(root.join("expected-inspection.json"))
            .expect("read shared expected inspection"),
    )
    .expect("parse shared expected inspection");
    assert_eq!(actual, expected, "Rust must honor the shared contract");
    let containers = inspection
        .packages
        .iter()
        .filter(|package| matches!(package.registry, Registry::DockerHub | Registry::Ghcr))
        .map(|package| {
            (
                package.registry,
                package.name.as_str(),
                package.manifest.as_str(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        containers,
        [
            (Registry::DockerHub, "acme/pipeline-app", "Dockerfile"),
            (Registry::Ghcr, "acme/pipeline-app", "Dockerfile"),
        ]
    );
}

fn argv(plan: &SetupPlan, id: &str) -> Vec<String> {
    let command = plan
        .steps
        .iter()
        .find(|step| step.id == id)
        .and_then(|step| step.command.as_ref())
        .unwrap_or_else(|| panic!("{id} has a command"));
    std::iter::once(command.program.clone())
        .chain(command.args.iter().cloned())
        .collect()
}

fn only_plan(inspection: &Inspection, registry: Registry) -> SetupPlan {
    let mut plans = build_plans_for(inspection, &BTreeSet::from([registry]));
    assert_eq!(plans.len(), 1);
    plans.remove(0)
}

#[test]
fn plans_docker_hub_repository_token_variables_and_secret() {
    let (_temporary, root) = fixture();
    let mut inspection = inspect_repository(&root).expect("inspect fixture");
    let plan = only_plan(&inspection, Registry::DockerHub);
    assert_eq!(
        plan.steps
            .iter()
            .map(|step| step.id.as_str())
            .collect::<Vec<_>>(),
        [
            "check-registry",
            "create-repository",
            "create-access-token",
            "check-github-cli",
            "set-image-variable",
            "set-username-variable",
            "set-token-secret",
        ]
    );
    let words = |text: &str| text.split(' ').map(str::to_owned).collect::<Vec<_>>();
    assert_eq!(
        argv(&plan, "set-image-variable"),
        words("gh variable set DOCKERHUB_IMAGE --body acme/pipeline-app --repo acme/pipeline-app")
    );
    assert_eq!(
        argv(&plan, "set-username-variable"),
        words("gh variable set DOCKERHUB_USERNAME --body acme --repo acme/pipeline-app")
    );
    assert_eq!(
        argv(&plan, "set-token-secret"),
        words("gh secret set DOCKERHUB_TOKEN --repo acme/pipeline-app"),
        "the token is read by gh from the terminal, never passed as an argument"
    );
    assert_eq!(
        plan.steps
            .iter()
            .find(|step| step.id == "create-repository")
            .and_then(|step| step.url.as_deref()),
        Some("https://hub.docker.com/repository/create?namespace=acme")
    );

    for package in &mut inspection.packages {
        if package.registry == Registry::DockerHub {
            package.exists_on_registry = Some(true);
        }
    }
    let attach = only_plan(&inspection, Registry::DockerHub);
    assert_eq!(attach.mode, Some(PlanMode::Attach));
    assert!(attach
        .steps
        .iter()
        .all(|step| step.id != "create-repository"));
}

#[test]
fn checks_ghcr_packages_write_and_links_the_package_after_the_first_push() {
    let (_temporary, root) = fixture();
    let plan = only_plan(
        &inspect_repository(&root).expect("inspect fixture"),
        Registry::Ghcr,
    );
    assert_eq!(plan.package.warnings, [] as [String; 0]);
    assert_eq!(
        plan.steps
            .iter()
            .map(|step| step.id.as_str())
            .collect::<Vec<_>>(),
        ["link-package"]
    );
    assert_eq!(
        plan.steps[0].url.as_deref(),
        Some("https://github.com/users/acme/packages/container/package/pipeline-app")
    );

    let workflow = root.join(".github/workflows/release.yml");
    let original = fs::read_to_string(&workflow).expect("read workflow");
    fs::write(
        &workflow,
        original.replace("packages: write", "packages: read"),
    )
    .expect("write workflow");
    let without_permission = only_plan(
        &inspect_repository(&root).expect("inspect fixture"),
        Registry::Ghcr,
    );
    assert!(without_permission.package.warnings[0].contains("packages: write"));
    assert_eq!(without_permission.steps[0].id, "grant-packages-write");
}

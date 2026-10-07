use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use package_registry_manager::browser::npm_prefill_script;
use package_registry_manager::{build_plans, inspect_repository, Registry};
use regex::Regex;
use tempfile::TempDir;

fn fixture() -> (TempDir, PathBuf) {
    let temporary = TempDir::new().expect("create temporary repository");
    let root = temporary.path().join("polyglot");
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/polyglot"),
        &root,
    );
    fs::create_dir_all(root.join(".git")).expect("create fixture git directory");
    fs::write(
        root.join(".git/config"),
        "[remote \"origin\"]\n\turl = git@github.com:acme/polyglot.git\n",
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
fn discovers_the_baseline_polyglot_registries_and_repository_metadata() {
    let (_temporary, root) = fixture();
    let inspection = inspect_repository(&root).expect("inspect fixture");
    let registries = inspection
        .packages
        .iter()
        .map(|package| package.registry)
        .collect::<BTreeSet<_>>();

    assert_eq!(
        registries,
        BTreeSet::from([
            Registry::Npm,
            Registry::CratesIo,
            Registry::PyPi,
            Registry::GoModules,
            Registry::NuGet,
            Registry::MavenCentral,
            Registry::Packagist,
            Registry::DockerHub,
            Registry::Ghcr
        ])
    );
    assert_eq!(inspection.packages.len(), 9);
    assert_eq!(inspection.repository.github_owner.as_deref(), Some("acme"));
    assert_eq!(
        inspection.repository.github_repository.as_deref(),
        Some("polyglot")
    );
    assert_eq!(
        inspection.repository.release_workflow.as_deref(),
        Some("publish.yml")
    );
    assert!(inspection
        .packages
        .iter()
        .all(|package| package.name != "should-not-be-discovered"));

    let mut actual = serde_json::to_value(&inspection).expect("serialize inspection");
    actual["repository"]["root"] = serde_json::Value::String("<ROOT>".to_owned());
    let expected: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(root.join("expected-inspection.json"))
            .expect("read shared expected inspection"),
    )
    .expect("parse shared expected inspection");
    assert_eq!(
        actual, expected,
        "Rust must honor the shared language contract"
    );
}

#[test]
fn npm_plan_prefills_trusted_publisher_from_repository() {
    let (_temporary, root) = fixture();
    let inspection = inspect_repository(&root).expect("inspect fixture");
    let plans = build_plans(&inspection);
    let npm = plans
        .iter()
        .find(|plan| plan.registry == Registry::Npm)
        .expect("npm plan");
    let publisher = npm.oidc_publisher.as_ref().expect("publisher prefill");

    assert_eq!(publisher.organization, "acme");
    assert_eq!(publisher.repository, "polyglot");
    assert_eq!(publisher.workflow, "publish.yml");
    assert!(npm
        .steps
        .iter()
        .any(|step| step.url.as_deref()
            == Some("https://www.npmjs.com/package/@acme%2Fwidgets/access")));

    let script = npm_prefill_script(publisher, false).expect("build browser script");
    assert!(script.contains("publish.yml"));
    assert!(script.contains("shouldSubmit = false"));
}

#[test]
fn plan_json_keeps_the_trusted_publisher_key() {
    let (_temporary, root) = fixture();
    let inspection = inspect_repository(&root).expect("inspect fixture");
    let npm = build_plans(&inspection)
        .into_iter()
        .find(|plan| plan.registry == Registry::Npm)
        .expect("npm plan");
    let json = serde_json::to_value(&npm).expect("serialize plan");

    assert_eq!(json["trusted_publisher"]["workflow"], "publish.yml");
    assert!(json.get("oidc_publisher").is_none());
    let parsed: package_registry_manager::model::SetupPlan =
        serde_json::from_value(json).expect("parse plan");
    assert_eq!(parsed.oidc_publisher, npm.oidc_publisher);
}

#[test]
fn publishes_only_to_bootstrap_a_missing_package_after_confirmation() {
    let (_temporary, root) = fixture();
    let inspection = inspect_repository(&root).expect("inspect fixture");
    let publish = Regex::new(r"^(npm|cargo) publish\b|^twine upload\b").expect("pattern");
    for plan in build_plans(&inspection) {
        for step in &plan.steps {
            let Some(command) = &step.command else {
                continue;
            };
            let rendered = format!("{} {}", command.program, command.args.join(" "));
            if publish.is_match(&rendered) && !rendered.contains("--dry-run") {
                assert_eq!(step.id, "first-publish", "{rendered}");
                assert_eq!(step.when.as_deref(), Some("package-missing"), "{rendered}");
                assert!(step.confirm, "{rendered}");
            }
        }
    }
    let mut complete = inspection;
    for package in &mut complete.packages {
        package.exists_on_registry = Some(true);
        package.trusted_publishing = Some(true);
    }
    for plan in build_plans(&complete) {
        assert!(
            plan.steps.iter().all(|step| step.id != "first-publish"),
            "{}",
            plan.registry
        );
    }
}

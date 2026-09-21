use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use package_registry_manager::browser::npm_prefill_script;
use package_registry_manager::{build_plans, inspect_repository, Registry};
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
fn discovers_every_maintained_registry_and_repository_metadata() {
    let (_temporary, root) = fixture();
    let inspection = inspect_repository(&root).expect("inspect fixture");
    let registries = inspection
        .packages
        .iter()
        .map(|package| package.registry)
        .collect::<BTreeSet<_>>();

    assert_eq!(registries, BTreeSet::from(Registry::ALL));
    assert_eq!(inspection.packages.len(), 7);
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
    let publisher = npm.trusted_publisher.as_ref().expect("publisher prefill");

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
fn setup_plan_never_contains_a_publish_command() {
    let (_temporary, root) = fixture();
    let inspection = inspect_repository(&root).expect("inspect fixture");
    for plan in build_plans(&inspection) {
        for command in plan.steps.iter().filter_map(|step| step.command.as_ref()) {
            let rendered = format!("{} {}", command.program, command.args.join(" "));
            assert!(
                rendered != "npm publish" && rendered != "cargo publish",
                "setup must not upload an artifact: {rendered}"
            );
        }
    }
}

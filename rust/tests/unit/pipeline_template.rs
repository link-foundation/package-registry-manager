use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use package_registry_manager::{build_plans_for, inspect_repository, Registry};
use tempfile::TempDir;

fn fixture() -> (TempDir, PathBuf) {
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
fn plans_no_actionable_steps_for_unpublishable_packages() {
    let (_temporary, root) = fixture();
    let inspection = inspect_repository(&root).expect("inspect fixture");
    let plans = build_plans_for(&inspection, &BTreeSet::from([Registry::Npm]));
    let private = plans
        .iter()
        .find(|plan| plan.package.name == "universal-example-app")
        .expect("private plan");
    assert!(!private.package.publishable);
    assert!(private.steps.is_empty());
    assert!(private.trusted_publisher.is_none());
    assert_eq!(
        private.skipped_reason.as_deref(),
        Some("package.json marks this package as private")
    );

    let public = plans
        .iter()
        .find(|plan| plan.package.name == "pipeline-app")
        .expect("public plan");
    assert!(public.skipped_reason.is_none());
    assert!(!public.steps.is_empty());
    assert_eq!(
        public
            .trusted_publisher
            .as_ref()
            .map(|publisher| publisher.workflow.as_str()),
        Some("release.yml")
    );
}

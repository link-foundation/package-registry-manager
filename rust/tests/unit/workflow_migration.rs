use package_registry_manager::workflows::Workflow;
use package_registry_manager::{build_plans_with, inspect_repository, PlanOptions, Registry};
use serde_json::json;
use std::{collections::BTreeSet, path::Path};

#[test]
fn nested_reusable_callers_keep_the_outer_identity_and_leaf_environment() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/token-rotation");
    let workflows: Vec<_> = [
        ("outer.yml", "on: push\njobs:\n  call:\n    uses: ./.github/workflows/middle.yml\n    secrets:\n      CARGO_TOKEN: ${{ secrets.CARGO_TOKEN }}\n"),
        ("middle.yml", "on: workflow_call\njobs:\n  call:\n    uses: ./.github/workflows/inner.yml\n    secrets:\n      CARGO_TOKEN: ${{ secrets.CARGO_TOKEN }}\n"),
        ("inner.yml", "on: workflow_call\njobs:\n  release:\n    environment: release-main\n    steps:\n      - run: cargo publish\n        env:\n          CARGO_TOKEN: ${{ secrets.CARGO_TOKEN }}\n"),
    ].into_iter().map(|(name, contents)| Workflow {name: name.into(), contents: contents.into()}).collect();
    let publisher = package_registry_manager::publishers::detect_publisher(
        &root,
        &workflows,
        Registry::CratesIo,
    );
    assert_eq!(publisher.workflow.as_deref(), Some("outer.yml"));
    assert_eq!(publisher.environment.as_deref(), Some("release-main"));
    let inspection = inspect_repository(&root).unwrap();
    let files = package_registry_manager::workflow_migration::workflow_migration_proposal(
        &root,
        &inspection.packages,
        &workflows,
    )
    .unwrap();
    assert_eq!(files.len(), 3);
    for contents in files.values() {
        assert!(contents.contains("id-token: write"));
        assert!(!contents.contains("secrets.CARGO_TOKEN"));
    }
}

#[test]
fn pypi_conversion_preserves_conditions_and_package_directory() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/token-rotation");
    let mut package = inspect_repository(&root).unwrap().packages.remove(0);
    package.registry = Registry::PyPi;
    let contents = "on: push\njobs:\n  publish:\n    steps:\n      - name: Upload Python package\n        id: publish\n        if: steps.release.outputs.version != ''\n        run: python -m twine upload python/dist/*\n        env:\n          TWINE_PASSWORD: ${{ secrets.PYPI_TOKEN }}\n          OTHER: retained\n";
    let files = package_registry_manager::workflow_migration::workflow_migration_proposal(
        &root,
        &[package.clone()],
        &[Workflow {
            name: "release.yml".into(),
            contents: contents.into(),
        }],
    )
    .unwrap();
    assert!(files["release.yml"].contains("id: publish"));
    assert!(files["release.yml"].contains("if: steps.release.outputs.version"));
    assert!(files["release.yml"].contains("packages-dir: 'python/dist/'"));
    assert!(files["release.yml"].contains("OTHER: retained"));
    assert!(!files["release.yml"].contains("TWINE_PASSWORD"));
    assert!(
        package_registry_manager::workflow_migration::workflow_migration_proposal(
            &root,
            &[package],
            &[Workflow {
                name: "release.yml".into(),
                contents: contents.replace(
                    "twine upload python/dist/*",
                    "twine upload --skip-existing python/dist/*"
                )
            }]
        )
        .is_err()
    );
}

#[test]
fn reusable_caller_environment_and_migration_match_javascript_golden_files() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/token-rotation");
    let inspection = inspect_repository(&root).unwrap();
    assert_eq!(
        inspection.packages[0].environment.as_deref(),
        Some("release-main")
    );
    let plans = build_plans_with(
        &inspection,
        &BTreeSet::from([Registry::CratesIo]),
        &PlanOptions {
            migrate: true,
            ..PlanOptions::default()
        },
    );
    assert_eq!(plans[0].steps[0].id, "add-publishing-workflow");
    let files = package_registry_manager::workflow_migration::workflow_migration_proposal(
        &root,
        &inspection.packages,
        &package_registry_manager::workflows::read_workflows(&root).unwrap(),
    )
    .unwrap();
    let expected: std::collections::BTreeMap<String, String> = serde_json::from_str(include_str!(
        "../../../tests/fixtures/token-rotation/expected.json"
    ))
    .unwrap();
    assert_eq!(files, expected);
}

#[test]
fn oidc_evidence_requires_successful_authentication_and_an_actual_upload() {
    use package_registry_manager::migration_evidence::published_through_oidc;
    let job = json!({"conclusion":"success","steps":[{"name":"Authenticate with crates.io (trusted publishing)","conclusion":"success"},{"name":"Publish crate","conclusion":"success"}]});
    assert!(published_through_oidc(
        &job,
        "Published crate v1.0.1",
        Registry::CratesIo
    ));
    assert!(!published_through_oidc(
        &job,
        "version already exists; skipping publish",
        Registry::CratesIo
    ));
    assert!(!published_through_oidc(
        &json!({"conclusion":"success","steps":[]}),
        "Published crate v1.0.1",
        Registry::CratesIo
    ));
}
